use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener};

pub const DEFAULT_PORT: u16 = 8000;
const BUMP_LIMIT: u16 = 10;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum IfaceKind {
    Lan,
    Other,
    Loopback,
}

pub fn rank(ip: IpAddr) -> IfaceKind {
    match ip {
        IpAddr::V4(v4) if v4.is_loopback() => IfaceKind::Loopback,
        IpAddr::V4(v4) if v4.is_private() => IfaceKind::Lan,
        IpAddr::V6(v6) if v6.is_loopback() => IfaceKind::Loopback,
        _ => IfaceKind::Other,
    }
}

#[derive(Debug)]
pub struct Iface {
    pub name: String,
    pub ip: IpAddr,
    pub kind: IfaceKind,
}

pub fn ranked_ifaces() -> Vec<Iface> {
    let mut v: Vec<Iface> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| match i.ip() {
            IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80, // drop link-local
            IpAddr::V4(_) => true,
        })
        .map(|i| Iface { kind: rank(i.ip()), ip: i.ip(), name: i.name })
        .collect();
    v.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.ip.is_ipv6().cmp(&b.ip.is_ipv6())));
    v
}

pub fn bind_port(bind: IpAddr, exact: Option<u16>) -> io::Result<(TcpListener, u16, bool)> {
    bind_port_from(bind, exact, DEFAULT_PORT)
}

pub fn bind_port_from(
    bind: IpAddr,
    exact: Option<u16>,
    base: u16,
) -> io::Result<(TcpListener, u16, bool)> {
    if let Some(p) = exact {
        let l = TcpListener::bind(SocketAddr::new(bind, p))?;
        return Ok((l, p, false));
    }
    let mut last_err = None;
    for (i, p) in (base..=base.saturating_add(BUMP_LIMIT)).enumerate() {
        match TcpListener::bind(SocketAddr::new(bind, p)) {
            Ok(l) => return Ok((l, p, i > 0)),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("no port available")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn ranks_ips() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(rank(ip("192.168.1.5")), IfaceKind::Lan);
        assert_eq!(rank(ip("10.0.3.2")), IfaceKind::Lan);
        assert_eq!(rank(ip("172.16.0.9")), IfaceKind::Lan);
        assert_eq!(rank(ip("100.64.1.2")), IfaceKind::Other); // tailscale CGNAT
        assert_eq!(rank(ip("127.0.0.1")), IfaceKind::Loopback);
        assert_eq!(rank(ip("::1")), IfaceKind::Loopback);
        assert!(IfaceKind::Lan < IfaceKind::Other && IfaceKind::Other < IfaceKind::Loopback);
    }

    #[test]
    fn bumps_busy_port() {
        let hold = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = hold.local_addr().unwrap().port();
        let (l, port, bumped) =
            bind_port_from("127.0.0.1".parse().unwrap(), None, busy).unwrap();
        assert!(bumped);
        assert_eq!(port, busy + 1);
        drop(l);
        assert!(bind_port_from("127.0.0.1".parse().unwrap(), Some(busy), busy).is_err());
    }
}
