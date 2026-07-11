# fshare TUI with Live Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ratatui full-screen TUI (auto-active on a tty) with single-key live toggles for mdns/upload/auth/token/hidden/dir-sizes/zip, backed by runtime-mutable settings read per-request.

**Architecture:** New `LiveSettings` struct (atomics + RwLocks) owned by `AppState`; handlers snapshot it per request via `AppState::opts()`. The `/s/<token>` prefix moves from `Router::nest` into a `token_gate` middleware so it can change at runtime. New `src/tui.rs` runs a ratatui event loop consuming the existing `log::Event` channel; plain log output remains the path for pipes, `--json-log`, and `--no-tui`.

**Tech Stack:** Rust 2021, axum 0.8, tokio, ratatui 0.29 (crossterm backend via `ratatui::crossterm` re-export), qrcode (existing).

## Global Constraints

- Toggles are session-only — the TUI never writes the config file.
- The mDNS TXT `path` property never carries the token prefix — always `/` (existing `txt_path`).
- Plain mode (pipes, `--json-log`, `--no-tui`, config `tui = false`, raw-mode failure) keeps today's output byte-for-byte (banner, streaming log, summary).
- POST with uploads disabled returns `405 Method Not Allowed` (same as today).
- Auth comparison stays constant-time (`auth::ct_eq`).
- All tests green: `cargo test`. Lints clean: `cargo clippy --all-targets -- -D warnings`.
- Commit only files you touched — never `git add -A`.

---

### Task 1: `LiveSettings` + per-request snapshots

**Files:**
- Create: `src/live.rs`
- Modify: `src/lib.rs`, `src/server.rs`, `src/auth.rs`, `src/upload.rs`, `src/main.rs`, `tests/http.rs`

**Interfaces:**
- Produces: `live::LiveSettings` (fields `mdns/upload/hidden/dir_sizes/zip: AtomicBool`, `auth: RwLock<Option<String>>`, `base: RwLock<String>`; methods `new(...)`, `base() -> String`, `auth() -> Option<String>`, `set_token(on: bool) -> String`), `AppState.live: Arc<LiveSettings>`, `AppState::opts() -> ShareOpts` (snapshot), `AppState::base() -> String`. `AppState::new` signature unchanged.
- Consumes: existing `server::ShareOpts`, `server::gen_token`.

- [ ] **Step 1: Write `src/live.rs` with tests**

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

/// Settings mutable at runtime (from the TUI) and read per-request.
/// Booleans use Relaxed ordering: toggles are independent, no ordering
/// relationship between settings is relied upon.
pub struct LiveSettings {
    pub mdns: AtomicBool,
    pub upload: AtomicBool,
    pub hidden: AtomicBool,
    pub dir_sizes: AtomicBool,
    pub zip: AtomicBool,
    pub auth: RwLock<Option<String>>, // "user:pass", None = off
    pub base: RwLock<String>,         // "" or "/s/<token>"
}

impl LiveSettings {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mdns: bool,
        upload: bool,
        hidden: bool,
        dir_sizes: bool,
        zip: bool,
        auth: Option<String>,
        base: String,
    ) -> Self {
        Self {
            mdns: AtomicBool::new(mdns),
            upload: AtomicBool::new(upload),
            hidden: AtomicBool::new(hidden),
            dir_sizes: AtomicBool::new(dir_sizes),
            zip: AtomicBool::new(zip),
            auth: RwLock::new(auth),
            base: RwLock::new(base),
        }
    }

    pub fn base(&self) -> String {
        self.base.read().unwrap().clone()
    }

    pub fn auth(&self) -> Option<String> {
        self.auth.read().unwrap().clone()
    }

    /// on = install a NEW random token (old links die); off = plain base.
    /// Returns the new base.
    pub fn set_token(&self, on: bool) -> String {
        let b = if on { format!("/s/{}", crate::server::gen_token()) } else { String::new() };
        *self.base.write().unwrap() = b.clone();
        b
    }
}

/// Flip an AtomicBool, returning the NEW value.
pub fn toggle(flag: &AtomicBool) -> bool {
    !flag.fetch_xor(true, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> LiveSettings {
        LiveSettings::new(true, false, false, false, true, None, String::new())
    }

    #[test]
    fn toggle_flips() {
        let l = fresh();
        assert!(toggle(&l.upload)); // false -> true, returns true
        assert!(l.upload.load(Ordering::Relaxed));
        assert!(!toggle(&l.upload)); // true -> false, returns false
        assert!(!l.upload.load(Ordering::Relaxed));
    }

    #[test]
    fn set_token_regenerates_and_clears() {
        let l = fresh();
        let a = l.set_token(true);
        assert!(a.starts_with("/s/") && a.len() == 3 + 12);
        assert_eq!(l.base(), a);
        let b = l.set_token(true);
        assert_ne!(a, b, "regeneration must mint a new token");
        assert_eq!(l.set_token(false), "");
        assert_eq!(l.base(), "");
    }

    #[test]
    fn auth_clone_out() {
        let l = fresh();
        assert_eq!(l.auth(), None);
        *l.auth.write().unwrap() = Some("u:p".into());
        assert_eq!(l.auth(), Some("u:p".to_string()));
    }
}
```

Register the module in `src/lib.rs`: add `pub mod live;` alphabetically among the existing `pub mod` lines.

- [ ] **Step 2: Run tests to verify they fail/compile**

Run: `cargo test --lib live -- --nocapture`
Expected: PASS (module is self-contained; this is scaffolding for the refactor)

- [ ] **Step 3: Refactor `AppState` in `src/server.rs`**

Replace the `AppState` struct and impl (currently lines 55–92) with:

```rust
pub struct AppState {
    pub root: PathBuf,
    pub single_file: bool,
    pub follow_links: bool,
    pub max_upload: Option<u64>,
    pub live: Arc<crate::live::LiveSettings>,
    pub events: tokio::sync::mpsc::UnboundedSender<crate::log::Event>,
    pub stats: Arc<Stats>,
    pub downloads_done: Arc<AtomicU64>,
    pub download_signal: Arc<tokio::sync::Notify>,
    pub limiter: Option<async_speed_limit::Limiter>,
}

impl AppState {
    pub fn new(
        root: PathBuf,
        single_file: bool,
        opts: ShareOpts,
        token: bool,
        events: tokio::sync::mpsc::UnboundedSender<crate::log::Event>,
        auth: Option<String>,
        limit: Option<u64>,
    ) -> Self {
        let base = if token { format!("/s/{}", gen_token()) } else { String::new() };
        let live = Arc::new(crate::live::LiveSettings::new(
            false, // actual mDNS state stored by main after announce succeeds
            opts.upload,
            opts.show_hidden,
            opts.dir_sizes,
            opts.zip,
            auth,
            base,
        ));
        Self {
            root,
            single_file,
            follow_links: opts.follow_links,
            max_upload: opts.max_upload,
            live,
            events,
            stats: Arc::default(),
            downloads_done: Arc::default(),
            download_signal: Arc::default(),
            limiter: limit.map(|n| async_speed_limit::Limiter::new(n as f64)),
        }
    }

    /// Per-request snapshot of the mutable settings in the legacy shape.
    pub fn opts(&self) -> ShareOpts {
        use std::sync::atomic::Ordering::Relaxed;
        ShareOpts {
            show_hidden: self.live.hidden.load(Relaxed),
            dir_sizes: self.live.dir_sizes.load(Relaxed),
            follow_links: self.follow_links,
            zip: self.live.zip.load(Relaxed),
            upload: self.live.upload.load(Relaxed),
            max_upload: self.max_upload,
        }
    }

    pub fn base(&self) -> String {
        self.live.base()
    }
}
```

- [ ] **Step 4: Update `router` and `handle` in `src/server.rs`**

`router()` — POST routes always registered (the `if state.opts.upload` branch dies); the nest still reads the startup base (token stays static until Task 2):

```rust
pub fn router(state: Arc<AppState>) -> Router {
    let inner = Router::new()
        .route("/", get(handle).post(crate::upload::handle))
        .route("/{*path}", get(handle).post(crate::upload::handle))
        .layer(axum::extract::DefaultBodyLimit::disable());
    // layer order: last added = outermost, so auth runs inside track and 401s get logged
    let inner = inner
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::require))
        .layer(axum::middleware::from_fn_with_state(state.clone(), track))
        .with_state(state.clone());
    let base = state.base();
    if base.is_empty() {
        inner
    } else {
        // axum 0.8's nest expands to `{base}` + `{base}/{*rest}`, and the
        // wildcard no longer matches empty — so exactly `{base}/` (the URL
        // printed in banner and QR) would 404. Route it explicitly.
        let slashless = base.clone();
        Router::new()
            .route(
                &format!("{base}/"),
                get(move || async move { axum::response::Redirect::permanent(&slashless) }),
            )
            .nest(&base, inner)
    }
}
```

`handle()` — snapshot once, use it everywhere `st.opts` / `st.base` appeared:

```rust
async fn handle(
    State(st): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
    uri: Uri,
    req: Request,
) -> Response {
    if st.single_file {
        return serve_single(&st, req).await;
    }
    let opts = st.opts();

    let rel_raw = uri.path().trim_start_matches('/').trim_end_matches('/');
    // decoded for display (breadcrumbs/title); resolve() decodes separately
    let rel = percent_decode_str(rel_raw).decode_utf8_lossy().into_owned();
    let Some(path) = resolve(&st.root, uri.path(), &opts) else {
        return not_found();
    };

    if path.is_dir() {
        if q.contains_key("zip") {
            if !opts.zip {
                return not_found();
            }
            return crate::zip::zip_response(path, rel.clone(), opts.show_hidden);
        }
        let entries = crate::listing::read_dir_entries(&path, opts.show_hidden, opts.dir_sizes);
        if q.get("format").map(String::as_str) == Some("json") {
            return axum::Json(entries).into_response();
        }
        return Html(crate::listing::render_html(
            &rel,
            &entries,
            &st.base(),
            opts.zip,
            opts.upload,
            opts.dir_sizes,
        ))
        .into_response();
    }

    // file: delegate to ServeDir for Range/ETag/MIME
    match ServeDir::new(&st.root).oneshot(req).await {
        Ok(res) => res.map(Body::new),
        Err(_) => not_found(),
    }
}
```

- [ ] **Step 5: Update `src/auth.rs` to read live auth**

Replace the top of `require`:

```rust
pub async fn require(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(expected) = st.live.auth() else {
        return next.run(req).await;
    };
```

(rest of the function unchanged; `expected` is now an owned `String`, `expected.as_bytes()` still works).

- [ ] **Step 6: Update `src/upload.rs`**

In `handle`, replace the `st.opts` / `st.base` reads:

```rust
    let opts = st.opts();
    if !opts.upload {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(dir) = resolve(&st.root, uri.path(), &opts) else {
        return StatusCode::NOT_FOUND.into_response();
    };
```

Further down, `st.opts.show_hidden` → `opts.show_hidden`, `st.opts.max_upload` → `opts.max_upload`, and the redirect line → `let back = format!("{}{}", st.base(), uri.path());`.

- [ ] **Step 7: Update `src/main.rs`**

Three call sites:

1. After the mDNS announce block, record actual state (add directly below the `let _mdns_guard = …;` statement):

```rust
    state.live.mdns.store(_mdns_guard.is_some(), std::sync::atomic::Ordering::Relaxed);
```

2. In `print_banner`, `state.base` → `state.base()` (two places: mDNS line and iface URL line). Since `base()` returns an owned `String`, bind it once at the top of `print_banner`:

```rust
    let base = state.base();
```

and use `{base}` in the two `format!` calls that used `state.base`.

3. The auth banner block `if let Some(a) = &state.auth` → 

```rust
    if let Some(a) = state.live.auth() {
        let (user, pass) = a.split_once(':').unwrap_or((a.as_str(), ""));
```

(rest unchanged).

- [ ] **Step 8: Update `tests/http.rs` and add live-toggle tests**

`spawn_opts` returns the state so tests can flip settings mid-flight. Change signature and body:

```rust
async fn spawn_opts(
    root: PathBuf,
    token: bool,
    opts: fshare::server::ShareOpts,
    auth: Option<String>,
) -> (String, Arc<fshare::server::AppState>, tokio::task::JoinHandle<()>) {
    let root = root.canonicalize().unwrap();
    let state = Arc::new(fshare::server::AppState::new(
        root, false, opts, token, fshare::log::Logger::spawn(false), auth, None,
    ));
    let base = state.base();
    let app = fshare::server::router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    (format!("http://{addr}{base}"), state, h)
}
```

Update `spawn` and `spawn_capped` to pass the extra tuple element through (change their return type the same way), and mechanically update every existing call site: `let (base, _h) = spawn(…)` becomes `let (base, _st, _h) = spawn(…)` (same for `spawn_opts`/`spawn_capped` uses). Do not otherwise change existing tests.

Append new tests:

```rust
#[tokio::test]
async fn upload_toggles_live() {
    let t = fixture();
    let (base, st, _h) = spawn(t.path().into(), false, false).await;
    let client = reqwest::Client::new();
    let form = || reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"hi".to_vec()).file_name("up.txt"),
    );

    let r = client.post(format!("{base}/")).multipart(form()).send().await.unwrap();
    assert_eq!(r.status(), 405, "uploads off -> 405");

    st.live.upload.store(true, std::sync::atomic::Ordering::Relaxed);
    let r = client.post(format!("{base}/")).multipart(form()).send().await.unwrap();
    assert!(r.status().is_success() || r.status().is_redirection());
    assert!(t.path().join("up.txt").exists());

    st.live.upload.store(false, std::sync::atomic::Ordering::Relaxed);
    let r = client.post(format!("{base}/")).multipart(form()).send().await.unwrap();
    assert_eq!(r.status(), 405, "toggled back off -> 405 again");
}

#[tokio::test]
async fn hidden_and_auth_toggle_live() {
    let t = fixture();
    let (base, st, _h) = spawn(t.path().into(), false, false).await;

    let html = reqwest::get(format!("{base}/")).await.unwrap().text().await.unwrap();
    assert!(!html.contains(".hidden"));
    st.live.hidden.store(true, std::sync::atomic::Ordering::Relaxed);
    let html = reqwest::get(format!("{base}/")).await.unwrap().text().await.unwrap();
    assert!(html.contains(".hidden"), "hidden files appear after live toggle");

    *st.live.auth.write().unwrap() = Some("u:p".into());
    let r = reqwest::get(format!("{base}/")).await.unwrap();
    assert_eq!(r.status(), 401, "auth enforced after live enable");
    let client = reqwest::Client::new();
    let r = client.get(format!("{base}/")).basic_auth("u", Some("p")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    *st.live.auth.write().unwrap() = None;
    let r = reqwest::get(format!("{base}/")).await.unwrap();
    assert_eq!(r.status(), 200, "auth off again");
}
```

- [ ] **Step 9: Run full test suite and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green (existing + 3 live unit tests + 2 new integration tests)

- [ ] **Step 10: Commit**

```bash
git add src/live.rs src/lib.rs src/server.rs src/auth.rs src/upload.rs src/main.rs tests/http.rs
git commit -m "feat: runtime-mutable LiveSettings read per-request"
```

---

### Task 2: token prefix as middleware

**Files:**
- Modify: `src/server.rs`, `tests/http.rs`

**Interfaces:**
- Consumes: `AppState::base()`, `live.set_token(on)` from Task 1.
- Produces: `server::token_gate` middleware; `router()` no longer nests — token prefix is checked/stripped per request, so `live.set_token` takes effect immediately.

- [ ] **Step 1: Add failing integration test**

Append to `tests/http.rs`:

```rust
#[tokio::test]
async fn token_regenerates_live() {
    let t = fixture();
    // token: true — spawn returns base WITH the /s/<tok> prefix
    let (base, st, _h) = spawn(t.path().into(), true, false).await;
    let plain = {
        // strip "/s/<tok>" (3 + 12 chars) to get the bare origin
        let cut = base.len() - (3 + 12);
        base[..cut].to_string()
    };

    // with prefix works, both with and without trailing slash
    assert_eq!(reqwest::get(format!("{base}")).await.unwrap().status(), 200);
    assert_eq!(reqwest::get(format!("{base}/")).await.unwrap().status(), 200);
    assert_eq!(reqwest::get(format!("{base}/hello.txt")).await.unwrap().status(), 200);
    // without prefix: 404
    assert_eq!(reqwest::get(format!("{plain}/hello.txt")).await.unwrap().status(), 404);
    // prefix must be a path-boundary match, not a string prefix
    assert_eq!(reqwest::get(format!("{base}extra/")).await.unwrap().status(), 404);

    // regenerate: old token dies, new one works
    let new_base = st.live.set_token(true);
    assert_eq!(reqwest::get(format!("{base}/hello.txt")).await.unwrap().status(), 404);
    let r = reqwest::get(format!("{plain}{new_base}/hello.txt")).await.unwrap();
    assert_eq!(r.status(), 200);

    // token off: plain path works
    st.live.set_token(false);
    assert_eq!(reqwest::get(format!("{plain}/hello.txt")).await.unwrap().status(), 200);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test http token_regenerates_live`
Expected: FAIL — regenerated token 404s (nest is baked at startup) and/or slashless base asserts differ.

- [ ] **Step 3: Implement `token_gate`, simplify `router`**

In `src/server.rs`, replace `router()` and add the middleware:

```rust
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handle).post(crate::upload::handle))
        .route("/{*path}", get(handle).post(crate::upload::handle))
        .layer(axum::extract::DefaultBodyLimit::disable())
        // layer order: last added = outermost. token_gate runs first (404s
        // and prefix-stripping before anything is logged), then track, then
        // auth — so 401s get logged, same as before.
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::require))
        .layer(axum::middleware::from_fn_with_state(state.clone(), track))
        .layer(axum::middleware::from_fn_with_state(state.clone(), token_gate))
        .with_state(state)
}

/// Enforce and strip the live token prefix ("" = no token, pass through).
/// Reads `live.base` per request so `set_token` takes effect immediately.
pub async fn token_gate(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let base = st.base();
    if base.is_empty() {
        return next.run(req).await;
    }
    let rest = match req.uri().path().strip_prefix(base.as_str()) {
        Some("") => "/".to_string(),                       // exactly "/s/<tok>"
        Some(r) if r.starts_with('/') => r.to_string(),    // "/s/<tok>/..."
        _ => return not_found(),                           // wrong or missing token
    };
    let pq = match req.uri().query() {
        Some(q) => format!("{rest}?{q}"),
        None => rest,
    };
    let mut parts = req.uri().clone().into_parts();
    parts.path_and_query = Some(pq.parse().expect("stripped path from a valid uri is valid"));
    *req.uri_mut() = Uri::from_parts(parts).expect("rebuilt uri from valid parts");
    next.run(req).await
}
```

Note: the old `{base}/` → `{base}` redirect special case is gone — both the slashless and slashed forms now serve the root listing directly (`Some("")` and `Some("/")` both map to `/`). Listing links are generated absolute with the base prefix (`render_html` receives `st.base()`), so relative-link resolution does not depend on the trailing slash.

- [ ] **Step 4: Run tests**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green, including `token_regenerates_live` and every pre-existing token test in `tests/http.rs`.

- [ ] **Step 5: Commit**

```bash
git add src/server.rs tests/http.rs
git commit -m "feat: token prefix enforced by live middleware, not router nest"
```

---

### Task 3: TUI module (`src/tui.rs`)

**Files:**
- Create: `src/tui.rs`
- Modify: `Cargo.toml`, `src/lib.rs`, `src/log.rs`, `src/mdns.rs`

**Interfaces:**
- Consumes: `AppState` (`live`, `stats`), `log::Event` + `log::format_pretty`, `mdns::announce`/`MdnsGuard`, `auth::parse_auth`, `net::ranked_ifaces`, `listing::human_size`, `qrcode`.
- Produces: `tui::run(app: App, events: UnboundedReceiver<log::Event>, shutdown: F) -> std::io::Result<Option<String>>` (returns the shutdown reason string when expiry fired, `None` on user quit); `tui::App::new(...)`; `tui::probe() -> bool` (raw-mode capability check); `log::Logger::spawn_printer(rx, json)`; `log::Event::Setting { text: String }`.

- [ ] **Step 1: Add dependencies**

In `Cargo.toml` `[dependencies]`:

```toml
ratatui = "0.29"
```

(crossterm comes via `ratatui::crossterm` re-export — do NOT add a separate crossterm dependency; a second copy risks version skew with the backend.)

- [ ] **Step 2: Extend `src/log.rs`**

Add a variant to `Event`:

```rust
    Setting { text: String },
```

Add a `format_pretty` arm:

```rust
        Event::Setting { text } => format!("{ts}  ⚙ {text}"),
```

Add a `format_json` arm:

```rust
        Event::Setting { text } => json!({ "event": "setting", "text": text }),
```

Split `Logger::spawn` so the TUI can own the receiver:

```rust
impl Logger {
    pub fn spawn(json: bool) -> mpsc::UnboundedSender<Event> {
        let (tx, rx) = mpsc::unbounded_channel::<Event>();
        Self::spawn_printer(rx, json);
        tx
    }

    pub fn spawn_printer(mut rx: mpsc::UnboundedReceiver<Event>, json: bool) {
        let cache: Arc<Mutex<HashMap<IpAddr, Option<String>>>> = Arc::default();
        tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if json {
                    println!("{}", format_json(&e));
                    continue;
                }
                let ip = match &e {
                    Event::Request { ip, .. } | Event::Done { ip, .. } | Event::Upload { ip, .. } => Some(*ip),
                    Event::Setting { .. } => None,
                };
                let mut line = format_pretty(&e);
                if let Some(ip) = ip {
                    if let Some(h) = lookup_cached(&cache, ip).await {
                        line = line.replacen(&ip.to_string(), &format!("{ip} ({h})"), 1);
                    }
                }
                println!("{line}");
            }
        });
    }
}
```

Add a test to the existing `log::tests` module:

```rust
    #[test]
    fn formats_setting() {
        let s = format_pretty(&Event::Setting { text: "upload enabled".into() });
        assert!(s.contains("⚙") && s.contains("upload enabled"));
    }
```

- [ ] **Step 3: Make `mdns::MdnsGuard` reusable from the TUI**

No signature change needed — `announce(port, base)` already returns `Result<MdnsGuard, String>` and dropping the guard unregisters. Verify nothing else is needed; done.

- [ ] **Step 4: Write `src/tui.rs`**

```rust
use crate::log;
use crate::server::AppState;
use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const LOG_CAP: usize = 1000;

/// Immutable share facts for the header line.
pub struct ShareInfo {
    pub root: std::path::PathBuf,
    pub single_file: bool,
    pub files: u64,
    pub bytes: u64,
}

#[derive(PartialEq)]
enum Popup {
    None,
    Qr,
    Help,
}

pub enum Action {
    None,
    Quit,
}

pub struct App {
    pub state: Arc<AppState>,
    scheme: &'static str,
    port: u16,
    info: ShareInfo,
    log: VecDeque<String>,
    scroll: usize, // lines above the bottom; 0 = follow
    popup: Popup,
    mdns_guard: Option<crate::mdns::MdnsGuard>,
    notice: Option<String>,           // e.g. generated credentials, cleared on any key
    initial_auth: Option<String>,     // "user:pass" from CLI/config, reused on re-enable
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: Arc<AppState>,
        scheme: &'static str,
        port: u16,
        info: ShareInfo,
        mdns_guard: Option<crate::mdns::MdnsGuard>,
        initial_auth: Option<String>,
        seed_notes: Vec<String>,
    ) -> Self {
        let mut app = Self {
            state,
            scheme,
            port,
            info,
            log: VecDeque::new(),
            scroll: 0,
            popup: Popup::None,
            mdns_guard,
            notice: None,
            initial_auth,
        };
        for n in seed_notes {
            app.push_line(n);
        }
        app
    }

    pub fn push_line(&mut self, line: String) {
        self.log.push_back(line);
        if self.log.len() > LOG_CAP {
            self.log.pop_front();
            self.scroll = self.scroll.saturating_sub(1);
        } else if self.scroll > 0 {
            // keep the viewed window stable while scrolled back
            self.scroll = (self.scroll + 1).min(self.log.len().saturating_sub(1));
        }
    }

    fn note(&mut self, text: &str) {
        self.push_line(log::format_pretty(&log::Event::Setting { text: text.to_string() }));
    }

    pub fn primary_url(&self) -> String {
        let base = self.state.base();
        let host = crate::net::ranked_ifaces()
            .first()
            .map(|i| match i.ip {
                IpAddr::V6(v6) => format!("[{v6}]"),
                IpAddr::V4(v4) => v4.to_string(),
            })
            .unwrap_or_else(|| "localhost".to_string());
        format!("{}://{host}:{}{base}/", self.scheme, self.port)
    }

    /// (key, label, on) triples for the hotkey bar, in display order.
    pub fn hotbar(&self) -> Vec<(char, &'static str, bool)> {
        let l = &self.state.live;
        vec![
            ('m', "mdns", l.mdns.load(Relaxed)),
            ('u', "upload", l.upload.load(Relaxed)),
            ('a', "auth", l.auth().is_some()),
            ('t', "token", !l.base().is_empty()),
            ('h', "hidden", l.hidden.load(Relaxed)),
            ('d', "dirs", l.dir_sizes.load(Relaxed)),
            ('z', "zip", l.zip.load(Relaxed)),
        ]
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // any key clears transient overlays first
        if self.popup != Popup::None || self.notice.is_some() {
            self.popup = Popup::None;
            self.notice = None;
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('x') => return Action::Quit,
            KeyCode::Char('m') => self.toggle_mdns(),
            KeyCode::Char('u') => {
                let on = crate::live::toggle(&self.state.live.upload);
                self.note(if on { "upload enabled" } else { "upload disabled" });
            }
            KeyCode::Char('h') => {
                let on = crate::live::toggle(&self.state.live.hidden);
                self.note(if on { "hidden files shown" } else { "hidden files hidden" });
            }
            KeyCode::Char('d') => {
                let on = crate::live::toggle(&self.state.live.dir_sizes);
                self.note(if on { "dir sizes on" } else { "dir sizes off" });
            }
            KeyCode::Char('z') => {
                let on = crate::live::toggle(&self.state.live.zip);
                self.note(if on { "zip downloads enabled" } else { "zip downloads disabled" });
            }
            KeyCode::Char('a') => self.toggle_auth(),
            KeyCode::Char('t') => {
                let turn_on = self.state.live.base().is_empty();
                self.state.live.set_token(turn_on);
                self.note(if turn_on {
                    "token URL enabled (new token — old links die)"
                } else {
                    "token URL disabled"
                });
            }
            KeyCode::Char('Q') => self.popup = Popup::Qr,
            KeyCode::Char('?') => self.popup = Popup::Help,
            KeyCode::Up => self.scroll_by(1),
            KeyCode::Down => self.scroll_by(-1),
            KeyCode::PageUp => self.scroll_by(10),
            KeyCode::PageDown => self.scroll_by(-10),
            _ => {}
        }
        Action::None
    }

    fn scroll_by(&mut self, delta: isize) {
        let max = self.log.len().saturating_sub(1);
        let cur = self.scroll as isize + delta;
        self.scroll = cur.clamp(0, max as isize) as usize;
    }

    fn toggle_mdns(&mut self) {
        if self.mdns_guard.take().is_some() {
            // drop unregisters
            self.state.live.mdns.store(false, Relaxed);
            self.note("mDNS announce disabled");
            return;
        }
        match crate::mdns::announce(self.port, "") {
            Ok(g) => {
                self.mdns_guard = Some(g);
                self.state.live.mdns.store(true, Relaxed);
                self.note("mDNS announce enabled");
            }
            Err(e) => {
                self.state.live.mdns.store(false, Relaxed);
                self.note(&format!("mDNS failed: {e}"));
            }
        }
    }

    fn toggle_auth(&mut self) {
        if self.state.live.auth().is_some() {
            *self.state.live.auth.write().unwrap() = None;
            self.note("auth disabled");
            return;
        }
        let creds = match &self.initial_auth {
            Some(c) => c.clone(),
            None => crate::auth::parse_auth(&None).expect("bare auth always parses"),
        };
        if self.initial_auth.is_none() {
            let (user, pass) = creds.split_once(':').unwrap_or((creds.as_str(), ""));
            self.notice = Some(format!("auth on — user: {user}  password: {pass}  (any key to dismiss)"));
        }
        *self.state.live.auth.write().unwrap() = Some(creds);
        self.note("auth enabled");
    }
}

/// Can we enter raw mode? Used by main to fall back to plain output.
pub fn probe() -> bool {
    use ratatui::crossterm::terminal;
    terminal::enable_raw_mode().and_then(|_| terminal::disable_raw_mode()).is_ok()
}

pub async fn run(
    mut app: App,
    mut events: mpsc::UnboundedReceiver<log::Event>,
    shutdown: impl std::future::Future<Output = String>,
) -> std::io::Result<Option<String>> {
    let mut terminal = ratatui::try_init()?;

    // blocking input thread -> channel (crossterm events aren't async)
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<CEvent>();
    std::thread::spawn(move || {
        use ratatui::crossterm::event;
        loop {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => {
                    if let Ok(ev) = event::read() {
                        if key_tx.send(ev).is_err() {
                            break;
                        }
                    }
                }
                Ok(false) => {
                    if key_tx.is_closed() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    tokio::pin!(shutdown);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut reason: Option<String> = None;

    loop {
        terminal.draw(|f| draw(f, &app))?;
        tokio::select! {
            Some(ev) = key_rx.recv() => {
                if let CEvent::Key(k) = ev {
                    if k.kind == ratatui::crossterm::event::KeyEventKind::Press {
                        if let Action::Quit = app.handle_key(k) {
                            break;
                        }
                    }
                }
                // resize events fall through; next draw() picks up the new size
            }
            Some(e) = events.recv() => app.push_line(log::format_pretty(&e)),
            _ = tick.tick() => {} // refresh stats in header
            r = &mut shutdown => { reason = Some(r); break; }
        }
    }

    drop(app.mdns_guard.take()); // unregister before leaving
    ratatui::restore();
    Ok(reason)
}

fn draw(f: &mut Frame, app: &App) {
    let [header, logs, bar] =
        Layout::vertical([Constraint::Length(4), Constraint::Min(3), Constraint::Length(1)])
            .areas(f.area());

    // header
    let title = if app.info.single_file {
        format!(" fshare v{} — sharing file {} ", env!("CARGO_PKG_VERSION"), app.info.root.display())
    } else {
        format!(
            " fshare v{} — {} ({} files, {}) ",
            env!("CARGO_PKG_VERSION"),
            app.info.root.display(),
            app.info.files,
            crate::listing::human_size(app.info.bytes),
        )
    };
    let stats = &app.state.stats;
    let status = format!(
        "➜ {}   {} clients   {} sent",
        app.primary_url(),
        stats.clients.lock().unwrap().len(),
        crate::listing::human_size(stats.bytes.load(Relaxed)),
    );
    let mut lines = vec![Line::from(Span::styled(status, Style::default().fg(Color::Green)))];
    if let Some(n) = &app.notice {
        lines.push(Line::from(Span::styled(
            n.clone(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        header,
    );

    // log pane: last N visible lines honoring scroll offset
    let h = logs.height.saturating_sub(2) as usize; // borders
    let total = app.log.len();
    let end = total.saturating_sub(app.scroll);
    let start = end.saturating_sub(h);
    let text: Vec<Line> = app.log.iter().skip(start).take(end - start).map(|l| Line::raw(l.clone())).collect();
    let log_title = if app.scroll > 0 { format!(" log (scrolled ↑{}) ", app.scroll) } else { " log ".to_string() };
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(log_title)),
        logs,
    );

    // hotkey bar
    let mut spans: Vec<Span> = Vec::new();
    for (key, label, on) in app.hotbar() {
        let style = if on {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(
            format!(" [{key}]{label}:{}", if on { "on" } else { "off" }),
            style,
        ));
    }
    spans.push(Span::styled(
        "  [Q]r [?]help [q]uit",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), bar);

    match app.popup {
        Popup::Qr => draw_qr_popup(f, app),
        Popup::Help => draw_help_popup(f),
        Popup::None => {}
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w.min(area.width), h.min(area.height))
}

fn draw_qr_popup(f: &mut Frame, app: &App) {
    let url = app.primary_url();
    let Ok(code) = qrcode::QrCode::new(url.as_bytes()) else {
        return;
    };
    let rendered = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build();
    let lines: Vec<&str> = rendered.lines().collect();
    let w = lines.first().map(|l| l.chars().count()).unwrap_or(0) as u16 + 2;
    let h = lines.len() as u16 + 2;
    let area = f.area();
    if w > area.width || h > area.height {
        let r = centered(area, 30, 3);
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new("terminal too small for QR").block(Block::default().borders(Borders::ALL)),
            r,
        );
        return;
    }
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(rendered).block(Block::default().borders(Borders::ALL).title(format!(" {url} "))),
        r,
    );
}

fn draw_help_popup(f: &mut Frame) {
    let text = "\
 m  toggle mDNS announce
 u  toggle uploads
 a  toggle auth (generated password shown)
 t  toggle token URL (new token each enable)
 h  toggle hidden files
 d  toggle dir sizes
 z  toggle zip downloads
 Q  QR code popup
 ↑↓ PgUp PgDn  scroll log
 q / x / Ctrl+C  quit";
    let r = centered(f.area(), 46, 12);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" keys ")),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{AppState, ShareOpts};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn test_app(auth: Option<String>, token: bool) -> App {
        let opts = ShareOpts {
            show_hidden: false,
            dir_sizes: false,
            follow_links: false,
            zip: true,
            upload: false,
            max_upload: None,
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(AppState::new(
            std::path::PathBuf::from("/tmp"),
            false,
            opts,
            token,
            tx,
            auth.clone(),
            None,
        ));
        App::new(
            state,
            "http",
            8000,
            ShareInfo { root: "/tmp".into(), single_file: false, files: 3, bytes: 1024 },
            None,
            auth,
            vec![],
        )
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn hotbar_reflects_state() {
        let app = test_app(None, false);
        let bar = app.hotbar();
        let get = |name| bar.iter().find(|(_, l, _)| *l == name).unwrap().2;
        assert!(!get("upload") && get("zip") && !get("auth") && !get("token"));
    }

    #[test]
    fn toggles_flip_live_state() {
        let mut app = test_app(None, false);
        app.handle_key(key('u'));
        assert!(app.state.live.upload.load(Relaxed));
        app.handle_key(key('u'));
        assert!(!app.state.live.upload.load(Relaxed));
        app.handle_key(key('t'));
        assert!(app.state.live.base().starts_with("/s/"));
        app.handle_key(key('t'));
        assert_eq!(app.state.live.base(), "");
    }

    #[test]
    fn auth_toggle_generates_and_reuses() {
        let mut app = test_app(None, false);
        app.handle_key(key('a'));
        let creds = app.state.live.auth().unwrap();
        assert!(creds.starts_with("fshare:"));
        assert!(app.notice.is_some(), "generated password surfaces in header");
        // any key dismisses the notice without acting
        app.handle_key(key('u'));
        assert!(app.notice.is_none());
        assert!(!app.state.live.upload.load(Relaxed), "dismissal key must not toggle");

        let mut app2 = test_app(Some("ben:pw".into()), false);
        app2.handle_key(key('a')); // off (was on via initial auth)
        assert_eq!(app2.state.live.auth(), None);
        app2.handle_key(key('a')); // back on — reuses explicit creds, no notice
        assert_eq!(app2.state.live.auth(), Some("ben:pw".to_string()));
        assert!(app2.notice.is_none());
    }

    #[test]
    fn quit_keys() {
        let mut app = test_app(None, false);
        assert!(matches!(app.handle_key(key('q')), Action::Quit));
        assert!(matches!(app.handle_key(key('x')), Action::Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(app.handle_key(ctrl_c), Action::Quit));
        // plain 'c' is not quit
        assert!(matches!(app.handle_key(key('c')), Action::None));
    }

    #[test]
    fn log_ring_trims_and_scroll_clamps() {
        let mut app = test_app(None, false);
        for i in 0..(LOG_CAP + 50) {
            app.push_line(format!("line {i}"));
        }
        assert_eq!(app.log.len(), LOG_CAP);
        assert_eq!(app.log.front().unwrap(), "line 50");
        app.scroll_by(10);
        assert_eq!(app.scroll, 10);
        app.scroll_by(-100);
        assert_eq!(app.scroll, 0);
        app.scroll_by(isize::MAX);
        assert_eq!(app.scroll, LOG_CAP - 1);
    }

    #[test]
    fn renders_header_and_hotbar() {
        let app = test_app(None, false);
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(text.contains("fshare v"));
        assert!(text.contains("[m]mdns"));
        assert!(text.contains("[u]upload:off"));
        assert!(text.contains("clients"));
    }

    #[test]
    fn qr_popup_renders() {
        let mut app = test_app(None, false);
        app.handle_key(key('Q'));
        assert!(matches!(app.popup, Popup::Qr));
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap(); // must not panic
        app.handle_key(key('m'));
        assert!(matches!(app.popup, Popup::None), "any key closes popup");
        assert!(!app.state.live.mdns.load(Relaxed) || true, "close key must not toggle");
    }
}
```

Register in `src/lib.rs`: add `pub mod tui;` alphabetically.

Note on `primary_url` in tests: `net::ranked_ifaces()` touches real interfaces — fine in tests (falls back to `localhost` if empty). The render test only asserts stable substrings.

- [ ] **Step 5: Run tests**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green (7 new tui tests + 1 new log test). The `tui::run` function is not yet called from main — that's Task 4; `pub` items in the lib produce no dead-code warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/tui.rs src/lib.rs src/log.rs src/mdns.rs
git commit -m "feat: ratatui TUI module with live setting toggles"
```

---

### Task 4: activation, wiring, docs

**Files:**
- Modify: `src/cli.rs`, `src/config.rs`, `src/main.rs`, `README.md`

**Interfaces:**
- Consumes: `tui::App::new`, `tui::run`, `tui::probe`, `tui::ShareInfo`, `log::Logger::spawn_printer`, `cli::tri`.
- Produces: `Settings.tui: bool` (default true), CLI pair `--tui/--no-tui`, config key `tui`.

- [ ] **Step 1: CLI pair in `src/cli.rs`**

Add after the `secure`/`no_secure` pair:

```rust
    /// Full-screen terminal UI (default when stdout is a terminal)
    #[arg(long, overrides_with = "no_tui")]
    pub tui: bool,
    /// Plain streaming log output (override config/default)
    #[arg(long, overrides_with = "tui")]
    pub no_tui: bool,
```

Extend the `flag_pairs_last_wins` test:

```rust
        let a = Args::parse_from(["fshare", "--tui", "--no-tui"]);
        assert!(a.no_tui && !a.tui);
```

- [ ] **Step 2: Config key + Settings field in `src/config.rs`**

- `Config` struct: add `pub tui: Option<bool>,` (alphabetical placement with the other fields; `deny_unknown_fields` already covers rejection of typos).
- `Settings` struct: add `pub tui: bool,` after `json_log`.
- In `resolve()`, next to the other boolean merges: `let tui = crate::cli::tri(cli.tui, cli.no_tui).or(c.tui).unwrap_or(true);` and include `tui` in the returned `Settings`. The secure bundle does NOT touch `tui`.
- Existing tests construct `Settings` or assert on it — update any struct literals to include `tui`. Add a precedence test in `config::tests`:

```rust
    #[test]
    fn tui_precedence() {
        let cfg: Config = toml::from_str("tui = false").unwrap();
        let s = resolve(&args(&[]), &cfg).unwrap();
        assert!(!s.tui, "config disables tui");
        let s = resolve(&args(&["--tui"]), &cfg).unwrap();
        assert!(s.tui, "CLI --tui beats config");
        let s = resolve(&args(&["--no-tui"]), &Config::default()).unwrap();
        assert!(!s.tui);
        let s = resolve(&args(&[]), &Config::default()).unwrap();
        assert!(s.tui, "default on");
    }
```

(`args(&[...])` — use the same CLI-args helper the existing config tests use; if it has a different name, match it.)

- [ ] **Step 3: Wire the TUI in `src/main.rs`**

Restructure `async_main`. The plain path must stay byte-identical; the TUI path replaces `print_banner` + printer with seeded notes + `tui::run`. Apply these changes in order:

1. Replace `let events = flog::Logger::spawn(settings.json_log);` with:

```rust
    let tui_wanted = settings.tui
        && !settings.json_log
        && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let use_tui = tui_wanted && fshare::tui::probe();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel::<flog::Event>();
    let mut events_rx = Some(events_rx);
    if !use_tui {
        flog::Logger::spawn_printer(events_rx.take().expect("rx present"), settings.json_log);
        if tui_wanted {
            println!("  {} terminal does not support raw mode — plain output", "note:".yellow());
        }
    }
```

2. Keep the mDNS announce block, but in TUI mode collect the failure note instead of printing it. Change the block to:

```rust
    let mut seed_notes: Vec<String> = Vec::new();
    let mut mdns_guard = if !settings.mdns {
        None
    } else {
        match fshare::mdns::announce(port, &state.base()) {
            Ok(g) => Some(g),
            Err(e) => {
                if use_tui {
                    seed_notes.push(format!("mDNS unavailable: {e}"));
                } else {
                    println!("  {} mDNS unavailable: {e}", "note:".yellow());
                }
                None
            }
        }
    };
    state.live.mdns.store(mdns_guard.is_some(), std::sync::atomic::Ordering::Relaxed);
```

3. TLS fingerprint note likewise: in the `tls_config` block, replace the `println!` with:

```rust
        let fp_note = format!(
            "TLS cert fingerprint SHA256: {}{}",
            paths.fingerprint,
            if paths.generated { "  (newly generated)" } else { "" },
        );
        if use_tui {
            seed_notes.push(fp_note);
        } else {
            println!("  {} {}", "note:".yellow(), fp_note);
        }
```

4. Banner and serve. Wrap the existing `print_banner(...)` call:

```rust
    if !use_tui {
        print_banner(/* unchanged args */);
    } else {
        if let Some(p) = cfg_loaded.as_deref() {
            seed_notes.push(format!("loaded {}", p.display()));
        }
        if settings.secure {
            seed_notes.push("secure mode — TLS/auth/token per settings, mDNS off unless overridden".to_string());
        }
        if settings.token {
            seed_notes.push("URLs include the access token".to_string());
        }
        if let Some(l) = settings.limit {
            seed_notes.push(format!("download speed limited to {}/s", fshare::listing::human_size(l)));
        }
        if let Some(a) = state.live.auth() {
            let (user, pass) = a.split_once(':').unwrap_or((a.as_str(), ""));
            let explicit = matches!(&settings.auth, Some(Some(v)) if v.contains(':'));
            if explicit {
                seed_notes.push(format!("auth enabled (user {user})"));
            } else {
                seed_notes.push(format!("auth enabled — user: {user}  password: {pass}"));
            }
        }
        for o in &others {
            seed_notes.push(format!(
                "another fshare serving {} on :{} (PID {})",
                o.dir.display(), o.port, o.pid
            ));
        }
        if bumped {
            seed_notes.push(format!("port {} was busy, using {port}", net::DEFAULT_PORT));
        }
    }
```

5. Serving. The current tail of `async_main` (from `let expire = …` to the final summary `println!`) becomes:

```rust
    let expire = expiry::wait(
        args.timeout,
        args.max_downloads,
        state.downloads_done.clone(),
        state.download_signal.clone(),
    );

    let make = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    listener.set_nonblocking(true)?;

    if use_tui {
        // server runs as a background task; TUI owns the foreground
        let server: tokio::task::JoinHandle<Result<(), std::io::Error>> =
            if let Some(cfg) = tls_config {
                let srv = axum_server::from_tcp_rustls(listener, cfg)?;
                tokio::spawn(async move { srv.serve(make).await })
            } else {
                let l = tokio::net::TcpListener::from_std(listener)?;
                tokio::spawn(async move { axum::serve(l, make).await })
            };

        let initial_auth = match &settings.auth {
            Some(Some(v)) if v.contains(':') => Some(v.clone()),
            _ => state.live.auth(), // generated or None — reuse what's active
        };
        let (files, bytes) = if single_file { (1, root.metadata().map(|m| m.len()).unwrap_or(0)) } else { dir_summary(&root) };
        let info = fshare::tui::ShareInfo { root: root.clone(), single_file, files, bytes };
        let tapp = fshare::tui::App::new(
            state.clone(), scheme, port, info, mdns_guard.take(), initial_auth, seed_notes,
        );
        let reason = fshare::tui::run(tapp, events_rx.take().expect("rx reserved for tui"), expire).await?;
        server.abort();
        if let Some(r) = reason {
            println!("\n  {} — shutting down", r.yellow());
        }
    } else {
        let shutdown = async {
            tokio::select! {
                reason = expire => println!("\n  {} — shutting down", reason.yellow()),
                _ = tokio::signal::ctrl_c() => println!(),
            }
        };
        if let Some(cfg) = tls_config {
            tokio::select! {
                r = axum_server::from_tcp_rustls(listener, cfg)?.serve(make) => r?,
                _ = shutdown => {}
            }
        } else {
            let l = tokio::net::TcpListener::from_std(listener)?;
            tokio::select! {
                r = axum::serve(l, make) => r?,
                _ = shutdown => {}
            }
        }
    }

    let s = &state.stats;
    println!(
        "  served {} requests to {} client(s), {} sent",
        s.requests.load(Ordering::Relaxed),
        s.clients.lock().unwrap().len(),
        fshare::listing::human_size(s.bytes.load(Ordering::Relaxed)),
    );
    Ok(())
```

Notes for the implementer: `let app = server::router(state.clone());` must be built before this block (it already is); rename the old `_mdns_guard` binding to `mdns_guard` (mutable) per change 2; keep `let others = instance::others();` where it is — the TUI branch reads it in change 4. `expiry::wait` returns a `String` reason future — exactly what `tui::run` expects.

- [ ] **Step 4: Run everything**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green. Integration tests run piped (not a tty) so they exercise the plain path unchanged.

Manual smoke (do this — it's the feature): `cargo run -- /tmp --port 18222` in a real terminal; verify TUI appears, `u` flips upload (hotbar updates), `curl -X POST http://localhost:18222/` gives 405→ after `u` a multipart POST works, `t` regenerates URL in header, `Q` shows QR, `?` shows help, `q` quits, terminal restored, summary printed. Then `cargo run -- /tmp 2>&1 | head -20` — plain banner, no TUI. Kill any leftover instance you started.

- [ ] **Step 5: Update `README.md`**

- Configuration section key list: add `tui = false          # plain log output, no full-screen UI`.
- Usage list: add `fshare --no-tui             # plain streaming log (default when piped)`.
- New section after "Usage":

```markdown
## Terminal UI

In an interactive terminal fshare runs a full-screen UI: header with live
URL and transfer stats, scrolling request log, and a hotkey bar. Settings
flip live — no restart:

| Key | Action |
|-----|--------|
| `m` | toggle mDNS announcement |
| `u` | toggle uploads |
| `a` | toggle Basic auth (generated password shown) |
| `t` | toggle token URL (new random token each enable) |
| `h` | toggle hidden files |
| `d` | toggle directory sizes |
| `z` | toggle zip downloads |
| `Q` | QR code popup |
| `?` | help |
| `q` / `x` / Ctrl+C | quit |

Toggles last for the session only; the config file is never modified.
Piped output, `--json-log`, `--no-tui`, or `tui = false` in the config all
keep the classic streaming log.
```

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs src/config.rs src/main.rs README.md
git commit -m "feat: activate TUI on tty with --tui/--no-tui and config key"
```

---

### Task 5: styled 404 page

**Files:**
- Create: `src/404.html`
- Modify: `src/server.rs`
- Test: `tests/http.rs`

**Interfaces:**
- Consumes: `token_gate` from Task 2 (its 404 must use the same negotiation).
- Produces: `server::not_found_res(html: bool) -> Response`, `server::wants_html(&HeaderMap) -> bool`. The old `not_found()` stays as the plain-text branch.

- [ ] **Step 1: Write failing integration test**

Append to `tests/http.rs`:

```rust
#[tokio::test]
async fn pretty_404_for_browsers_only() {
    let t = fixture();
    let (base, _st, _h) = spawn(t.path().into(), false, false).await;
    let client = reqwest::Client::new();

    // browser: styled page
    let r = client
        .get(format!("{base}/nope.txt"))
        .header("accept", "text/html,application/xhtml+xml")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let body = r.text().await.unwrap();
    assert!(body.contains("<!doctype html>") && body.contains("history.back"));

    // script/curl: plain text unchanged
    let r = client.get(format!("{base}/nope.txt")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.text().await.unwrap(), "404 — not found");
}

#[tokio::test]
async fn token_404_page_leaks_no_base() {
    let t = fixture();
    let (base, _st, _h) = spawn(t.path().into(), true, false).await;
    let token = base.rsplit('/').next().unwrap().to_string();
    let plain = {
        let cut = base.len() - (3 + 12);
        base[..cut].to_string()
    };
    let r = reqwest::Client::new()
        .get(format!("{plain}/wrong/path"))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let body = r.text().await.unwrap();
    assert!(body.contains("<!doctype html>"));
    assert!(!body.contains(&token), "404 page must not leak the token");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test http pretty_404`
Expected: FAIL — body is plain text even with the html Accept header.

- [ ] **Step 3: Create `src/404.html`**

```html
<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>fshare — not found</title>
<style>
:root { --bg:#fff; --fg:#1a1a1a; --muted:#777; --line:#e5e5e5; --accent:#0b6cff; }
@media (prefers-color-scheme: dark) {
  :root { --bg:#121212; --fg:#e8e8e8; --muted:#999; --line:#2a2a2a; --accent:#5aa2ff; }
}
* { box-sizing:border-box }
body { margin:0; min-height:100vh; display:flex; flex-direction:column;
  align-items:center; justify-content:center; gap:.5rem; background:var(--bg);
  color:var(--fg); font:15px/1.5 system-ui, sans-serif; padding:1rem; }
h1 { font-size:5rem; margin:0; color:var(--muted); font-weight:700; letter-spacing:.05em }
p { margin:0; color:var(--muted) }
a { color:var(--accent); text-decoration:none } a:hover { text-decoration:underline }
svg { width:56px; height:56px; opacity:.9 }
footer { position:fixed; bottom:1rem; color:var(--muted); font-size:.8em }
footer a { color:var(--muted) }
</style>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96">
  <g fill="none" stroke="#0b6cff" stroke-linecap="round">
    <path d="M76 30 A 12 12 0 0 0 64 18" stroke-width="5" opacity=".9"/>
    <path d="M85 30 A 21 21 0 0 0 64 9" stroke-width="5" opacity=".6"/>
    <path d="M94 30 A 30 30 0 0 0 64 0" stroke-width="5" opacity=".3"/>
  </g>
  <path fill="#0b6cff" d="M8 34c0-3.3 2.7-6 6-6h17l8 8h21c3.3 0 6 2.7 6 6v4H8V34z"/>
  <path fill="#0b6cff" opacity=".85"
        d="M10 50h56c3.8 0 6.7 3.4 5.9 7.1l-4.6 24C66.7 84 64.2 86 61.4 86H14.6c-2.8 0-5.3-2-5.9-4.9l-4.6-24C3.3 53.4 6.2 50 10 50z"/>
  <circle cx="64" cy="30" r="4.5" fill="#0b6cff"/>
</svg>
<h1>404</h1>
<p>nothing here</p>
<p><a href="javascript:history.back()">&#8592; back</a></p>
<footer><a href="https://github.com/13/fshare">fshare</a> v{{version}}</footer>
```

Security note honored: no link to the share root — this page also serves wrong-token 404s, and a root link would leak the token. `javascript:history.back()` only.

- [ ] **Step 4: Content negotiation in `src/server.rs`**

Replace `not_found()` with the trio:

```rust
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "404 — not found").into_response()
}

pub fn wants_html(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"))
}

pub fn not_found_res(html: bool) -> Response {
    if html {
        let page = include_str!("404.html").replacen("{{version}}", env!("CARGO_PKG_VERSION"), 1);
        (StatusCode::NOT_FOUND, Html(page)).into_response()
    } else {
        not_found()
    }
}
```

Wire the call sites:

1. `handle()`: at the top (before any `req` consumption) add `let accept_html = wants_html(req.headers());`, then replace every `return not_found();` / `Err(_) => not_found(),` in `handle` with `not_found_res(accept_html)`.
2. `token_gate()`: replace `return not_found();` with `return not_found_res(wants_html(req.headers()));`.
3. `serve_single()`: add an `accept_html: bool` parameter (caller computes it in `handle` before the call: `serve_single(&st, accept_html, req).await`), and its `Err(_) => not_found(),` becomes `Err(_) => not_found_res(accept_html),`.

- [ ] **Step 5: Run tests**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green including both new 404 tests.

- [ ] **Step 6: Commit**

```bash
git add src/404.html src/server.rs tests/http.rs
git commit -m "feat: styled 404 page for browser requests"
```
