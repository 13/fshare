use crate::net::{ranked_ifaces, IfaceKind};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::IpAddr;

pub fn sanitize_hostname(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true; // suppress leading dashes
    for ch in raw.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "host".to_string() } else { out.to_string() }
}

/// e.g. "fshare-benpc" — unique per machine, shared by all local instances.
pub fn host_label() -> String {
    format!("fshare-{}", sanitize_hostname(&machine_hostname()))
}

pub fn mdns_host() -> String {
    format!("{}.local.", host_label())
}

pub fn machine_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "host".to_string())
}

pub fn instance_name(host: &str, port: u16) -> String {
    if port == crate::net::DEFAULT_PORT {
        format!("fshare on {host}")
    } else {
        format!("fshare on {host} ({port})")
    }
}

pub struct MdnsGuard {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for MdnsGuard {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// TXT record path we advertise. Always "/" — never the real (possibly
/// token-bearing) base path, since mDNS TXT records are broadcast in the
/// clear to every listener on the local network.
fn txt_path(_base: &str) -> String {
    "/".to_string()
}

pub fn announce(port: u16, base: &str) -> Result<MdnsGuard, String> {
    let ips: Vec<IpAddr> = ranked_ifaces()
        .into_iter()
        .filter(|i| i.kind != IfaceKind::Loopback)
        .map(|i| i.ip)
        .collect();
    if ips.is_empty() {
        return Err("no non-loopback interfaces".into());
    }
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let path = txt_path(base);
    let props = [("path", path.as_str())];
    let info = ServiceInfo::new(
        "_http._tcp.local.",
        &instance_name(&machine_hostname(), port),
        &mdns_host(),
        &ips[..],
        port,
        &props[..],
    )
    .map_err(|e| e.to_string())?;
    let fullname = info.get_fullname().to_string();
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok(MdnsGuard { daemon, fullname })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_hostnames() {
        assert_eq!(sanitize_hostname("benpc"), "benpc");
        assert_eq!(sanitize_hostname("Ben's PC"), "ben-s-pc");
        assert_eq!(sanitize_hostname("--weird__name--"), "weird-name");
        assert_eq!(sanitize_hostname("???"), "host");
        assert_eq!(sanitize_hostname(""), "host");
    }

    #[test]
    fn host_label_prefixed() {
        assert!(host_label().starts_with("fshare-"));
        assert!(mdns_host().ends_with(".local."));
    }

    #[test]
    fn txt_path_never_leaks_token() {
        assert_eq!(txt_path(""), "/");
        assert_eq!(txt_path("/s/abc"), "/");
    }

    #[test]
    fn instance_names() {
        assert_eq!(instance_name("ben-pc", 8000), "fshare on ben-pc");
        assert_eq!(instance_name("ben-pc", 8001), "fshare on ben-pc (8001)");
    }

    #[test]
    #[ignore] // real multicast; run manually: cargo test mdns_browse_back -- --ignored
    fn mdns_browse_back() {
        let guard = announce(18999, "").expect("announce");
        let daemon = mdns_sd::ServiceDaemon::new().unwrap();
        let rx = daemon.browse("_http._tcp.local.").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            if let Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) =
                rx.recv_timeout(std::time::Duration::from_millis(300))
            {
                if info.get_fullname().starts_with("fshare on") && info.get_port() == 18999 {
                    found = true;
                    break;
                }
            }
        }
        drop(guard);
        assert!(found, "service not discovered within 3s");
    }
}
