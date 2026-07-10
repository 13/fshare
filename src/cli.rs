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

    /// Address to bind
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: IpAddr,

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
    #[arg(long)]
    pub no_zip: bool,

    /// Show dotfiles
    #[arg(long)]
    pub hidden: bool,

    /// Don't print the QR code
    #[arg(long)]
    pub no_qr: bool,

    /// Machine-readable JSON-lines event log
    #[arg(long)]
    pub json_log: bool,

    /// Allow symlinks that point outside the shared root
    #[arg(long)]
    pub follow_links: bool,

    /// Enable uploads (drag & drop on the listing page)
    #[arg(long)]
    pub upload: bool,

    /// Reject uploads larger than this, e.g. 500M, 2G (default unlimited)
    #[arg(long, value_parser = parse_size)]
    pub max_upload_size: Option<u64>,

    /// Require HTTP Basic auth: --auth (generated), --auth=user or --auth=user:pass
    #[arg(long, require_equals = true, value_name = "USER[:PASS]")]
    pub auth: Option<Option<String>>,

    /// Don't announce fshare.local via mDNS
    #[arg(long)]
    pub no_mdns: bool,

    /// Serve HTTPS with a persisted self-signed certificate
    #[arg(long)]
    pub tls: bool,
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
}
