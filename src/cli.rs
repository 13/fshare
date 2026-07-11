use clap::Parser;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Share the current directory (or a single file) over HTTP on your LAN.
#[derive(Parser, Debug)]
#[command(name = "fshare", version, about)]
pub struct Args {
    /// Directory or single file to share
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Exact port (disables auto-bump; errors if busy)
    #[arg(short, long)]
    pub port: Option<u16>,

    /// Address to bind (default 0.0.0.0)
    #[arg(long)]
    pub bind: Option<IpAddr>,

    /// Auto-shutdown after duration, e.g. 30m, 2h, 90s
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,

    /// Shut down after N completed file downloads
    #[arg(long)]
    pub max_downloads: Option<u64>,

    /// Serve under a random /s/<token>/ prefix
    #[arg(long)]
    pub token: bool,

    /// Disable folder zip downloads
    #[arg(long, overrides_with = "zip")]
    pub no_zip: bool,
    /// Enable folder zip downloads (override config)
    #[arg(long, overrides_with = "no_zip")]
    pub zip: bool,

    /// Show dotfiles
    #[arg(long, overrides_with = "no_hidden")]
    pub hidden: bool,
    /// Hide dotfiles (override config)
    #[arg(long, overrides_with = "hidden")]
    pub no_hidden: bool,

    /// Don't print the QR code
    #[arg(long, overrides_with = "qr")]
    pub no_qr: bool,
    /// Print the QR code (override config)
    #[arg(long, overrides_with = "no_qr")]
    pub qr: bool,

    /// Machine-readable JSON-lines event log
    #[arg(long, overrides_with = "no_json_log")]
    pub json_log: bool,
    /// Human-readable log (override config)
    #[arg(long, overrides_with = "json_log")]
    pub no_json_log: bool,

    /// Allow symlinks that point outside the shared root
    #[arg(long, overrides_with = "no_follow_links")]
    pub follow_links: bool,
    /// Don't follow symlinks outside the root (override config)
    #[arg(long, overrides_with = "follow_links")]
    pub no_follow_links: bool,

    /// Enable uploads (drag & drop on the listing page)
    #[arg(long, overrides_with = "no_upload")]
    pub upload: bool,
    /// Disable uploads (override config)
    #[arg(long, overrides_with = "upload")]
    pub no_upload: bool,

    /// Reject uploads larger than this, e.g. 500M, 2G (default unlimited)
    #[arg(long, value_parser = parse_size)]
    pub max_upload_size: Option<u64>,

    /// Require HTTP Basic auth: --auth (generated), --auth=user or --auth=user:pass
    #[arg(long, require_equals = true, value_name = "USER[:PASS]", overrides_with = "no_auth")]
    pub auth: Option<Option<String>>,
    /// Disable auth (override config)
    #[arg(long, overrides_with = "auth")]
    pub no_auth: bool,

    /// Don't announce fshare-<hostname>.local via mDNS
    #[arg(long, overrides_with = "mdns")]
    pub no_mdns: bool,
    /// Announce via mDNS (override config)
    #[arg(long, overrides_with = "no_mdns")]
    pub mdns: bool,

    /// Serve HTTPS with a persisted self-signed certificate
    #[arg(long, overrides_with = "no_tls")]
    pub tls: bool,
    /// Serve plain HTTP (override config)
    #[arg(long, overrides_with = "tls")]
    pub no_tls: bool,

    /// Show recursive directory sizes in listings (walks subtrees per page view)
    #[arg(long, overrides_with = "no_dir_sizes")]
    pub dir_sizes: bool,
    /// Hide directory sizes (override config)
    #[arg(long, overrides_with = "dir_sizes")]
    pub no_dir_sizes: bool,

    /// Public-network bundle: TLS + auth + token URL, mDNS off
    #[arg(long, overrides_with = "no_secure")]
    pub secure: bool,
    /// Disable secure bundle (override config)
    #[arg(long, overrides_with = "secure")]
    pub no_secure: bool,

    /// Cap total download speed, e.g. --limit 5M (bytes/second, all clients; 0 = unlimited)
    #[arg(long, value_parser = parse_limit)]
    pub limit: Option<u64>,
}

/// Positive/negative flag pair to tri-state: set on / set off / absent.
pub fn tri(pos: bool, neg: bool) -> Option<bool> {
    match (pos, neg) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

fn parse_limit(s: &str) -> Result<u64, String> {
    parse_size(s) // 0 = unlimited (overrides a config limit)
}

pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().map_err(|_| format!("invalid size '{s}'"))?;
    let mult: u64 = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1 << 10,
        "M" | "MB" => 1 << 20,
        "G" | "GB" => 1 << 30,
        u => return Err(format!("unknown size unit '{u}': use K, M or G")),
    };
    n.checked_mul(mult).ok_or_else(|| "size overflows".to_string())
}

pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| "missing unit: use s, m or h (e.g. 30m)".to_string())?;
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().map_err(|_| format!("invalid number '{num}'"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        u => return Err(format!("unknown unit '{u}': use s, m or h")),
    };
    Ok(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30s").unwrap().as_secs(), 30);
        assert_eq!(parse_duration("30m").unwrap().as_secs(), 1800);
        assert_eq!(parse_duration("2h").unwrap().as_secs(), 7200);
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("x5m").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("500").unwrap(), 500);
        assert_eq!(parse_size("500K").unwrap(), 500 * 1024);
        assert_eq!(parse_size("2M").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("3g").unwrap(), 3 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1GB").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_size("").is_err());
        assert!(parse_size("x5").is_err());
        assert!(parse_size("5X").is_err());
    }

    #[test]
    fn limit_zero_allowed() {
        assert_eq!(parse_limit("0").unwrap(), 0);
        assert_eq!(parse_limit("5M").unwrap(), 5 * 1024 * 1024);
    }

    #[test]
    fn tri_state() {
        assert_eq!(tri(true, false), Some(true));
        assert_eq!(tri(false, true), Some(false));
        assert_eq!(tri(false, false), None);
    }

    #[test]
    fn flag_pairs_last_wins() {
        let a = Args::parse_from(["fshare", "--no-mdns", "--mdns"]);
        assert!(a.mdns && !a.no_mdns);
        let a = Args::parse_from(["fshare", "--tls", "--no-tls"]);
        assert!(a.no_tls && !a.tls);
        let a = Args::parse_from(["fshare", "--secure"]);
        assert!(a.secure && !a.no_secure);
        let a = Args::parse_from(["fshare", "--no-auth"]);
        assert!(a.no_auth);
        let a = Args::parse_from(["fshare"]);
        assert_eq!(a.bind, None);
    }
}
