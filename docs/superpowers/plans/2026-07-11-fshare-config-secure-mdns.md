# fshare Config File, Secure Mode, Per-Machine mDNS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** TOML config file for persistent defaults, `--secure` bundle flag for public networks, per-machine mDNS hostname `fshare-<hostname>.local`.

**Architecture:** New `src/config.rs` holds serde `Config` (all-`Option` mirror of allowed keys), `load()`, and a pure `resolve(cli, cfg) -> Settings` merge that also expands the `--secure` bundle. `cli::Args` booleans gain positive/negative flag pairs (clap `overrides_with`) so the CLI can flip config values either way; a `tri()` helper turns each pair into `Option<bool>`. `main.rs` consumes `Settings` instead of raw flags. `mdns.rs` gains `sanitize_hostname` + `host_label()`; `tls.rs` persists the SAN list and regenerates the cert when a requested SAN is missing.

**Tech Stack:** serde (derive), toml, existing clap 4 / mdns-sd / rcgen.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-11-fshare-config-secure-mdns-design.md`.
- Config location: `$XDG_CONFIG_HOME/fshare/config.toml`, fallback `~/.config/fshare/config.toml`; `FSHARE_CONFIG=<path>` overrides. Missing file = defaults, no error.
- Allowed config keys ONLY: `port, bind, hidden, follow_links, dir_sizes, qr, zip, upload, max_upload_size, auth, tls, limit, mdns, json_log, secure`. Unknown key or malformed TOML = hard error naming the file.
- Precedence: CLI > config > built-in default. `--secure` bundle fills only settings nobody set explicitly.
- `--limit 0` = unlimited (overrides config limit).
- mDNS host: `fshare-<sanitized-hostname>.local`; sanitize = lowercase, non `[a-z0-9-]` → `-`, collapse runs, trim `-`, empty → `host`.
- Existing flag spellings keep working; only inverses are new.
- `cargo clippy --all-targets -- -D warnings` clean; all existing tests keep passing.

---

### Task 1: CLI flag pairs + `--limit 0`

**Files:**
- Modify: `src/cli.rs`

**Interfaces:**
- Produces: `Args` with new fields `mdns, zip, qr, no_hidden, no_follow_links, no_upload, no_tls, no_dir_sizes, no_json_log, no_auth, secure, no_secure` (all `bool`), plus `pub bind: Option<IpAddr>` (default removed). `pub fn tri(pos: bool, neg: bool) -> Option<bool>`. `parse_limit` accepts `0`.

- [ ] **Step 1: Failing tests** — append to `src/cli.rs` tests module:

```rust
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
```

- [ ] **Step 2:** `cargo test --lib cli` — FAIL (fields/`tri` missing).

- [ ] **Step 3: Implement.** In `src/cli.rs`:

`parse_limit` becomes:

```rust
fn parse_limit(s: &str) -> Result<u64, String> {
    parse_size(s) // 0 = unlimited (overrides a config limit)
}
```

Update `--limit` doc comment to: `/// Cap total download speed, e.g. --limit 5M (bytes/second, all clients; 0 = unlimited)`.

Change `bind`:

```rust
    /// Address to bind (default 0.0.0.0)
    #[arg(long)]
    pub bind: Option<IpAddr>,
```

Wire pairs — replace/annotate the boolean fields so each names its inverse:

```rust
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
```

Add helper (top level, after `Args`):

```rust
/// Positive/negative flag pair to tri-state: set on / set off / absent.
pub fn tri(pos: bool, neg: bool) -> Option<bool> {
    match (pos, neg) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}
```

`src/main.rs` won't compile yet (`args.bind` now `Option`); temporary shim in this task: in `run()` replace `net::bind_port(args.bind, args.port)` with `net::bind_port(args.bind.unwrap_or_else(|| "0.0.0.0".parse().unwrap()), args.port)` (Task 3 replaces it properly).

- [ ] **Step 4:** `cargo test` all green, `cargo clippy --all-targets -- -D warnings`.
- [ ] **Step 5:** `git add -A && git commit -m "feat(cli): positive/negative flag pairs, tri-state helper, --limit 0"`

---

### Task 2: config module — load + resolve + secure bundle

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs` (add `pub mod config;`), `Cargo.toml`

**Interfaces:**
- Consumes: `cli::Args` fields + `cli::tri` + `cli::parse_size` (Task 1).
- Produces:
  - `config::Config` (serde struct, all `Option`), `config::AuthCfg`
  - `config::default_path() -> Option<PathBuf>` — honors `FSHARE_CONFIG` then XDG
  - `config::load(path: &Path) -> Result<Option<Config>, String>` — `None` when file absent
  - `config::resolve(a: &cli::Args, c: &Config) -> Result<Settings, String>`
  - `config::Settings { port: Option<u16>, bind: IpAddr, hidden: bool, follow_links: bool, dir_sizes: bool, qr: bool, zip: bool, upload: bool, max_upload_size: Option<u64>, auth: Option<Option<String>>, tls: bool, limit: Option<u64>, mdns: bool, json_log: bool, token: bool, secure: bool }`

- [ ] **Step 1:** `cargo add serde --features derive && cargo add toml`

- [ ] **Step 2: Write `src/config.rs` with failing tests:**

```rust
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
        assert_eq!(load(Path::new("/nonexistent/fshare.toml")).unwrap().is_none(), true);
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
```

- [ ] **Step 3:** Add `pub mod config;` to `src/lib.rs`. `cargo test --lib config` — compile errors first, then green once code above is complete (tests + impl land together here since the module is new; the failing state is Step 2 without `src/lib.rs` registration).
- [ ] **Step 4:** `cargo test` all green, `cargo clippy --all-targets -- -D warnings`.
- [ ] **Step 5:** `git add -A && git commit -m "feat: config file loading and CLI/config/default resolution with --secure bundle"`

---

### Task 3: wire Settings into main + banner notes + README

**Files:**
- Modify: `src/main.rs`, `README.md`

**Interfaces:**
- Consumes: `config::{default_path, load, resolve, Settings}` (Task 2).

- [ ] **Step 1: `run()`** — load config before binding (replace Task 1's shim):

```rust
fn run(args: cli::Args) -> Result<(), Box<dyn std::error::Error>> {
    let root = args
        .path
        .canonicalize()
        .map_err(|e| format!("cannot share '{}': {e}", args.path.display()))?;
    let single_file = root.is_file();
    if !single_file && !root.is_dir() {
        return Err(format!("'{}' is neither file nor directory", root.display()).into());
    }

    let cfg_path = fshare::config::default_path();
    let (cfg, cfg_loaded) = match &cfg_path {
        Some(p) => match fshare::config::load(p)? {
            Some(c) => (c, Some(p.clone())),
            None => (fshare::config::Config::default(), None),
        },
        None => (fshare::config::Config::default(), None),
    };
    let settings = fshare::config::resolve(&args, &cfg)?;

    let (listener, port, bumped) = net::bind_port(settings.bind, settings.port).map_err(|e| {
        format!(
            "cannot bind port {}: {e} (try --port <N>)",
            settings.port.unwrap_or(net::DEFAULT_PORT)
        )
    })?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(args, settings, cfg_loaded, root, single_file, listener, port, bumped))
}
```

- [ ] **Step 2: `async_main`** — new signature `async fn async_main(args: cli::Args, settings: fshare::config::Settings, cfg_loaded: Option<std::path::PathBuf>, root: PathBuf, single_file: bool, listener: std::net::TcpListener, port: u16, bumped: bool)`. Replace field uses:
  - `opts`: `show_hidden: settings.hidden, dir_sizes: settings.dir_sizes, follow_links: settings.follow_links, zip: settings.zip && !single_file, upload: settings.upload && !single_file, max_upload: settings.max_upload_size`
  - `auth`: `let auth = match &settings.auth { Some(v) => Some(fshare::auth::parse_auth(v)?), None => None };`
  - `events`: `flog::Logger::spawn(settings.json_log)`
  - `AppState::new(..., args.token || settings.token, events, auth, settings.limit)` → use `settings.token` (resolve already folds `a.token` in): `AppState::new(root.clone(), single_file, opts, settings.token, events, auth, settings.limit)`
  - mDNS guard condition: `if !settings.mdns { None } else { match ... } }`
  - `let scheme = if settings.tls { "https" } else { "http" };` and `if settings.tls {` for the cert branch
  - `print_banner(&settings, cfg_loaded.as_deref(), &state, port, bumped, &others, single_file, &root, _mdns_guard.is_some(), scheme)` — `args` no longer passed
- [ ] **Step 3: `print_banner`** — signature `fn print_banner(settings: &fshare::config::Settings, cfg_loaded: Option<&std::path::Path>, state: &server::AppState, port: u16, bumped: bool, others: &[instance::Instance], single_file: bool, root: &std::path::Path, mdns_on: bool, scheme: &str)`. Changes inside:
  - `let show_qr = settings.qr && std::io::IsTerminal::is_terminal(&std::io::stdout());`
  - notes section — replace `args.token`/`args.limit`/`args.auth` uses:

```rust
    if let Some(p) = cfg_loaded {
        println!("  {} loaded {}", "note:".yellow(), p.display());
    }
    if settings.secure {
        println!(
            "  {} secure mode — TLS {}, auth {}, token URL, mDNS {}",
            "note:".yellow(),
            if settings.tls { "on" } else { "off (overridden)" },
            if settings.auth.is_some() { "on" } else { "off (overridden)" },
            if mdns_on { "on (overridden)" } else { "off" },
        );
    }
    if settings.token {
        println!("  {} URLs above include the access token", "note:".yellow());
    }
    if let Some(l) = settings.limit {
        println!(
            "  {} download speed limited to {}/s",
            "note:".yellow(),
            fshare::listing::human_size(l)
        );
    }
    if let Some(a) = &state.auth {
        let (user, pass) = a.split_once(':').unwrap_or((a.as_str(), ""));
        let explicit = matches!(&settings.auth, Some(Some(v)) if v.contains(':'));
        if explicit {
            println!("  {} auth enabled (user {user})", "note:".yellow());
        } else {
            println!(
                "  {} auth enabled — user: {user}  password: {pass}",
                "note:".yellow()
            );
        }
    }
```

- [ ] **Step 4:** `cargo test` green, `cargo clippy --all-targets -- -D warnings`, then smoke:

```bash
cargo build
FSHARE_CONFIG=/tmp/claude-1000/-home-ben-repo-fshare/f5eb5616-e44a-4a35-9543-ef7fd216f9ab/scratchpad/fshare.toml sh -c '
  printf "mdns = false\nupload = true\n" > "$FSHARE_CONFIG"
  script -qec "stty cols 200; ./target/debug/fshare --port 18150 /tmp & sleep 1; kill %1" /dev/null | head -30'
```

Expected: `note: loaded ...fshare.toml`, no mDNS line, uploads active. Then `--secure` run shows secure note + https + token URL. Also `FSHARE_CONFIG=/nonexistent ./target/debug/fshare --secure ...` works without config.

- [ ] **Step 5: README** — add after the install section:

```markdown
## Configuration

Persistent defaults live in `~/.config/fshare/config.toml` (or
`$XDG_CONFIG_HOME/fshare/config.toml`; `FSHARE_CONFIG=<path>` overrides):

```toml
port = 9000
mdns = false          # don't announce on the network
upload = true
limit = "5MB"         # total download bandwidth
auth = "ben:secret"   # or `auth = true` for a generated password
tls = true
```

CLI flags always win — every boolean has an inverse (`--mdns/--no-mdns`,
`--tls/--no-tls`, …), and `--limit 0` lifts a configured limit.
Per-share options (`--token`, `--timeout`, `--max-downloads`) are CLI-only.

## Sharing on a public network

```sh
fshare --secure
```

One flag enables TLS, HTTP Basic auth with a generated password, a random
token URL, and turns mDNS announcement off. Anything you set explicitly
(e.g. `--auth bob:pw` or `mdns = true` in the config) wins over the bundle.
```

- [ ] **Step 6:** `git add -A && git commit -m "feat: wire config/secure settings into server startup and banner"`

---

### Task 4: per-machine mDNS hostname + TLS SAN regeneration

**Files:**
- Modify: `src/mdns.rs`, `src/tls.rs`, `src/main.rs`

**Interfaces:**
- Produces: `mdns::sanitize_hostname(raw: &str) -> String`, `mdns::host_label() -> String` (e.g. `fshare-benpc`), `mdns::mdns_host() -> String` (label + `.local.`). `tls::load_or_generate` unchanged signature, now SAN-aware.

- [ ] **Step 1: Failing tests** — `src/mdns.rs` tests:

```rust
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
```

`src/tls.rs` test:

```rust
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
```

- [ ] **Step 2:** `cargo test --lib` — FAIL (`sanitize_hostname` missing; SAN test fails on reuse-despite-new-SAN).

- [ ] **Step 3: Implement `src/mdns.rs`** — replace `pub const MDNS_HOST` with:

```rust
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
```

In `announce()` replace `MDNS_HOST` argument with `&mdns_host()`.

- [ ] **Step 4: Implement `src/tls.rs`** — `load_or_generate` reuses only when stored SANs cover the request:

```rust
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

    let mut params = rcgen::CertificateParams::new(sans.to_vec()).map_err(|e| e.to_string())?;
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3650);
    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let certificate = params.self_signed(&key_pair).map_err(|e| e.to_string())?;

    std::fs::write(&cert, certificate.pem()).map_err(|e| e.to_string())?;
    std::fs::write(&key, key_pair.serialize_pem()).map_err(|e| e.to_string())?;
    std::fs::write(&sans_file, sans.join("\n")).map_err(|e| e.to_string())?;
    let mut perms = std::fs::metadata(&key).map_err(|e| e.to_string())?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o600);
    std::fs::set_permissions(&key, perms).map_err(|e| e.to_string())?;

    let fp = fingerprint_of(&cert)?;
    Ok(TlsPaths { cert, key, fingerprint: fp, generated: true })
}
```

(Existing `generates_and_reuses` test keeps passing: first call writes `sans.txt`, second is covered.)

- [ ] **Step 5: `src/main.rs`** — banner mDNS line uses the label:

```rust
    if mdns_on {
        let host = fshare::mdns::host_label();
        addr_lines.push((
            format!("➜ {scheme}://{host}.local:{port}{}/    (mDNS)", state.base),
            format!("{} {scheme}://{host}.local:{port}{}/    (mDNS)", "➜".green(), state.base),
        ));
    }
```

TLS SAN list: replace `"fshare.local".to_string()` with `format!("{}.local", fshare::mdns::host_label())`.

- [ ] **Step 6:** `cargo test` green, `cargo clippy --all-targets -- -D warnings`. Smoke: run `./target/debug/fshare /tmp` on a pty, banner shows `fshare-<host>.local`; `curl -sI http://fshare-$(cat /etc/hostname).local:8000/` from same machine resolves (avahi permitting). Manual ignored test if multicast available: `cargo test mdns_browse_back -- --ignored`.
- [ ] **Step 7:** `git add -A && git commit -m "feat: per-machine mDNS hostname fshare-<host>.local, regenerate TLS cert on SAN change"`

---

## Self-Review Notes

- Spec coverage: config keys/precedence/negation pairs (T1+T2), FSHARE_CONFIG + missing-file + hard errors (T2), `--limit 0` (T1/T2), secure bundle + explicit-wins + banner note + README (T2/T3), banner "loaded config" note (T3), hostname sanitize + rename + banner/QR (T4), TLS SAN persistence + regeneration (T4). Integration test isolation: existing tests build AppState in-process, never parse CLI or read config — unaffected.
- Type consistency: `Settings` fields in T3 match T2 definition; `tri` from T1 used in T2; `host_label` in T4 main matches mdns.rs.
- ignored `mdns_browse_back` still matches: instance name unchanged ("fshare on <host>").
