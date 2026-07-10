# fshare mDNS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Announce `fshare.local` + DNS-SD `_http._tcp` service by default (`--no-mdns` opt-out); banner shows the .local URL.

**Architecture:** New `src/mdns.rs` wraps `mdns-sd`: `announce(port, base)` registers a `ServiceInfo` (hostname `fshare.local.`, IPs from `net::ranked_ifaces()`), returns `MdnsGuard` whose Drop unregisters. `main.rs` calls it unless `--no-mdns`; failure = banner note, never fatal.

**Tech Stack:** `mdns-sd` crate (pure Rust responder).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-10-fshare-mdns-design.md`.
- mDNS failure never fatal — degrade to `note: mDNS unavailable: <e>`.
- Instance name: `fshare on <host>` (port 8000) / `fshare on <host> (<port>)` otherwise.
- Hostname source: `/etc/hostname` trimmed, fallback `"host"`.
- QR keeps numeric-IP URL; banner .local line added above IP list.
- Multicast unreliable in CI → browse-back integration test is `#[ignore]`.
- mdns-sd API drift: adapt to compiler/docs if signatures differ; spec behavior is authoritative.

---

### Task 1: mdns.rs

**Files:**
- Create: `src/mdns.rs`; Modify: `src/lib.rs` (add `pub mod mdns;`), `Cargo.toml` (add `mdns-sd = "0.13"`)

**Interfaces:**
- Consumes: `net::{ranked_ifaces, IfaceKind}`.
- Produces:
  - `mdns::instance_name(host: &str, port: u16) -> String`
  - `mdns::machine_hostname() -> String`
  - `mdns::announce(port: u16, base: &str) -> Result<MdnsGuard, String>`
  - `mdns::MdnsGuard` (Drop unregisters + shuts down daemon)

- [ ] **Step 1: Failing unit tests** in `src/mdns.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names() {
        assert_eq!(instance_name("ben-pc", 8000), "fshare on ben-pc");
        assert_eq!(instance_name("ben-pc", 8001), "fshare on ben-pc (8001)");
    }

    #[test]
    #[ignore] // real multicast; run manually: cargo test mdns_browse -- --ignored
    fn mdns_browse_back() {
        let guard = announce(18999, "").expect("announce");
        let daemon = mdns_sd::ServiceDaemon::new().unwrap();
        let rx = daemon.browse("_http._tcp.local.").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            if let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
                if let mdns_sd::ServiceEvent::ServiceResolved(info) = ev {
                    if info.get_fullname().starts_with("fshare on") && info.get_port() == 18999 {
                        found = true;
                        break;
                    }
                }
            }
        }
        drop(guard);
        assert!(found, "service not discovered within 3s");
    }
}
```

Run: `cargo test mdns::` — FAIL.

- [ ] **Step 2: Implement**

```rust
use crate::net::{ranked_ifaces, IfaceKind};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::IpAddr;

pub const MDNS_HOST: &str = "fshare.local.";

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
    let path = if base.is_empty() { "/".to_string() } else { format!("{base}/") };
    let props = [("path", path.as_str())];
    let info = ServiceInfo::new(
        "_http._tcp.local.",
        &instance_name(&machine_hostname(), port),
        MDNS_HOST,
        &ips[..],
        port,
        &props[..],
    )
    .map_err(|e| e.to_string())?;
    let fullname = info.get_fullname().to_string();
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok(MdnsGuard { daemon, fullname })
}
```

`IfaceKind` needs `PartialEq` — already derives it. `net::DEFAULT_PORT` already pub.

- [ ] **Step 3: Verify** — `cargo test mdns::` PASS (browse test ignored).
- [ ] **Step 4: Commit** — `git commit -am "feat: mDNS responder announcing fshare.local and DNS-SD service"`

---

### Task 2: CLI + main wiring + banner + README

**Files:**
- Modify: `src/cli.rs`, `src/main.rs`, `README.md`

**Interfaces:**
- Consumes: `mdns::{announce, MdnsGuard}`.
- Produces: `Args.no_mdns: bool`.

- [ ] **Step 1: CLI** — `src/cli.rs` after `auth` field:

```rust
    /// Don't announce fshare.local via mDNS
    #[arg(long)]
    pub no_mdns: bool,
```

- [ ] **Step 2: main wiring** — in `async_main`, after `print_banner` call, replace banner call area:

Actually announce BEFORE banner so banner can show status. Insert before `print_banner`:

```rust
    let mdns_guard = if args.no_mdns {
        None
    } else {
        match fshare::mdns::announce(port, &state.base) {
            Ok(g) => Some(g),
            Err(e) => {
                println!("  {} mDNS unavailable: {e}", "note:".yellow());
                None
            }
        }
    };
```

Keep `mdns_guard` alive until after `tokio::select!` (it is — binding lives to end of `async_main`; silence unused warning via `let _mdns_guard = ...` naming). Pass `mdns_on: mdns_guard.is_some()` into `print_banner` (new `bool` param) and inside `print_banner`, before the interface loop:

```rust
    if mdns_on {
        println!("  {} http://fshare.local:{port}{}/    (mDNS)", "➜".green(), state.base);
    }
```

- [ ] **Step 3: README** — Usage gains `fshare --no-mdns          # skip fshare.local announcement`; feature list/Extras gains "Announces `http://fshare.local:8000` via mDNS (zero-config, `--no-mdns` to disable)". Roadmap: drop mDNS line.

- [ ] **Step 4: Verify** — `cargo test && cargo clippy --all-targets -- -D warnings`. Manual smoke:

```bash
./target/debug/fshare --port 18126 <tmpdir> &   # banner shows fshare.local line
dig +short @224.0.0.251 -p 5353 fshare.local A  # returns LAN IP(s)
curl -s http://fshare.local:18126/ | head -1     # resolves if system has mDNS resolution
```

(dig result is the authoritative check; curl may fail if nss-mdns absent — fine.)

- [ ] **Step 5: Commit** — `git commit -am "feat: --no-mdns flag, banner .local URL, docs"`

---

## Self-Review Notes

- Spec coverage: default-on + flag (T2), hostname+service+TXT path (T1), instance naming (T1), never-fatal (T2), banner line + QR unchanged (T2), ignored browse-back test (T1), README (T2).
- mdns-sd `ServiceInfo::new` signature varies across versions (ip param takes `&str`/`&[IpAddr]`/impl AsIpAddrs) — adapt per compiler.
