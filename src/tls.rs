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
    if cert.exists() && key.exists() {
        let fp = fingerprint_of(&cert)?;
        return Ok(TlsPaths { cert, key, fingerprint: fp, generated: false });
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let mut params = rcgen::CertificateParams::new(sans.to_vec()).map_err(|e| e.to_string())?;
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3650);
    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let certificate = params.self_signed(&key_pair).map_err(|e| e.to_string())?;

    std::fs::write(&cert, certificate.pem()).map_err(|e| e.to_string())?;
    std::fs::write(&key, key_pair.serialize_pem()).map_err(|e| e.to_string())?;
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
}
