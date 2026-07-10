# fshare TLS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `--tls` serves HTTPS with a persisted self-signed cert (generated once via rcgen, reused forever), fingerprint shown in banner.

**Architecture:** New `src/tls.rs` owns cert lifecycle: `load_or_generate(dir, sans)` returns `TlsPaths` (paths + SHA-256 fingerprint + generated flag). `main.rs` branches: `--tls` → `axum_server::from_tcp_rustls`, else existing `axum::serve`. Scheme threaded into banner/QR.

**Tech Stack:** `axum-server` (tls-rustls), `rcgen`, `sha2`, `time` (rcgen validity dates). PEM→DER for fingerprint via existing `base64` crate (strip BEGIN/END lines).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-10-fshare-tls-design.md`.
- TLS setup failure FATAL — never silent fallback to plain HTTP.
- Cert reused when present, never rewritten. `key.pem` mode 0600.
- SANs: `fshare.local`, machine hostname, `localhost`, non-loopback ranked IPs. Validity 3650 days.
- Banner/QR/mDNS line use `https://` when `--tls`.
- rcgen/axum-server API drift: adapt per compiler; spec behavior authoritative.

---

### Task 1: tls.rs — cert lifecycle

**Files:**
- Create: `src/tls.rs`; Modify: `src/lib.rs` (`pub mod tls;`), `Cargo.toml`

**Interfaces:**
- Produces:
  - `tls::TlsPaths { cert: PathBuf, key: PathBuf, fingerprint: String, generated: bool }`
  - `tls::load_or_generate(dir: &Path, sans: &[String]) -> Result<TlsPaths, String>`
  - `tls::data_dir() -> PathBuf` — `$XDG_DATA_HOME/fshare` else `$HOME/.local/share/fshare`

- [ ] **Step 1: Deps**

```bash
cargo add rcgen sha2 time axum-server --features axum-server/tls-rustls
```

- [ ] **Step 2: Failing tests** in `src/tls.rs`:

```rust
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
        // key mode 0600
        let mode = std::fs::metadata(&a.key).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // cert PEM contains SAN host (rcgen writes DNS names into cert; check PEM parses)
        let pem = std::fs::read_to_string(&a.cert).unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"));
        // fingerprint format AA:BB:...
        assert_eq!(a.fingerprint.len(), 32 * 3 - 1);
        assert!(a.fingerprint.chars().all(|c| c.is_ascii_hexdigit() || c == ':'));
        // second call reuses: same fingerprint, generated == false, files untouched
        let before = std::fs::read(&a.cert).unwrap();
        let b = load_or_generate(t.path(), &sans).unwrap();
        assert!(!b.generated);
        assert_eq!(b.fingerprint, a.fingerprint);
        assert_eq!(std::fs::read(&b.cert).unwrap(), before);
    }
}
```

Run: `cargo test tls::` — FAIL.

- [ ] **Step 3: Implement**

```rust
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

    let mut params = rcgen::CertificateParams::new(sans.to_vec())
        .map_err(|e| e.to_string())?;
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
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .map_err(|_| format!("corrupt PEM in {}", cert_pem.display()))?;
    let hash = Sha256::digest(&der);
    Ok(hash
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}
```

Note: rcgen 0.13 `CertificateParams::new(Vec<String>)` auto-detects IP strings as IP SANs.

- [ ] **Step 4: Verify** — `cargo test tls::` PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat: persisted self-signed cert generation with fingerprint"`

---

### Task 2: CLI + HTTPS serving + banner scheme + integration test

**Files:**
- Modify: `src/cli.rs`, `src/main.rs`, `tests/http.rs`, `README.md`

**Interfaces:**
- Consumes: `tls::{load_or_generate, data_dir, TlsPaths}`.
- Produces: `Args.tls: bool`; banner takes `scheme: &str`.

- [ ] **Step 1: CLI** — after `no_mdns`:

```rust
    /// Serve HTTPS with a persisted self-signed certificate
    #[arg(long)]
    pub tls: bool,
```

- [ ] **Step 2: Failing integration test** (`tests/http.rs`):

```rust
#[tokio::test]
async fn tls_serves_https() {
    let t = fixture();
    let certdir = tempfile::tempdir().unwrap();
    let tls = fshare::tls::load_or_generate(certdir.path(), &["localhost".to_string()]).unwrap();
    let root = t.path().canonicalize().unwrap();
    let opts = fshare::server::ShareOpts {
        show_hidden: false,
        follow_links: false,
        zip: true,
        upload: false,
        max_upload: None,
    };
    let state = Arc::new(fshare::server::AppState::new(
        root, false, opts, false, fshare::log::Logger::spawn(false), None,
    ));
    let app = fshare::server::router(state);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
        .await
        .unwrap();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let c = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
    let r = c.get(format!("https://{addr}/hello.txt")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "hello world");

    // plain http against TLS port fails
    assert!(reqwest::get(format!("http://{addr}/")).await.is_err());
}
```

Add `axum-server` to `[dev-dependencies]`? Not needed — it's a normal dependency (Task 1 Step 1), test can use it. Run: FAIL until wiring compiles (should compile already after Task 1; test may pass immediately — fine, it pins behavior).

- [ ] **Step 3: main.rs wiring**

Scheme + TLS config before banner; replace serve branch:

```rust
    let scheme = if args.tls { "https" } else { "http" };

    let tls_config = if args.tls {
        let mut sans = vec![
            "fshare.local".to_string(),
            fshare::mdns::machine_hostname(),
            "localhost".to_string(),
        ];
        sans.extend(
            net::ranked_ifaces()
                .into_iter()
                .filter(|i| i.kind != net::IfaceKind::Loopback)
                .map(|i| i.ip.to_string()),
        );
        let paths = fshare::tls::load_or_generate(&fshare::tls::data_dir(), &sans)?;
        println!(
            "  {} TLS cert fingerprint SHA256: {}{}",
            "note:".yellow(),
            paths.fingerprint,
            if paths.generated { "  (newly generated)" } else { "" },
        );
        Some(
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&paths.cert, &paths.key)
                .await
                .map_err(|e| format!("TLS config: {e}"))?,
        )
    } else {
        None
    };
```

Serving select — replace current `let serve = axum::serve(...)` + select with:

```rust
    let make = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    let serve_plain;
    let serve_tls;
    match tls_config {
        Some(cfg) => {
            // axum-server takes the std listener directly
            serve_tls = Some(axum_server::from_tcp_rustls(listener_std, cfg).serve(make));
            serve_plain = None;
        }
        None => {
            let l = tokio::net::TcpListener::from_std(listener_std)?;
            serve_plain = Some(axum::serve(l, make));
            serve_tls = None;
        }
    }
```

Simpler pattern (avoid Option<future> gymnastics): extract shutdown future first, then:

```rust
    let shutdown = async {
        tokio::select! {
            reason = expire => println!("\n  {} — shutting down", reason.yellow()),
            _ = tokio::signal::ctrl_c() => println!(),
        }
    };

    if let Some(cfg) = tls_config {
        tokio::select! {
            r = axum_server::from_tcp_rustls(listener, cfg)
                .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>()) => r?,
            _ = shutdown => {}
        }
    } else {
        let l = tokio::net::TcpListener::from_std(listener)?;
        tokio::select! {
            r = axum::serve(l, app.into_make_service_with_connect_info::<std::net::SocketAddr>()) => r?,
            _ = shutdown => {}
        }
    }
```

(`listener` here is the std listener; move `tokio::net::TcpListener::from_std` into the else branch; `expire` future moved into `shutdown`.) NOTE: axum-server's `from_tcp_rustls` wants a blocking std listener — remove the unconditional `listener.set_nonblocking(true)` in `run()` and set it only in the plain branch right before `from_std` (from_std requires nonblocking). In TLS branch pass the std listener as-is.

Banner scheme: `print_banner` gains `scheme: &str` param; both URL formats change:

```rust
        println!("  {} {scheme}://fshare.local:{port}{}/    (mDNS)", "➜".green(), state.base);
        let url = format!("{scheme}://{host}:{port}{}/", state.base);
```

- [ ] **Step 4: README** — Usage gains `fshare --tls # HTTPS with persisted self-signed cert (fingerprint in banner)`; Security notes: replace plain-HTTP caveat tail with "or run `--tls`: self-signed cert persisted in `~/.local/share/fshare/` (delete to regenerate), fingerprint printed at startup so you can match the browser warning." Roadmap: drop TLS line.

- [ ] **Step 5: Verify** — `cargo test && cargo clippy --all-targets -- -D warnings`. Manual smoke:

```bash
./target/debug/fshare --tls --auth --port 18127 <tmpdir> &
curl -sk -u fshare:<pass> https://127.0.0.1:18127/ | head -1   # HTML
curl -s http://127.0.0.1:18127/ ; echo $?                       # connection error
```

- [ ] **Step 6: Commit** — `git commit -am "feat: --tls HTTPS serving with persisted cert, https banner"`

---

## Self-Review Notes

- Spec coverage: persist+reuse+0600+fingerprint (T1), SAN list incl. IPs (T2 wiring), fatal errors (`?` propagation in main — run() returns Err → exit 1) (T2), https banner/QR (scheme param drives both — QR uses `best_url` built from same format) (T2), integration https test + plain-fails (T2), README (T2).
- rcgen 0.13 API (`CertificateParams::new(Vec<String>)?`, `KeyPair::generate()`, `params.self_signed(&key_pair)`) — verify against installed version, adapt if 0.12 (`Certificate::from_params`).
- axum-server listener blocking-mode requirement called out explicitly (common footgun).
