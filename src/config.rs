use crate::cli::{self, tri};
use serde::Deserialize;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub port: Option<u16>,
    pub bind: Option<IpAddr>,
    pub hidden: Option<bool>,
    pub follow_links: Option<bool>,
    pub dir_sizes: Option<bool>,
    pub qr: Option<bool>,
    pub zip: Option<bool>,
    pub upload: Option<bool>,
    pub max_upload_size: Option<String>,
    pub auth: Option<AuthCfg>,
    pub tls: Option<bool>,
    pub limit: Option<String>,
    pub mdns: Option<bool>,
    pub json_log: Option<bool>,
    pub secure: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AuthCfg {
    Enabled(bool),  // auth = true → generated creds; false → off
    Creds(String),  // auth = "user" or "user:pass"
}

/// Effective settings after CLI > config > default merge + secure expansion.
#[derive(Debug, PartialEq)]
pub struct Settings {
    pub port: Option<u16>,
    pub bind: IpAddr,
    pub hidden: bool,
    pub follow_links: bool,
    pub dir_sizes: bool,
    pub qr: bool,
    pub zip: bool,
    pub upload: bool,
    pub max_upload_size: Option<u64>,
    /// None = off, Some(None) = generated creds, Some(Some("u[:p]")) = given
    pub auth: Option<Option<String>>,
    pub tls: bool,
    pub limit: Option<u64>,
    pub mdns: bool,
    pub json_log: bool,
    pub token: bool,
    pub secure: bool,
}

pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FSHARE_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("fshare/config.toml"))
}

pub fn load(path: &Path) -> Result<Option<Config>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn resolve(a: &cli::Args, c: &Config) -> Result<Settings, String> {
    let cli_tls = tri(a.tls, a.no_tls);
    let cli_mdns = tri(a.mdns, a.no_mdns);
    let secure = tri(a.secure, a.no_secure).or(c.secure).unwrap_or(false);

    // auth as tri-state over Option<Option<String>>
    let cli_auth: Option<Option<Option<String>>> = if a.no_auth {
        Some(None)
    } else {
        a.auth.as_ref().map(|v| Some(v.clone()))
    };
    let cfg_auth: Option<Option<Option<String>>> = c.auth.as_ref().map(|v| match v {
        AuthCfg::Enabled(true) => Some(None),
        AuthCfg::Enabled(false) => None,
        AuthCfg::Creds(s) => Some(Some(s.clone())),
    });

    let mut tls = cli_tls.or(c.tls).unwrap_or(false);
    let mut mdns = cli_mdns.or(c.mdns).unwrap_or(true);
    let mut auth = cli_auth.clone().or(cfg_auth.clone()).unwrap_or(None);
    let mut token = a.token;

    if secure {
        // bundle fills only what nobody set explicitly (CLI or config)
        if cli_tls.or(c.tls).is_none() {
            tls = true;
        }
        if cli_auth.or(cfg_auth).is_none() {
            auth = Some(None);
        }
        if cli_mdns.or(c.mdns).is_none() {
            mdns = false;
        }
        token = true; // --token has no inverse; secure always tokens the URL
    }

    let limit = match a.limit {
        Some(0) => None,
        Some(n) => Some(n),
        None => match &c.limit {
            Some(s) => match cli::parse_size(s).map_err(|e| format!("config limit: {e}"))? {
                0 => None,
                n => Some(n),
            },
            None => None,
        },
    };
    let max_upload_size = match a.max_upload_size {
        Some(n) => Some(n),
        None => c
            .max_upload_size
            .as_deref()
            .map(cli::parse_size)
            .transpose()
            .map_err(|e| format!("config max_upload_size: {e}"))?,
    };

    Ok(Settings {
        port: a.port.or(c.port),
        bind: a.bind.or(c.bind).unwrap_or_else(|| "0.0.0.0".parse().unwrap()),
        hidden: tri(a.hidden, a.no_hidden).or(c.hidden).unwrap_or(false),
        follow_links: tri(a.follow_links, a.no_follow_links).or(c.follow_links).unwrap_or(false),
        dir_sizes: tri(a.dir_sizes, a.no_dir_sizes).or(c.dir_sizes).unwrap_or(false),
        qr: tri(a.qr, a.no_qr).or(c.qr).unwrap_or(true),
        zip: tri(a.zip, a.no_zip).or(c.zip).unwrap_or(true),
        upload: tri(a.upload, a.no_upload).or(c.upload).unwrap_or(false),
        max_upload_size,
        auth,
        tls,
        limit,
        mdns,
        json_log: tri(a.json_log, a.no_json_log).or(c.json_log).unwrap_or(false),
        token,
        secure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args(v: &[&str]) -> cli::Args {
        cli::Args::parse_from(std::iter::once("fshare").chain(v.iter().copied()))
    }

    fn cfg(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn load_missing_is_none() {
        assert!(load(Path::new("/nonexistent/fshare.toml")).unwrap().is_none());
    }

    #[test]
    fn load_empty_file_is_none() {
        let t = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(t.path(), "").unwrap();
        assert!(load(t.path()).unwrap().is_none());
        std::fs::write(t.path(), "   \n\n").unwrap();
        assert!(load(t.path()).unwrap().is_none());
    }

    #[test]
    fn load_rejects_unknown_key() {
        let t = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(t.path(), "portt = 9000\n").unwrap();
        let err = load(t.path()).unwrap_err();
        assert!(err.contains("portt"), "{err}");
    }

    #[test]
    fn load_rejects_bad_toml() {
        let t = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(t.path(), "port = = 9\n").unwrap();
        assert!(load(t.path()).is_err());
    }

    #[test]
    fn defaults_when_everything_absent() {
        let s = resolve(&args(&[]), &Config::default()).unwrap();
        assert_eq!(s.port, None);
        assert_eq!(s.bind.to_string(), "0.0.0.0");
        assert!(s.mdns && s.qr && s.zip);
        assert!(!s.tls && !s.upload && !s.secure && !s.token);
        assert_eq!(s.auth, None);
        assert_eq!(s.limit, None);
    }

    #[test]
    fn config_beats_default_cli_beats_config() {
        let c = cfg("mdns = false\nport = 9000\nupload = true\nlimit = \"1M\"");
        let s = resolve(&args(&[]), &c).unwrap();
        assert!(!s.mdns && s.upload);
        assert_eq!(s.port, Some(9000));
        assert_eq!(s.limit, Some(1 << 20));
        // CLI flips config both directions
        let s = resolve(&args(&["--mdns", "--no-upload", "--limit", "0", "--port", "8123"]), &c).unwrap();
        assert!(s.mdns && !s.upload);
        assert_eq!(s.limit, None);
        assert_eq!(s.port, Some(8123));
    }

    #[test]
    fn config_auth_forms() {
        assert_eq!(resolve(&args(&[]), &cfg("auth = true")).unwrap().auth, Some(None));
        assert_eq!(resolve(&args(&[]), &cfg("auth = false")).unwrap().auth, None);
        assert_eq!(
            resolve(&args(&[]), &cfg("auth = \"bob:pw\"")).unwrap().auth,
            Some(Some("bob:pw".into()))
        );
        // CLI --no-auth beats config creds
        assert_eq!(resolve(&args(&["--no-auth"]), &cfg("auth = \"bob:pw\"")).unwrap().auth, None);
    }

    #[test]
    fn secure_bundle_and_overrides() {
        let s = resolve(&args(&["--secure"]), &Config::default()).unwrap();
        assert!(s.tls && s.token && !s.mdns && s.secure);
        assert_eq!(s.auth, Some(None));
        // explicit CLI wins inside bundle
        let s = resolve(&args(&["--secure", "--auth=bob:pw", "--mdns"]), &Config::default()).unwrap();
        assert!(s.tls && s.token && s.mdns);
        assert_eq!(s.auth, Some(Some("bob:pw".into())));
        // explicit config wins inside bundle too
        let s = resolve(&args(&["--secure"]), &cfg("tls = false")).unwrap();
        assert!(!s.tls && s.token);
        // secure from config, disabled from CLI
        let s = resolve(&args(&["--no-secure"]), &cfg("secure = true")).unwrap();
        assert!(!s.secure && !s.tls && s.mdns);
    }

    #[test]
    fn config_bad_limit_is_error() {
        assert!(resolve(&args(&[]), &cfg("limit = \"5X\"")).is_err());
    }
}
