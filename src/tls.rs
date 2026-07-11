use base64::Engine;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub fingerprint: String,
    pub generated: bool,
}

pub fn data_dir() -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(d) => PathBuf::from(d).join("fshare"),
        None => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local/share/fshare")
        }
    }
}

pub fn load_or_generate(dir: &Path, sans: &[String]) -> Result<TlsPaths, String> {
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    let sans_file = dir.join("sans.txt");
    let stored: Vec<String> = std::fs::read_to_string(&sans_file)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let covered = sans.iter().all(|s| stored.contains(s));
    if cert.exists() && key.exists() && covered {
        let fp = fingerprint_of(&cert)?;
        return Ok(TlsPaths { cert, key, fingerprint: fp, generated: false });
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    // Regenerate over the union of previously-stored and newly-requested
    // SANs (stored first, then new ones appended) so alternating networks
    // (e.g. a laptop moving between IP A and IP B) don't churn the cert on
    // every switch — once both SANs have been seen, either one is covered.
    let mut union_sans = stored.clone();
    for s in sans {
        if !union_sans.contains(s) {
            union_sans.push(s.clone());
        }
    }

    let mut params = rcgen::CertificateParams::new(union_sans.clone()).map_err(|e| e.to_string())?;
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3650);
    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let certificate = params.self_signed(&key_pair).map_err(|e| e.to_string())?;

    std::fs::write(&cert, certificate.pem()).map_err(|e| e.to_string())?;
    std::fs::write(&key, key_pair.serialize_pem()).map_err(|e| e.to_string())?;
    std::fs::write(&sans_file, union_sans.join("\n")).map_err(|e| e.to_string())?;
    let mut perms = std::fs::metadata(&key).map_err(|e| e.to_string())?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o600);
    std::fs::set_permissions(&key, perms).map_err(|e| e.to_string())?;

    let fp = fingerprint_of(&cert)?;
    Ok(TlsPaths { cert, key, fingerprint: fp, generated: true })
}

fn fingerprint_of(cert_pem: &Path) -> Result<String, String> {
    let pem = std::fs::read_to_string(cert_pem)
        .map_err(|e| format!("cannot read {}: {e}", cert_pem.display()))?;
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .map_err(|_| format!("corrupt PEM in {}", cert_pem.display()))?;
    let hash = Sha256::digest(&der);
    Ok(hash.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn generates_and_reuses() {
        let t = tempfile::tempdir().unwrap();
        let sans = vec!["fshare.local".to_string(), "192.168.1.5".to_string()];
        let a = load_or_generate(t.path(), &sans).unwrap();
        assert!(a.generated);
        assert!(a.cert.exists() && a.key.exists());
        let mode = std::fs::metadata(&a.key).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let pem = std::fs::read_to_string(&a.cert).unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(a.fingerprint.len(), 32 * 3 - 1);
        assert!(a.fingerprint.chars().all(|c| c.is_ascii_hexdigit() || c == ':'));
        let before = std::fs::read(&a.cert).unwrap();
        let b = load_or_generate(t.path(), &sans).unwrap();
        assert!(!b.generated);
        assert_eq!(b.fingerprint, a.fingerprint);
        assert_eq!(std::fs::read(&b.cert).unwrap(), before);
    }

    #[test]
    fn regenerates_when_san_missing() {
        let t = tempfile::tempdir().unwrap();
        let a = load_or_generate(t.path(), &["fshare-old.local".to_string()]).unwrap();
        assert!(a.generated);
        // same SANs: reuse
        let b = load_or_generate(t.path(), &["fshare-old.local".to_string()]).unwrap();
        assert!(!b.generated);
        // new SAN not covered: regenerate
        let c = load_or_generate(t.path(), &["fshare-new.local".to_string()]).unwrap();
        assert!(c.generated);
        assert_ne!(c.fingerprint, a.fingerprint);
        // regenerated cert covers new SAN: reuse again
        let d = load_or_generate(t.path(), &["fshare-new.local".to_string()]).unwrap();
        assert!(!d.generated);
    }

    #[test]
    fn regenerate_unions_sans_to_avoid_churn() {
        let t = tempfile::tempdir().unwrap();
        let a = load_or_generate(t.path(), &["a.local".to_string()]).unwrap();
        assert!(a.generated);
        // requesting a different SAN regenerates (b not covered)
        let b = load_or_generate(t.path(), &["b.local".to_string()]).unwrap();
        assert!(b.generated);
        assert_ne!(b.fingerprint, a.fingerprint);
        // union of {a, b} is now stored, so re-requesting "a" alone is covered: no regen
        let c = load_or_generate(t.path(), &["a.local".to_string()]).unwrap();
        assert!(!c.generated);
        assert_eq!(c.fingerprint, b.fingerprint);
    }
}
