# fshare `--upload` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Opt-in drag-and-drop uploads: `POST multipart/form-data` to any browsed directory, streamed to disk with atomic rename, collision auto-rename, size cap, progress UI.

**Architecture:** New `src/upload.rs` holds the axum handler plus pure helpers (`sanitize_name`, `unique_path`); `ShareOpts` gains `upload`/`max_upload`; the POST route is registered only when `--upload` is set. Each multipart part streams to a `.fshare-upload-<rand>` temp file guarded by a Drop cleanup, then atomically renames to its final name. Listing template gains a dropzone + XHR progress JS block when uploads are on.

**Tech Stack:** axum 0.8 `Multipart` extractor (feature `multipart`), existing tokio/tower-http stack.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-10-fshare-upload-design.md`.
- Without `--upload` no write route is registered; POST then returns 405 (axum's method-not-allowed for GET-only routes).
- Temp file lives in the destination directory (same filesystem → atomic `rename`).
- Filename policy: final path component only; NUL stripped; empty/`.`/`..` rejected; dot-leading rejected unless `--hidden`.
- Collisions: `photo.jpg` → `photo (1).jpg` → `photo (2).jpg`; extensionless names get suffix at end.
- Over-cap → 413, partial temp removed. Client abort → temp removed. Uploads never count toward `--max-downloads`.
- axum's default 2 MB body limit MUST be lifted on the router (`DefaultBodyLimit::disable()`) — our streaming cap replaces it.
- Existing tests keep passing; TDD; commit per task.

---

### Task 1: CLI flags + size parser

**Files:**
- Modify: `src/cli.rs`

**Interfaces:**
- Produces: `Args.upload: bool`, `Args.max_upload_size: Option<u64>`, `cli::parse_size(&str) -> Result<u64, String>` accepting `500`, `500K`, `2M`, `3G`, `1GB`, case-insensitive.

- [ ] **Step 1: Failing test** (append inside `mod tests` in `src/cli.rs`):

```rust
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
```

Run: `cargo test parses_sizes` — FAIL (undefined).

- [ ] **Step 2: Implement** — add to `src/cli.rs`:

```rust
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
```

And two new fields on `Args` (after `follow_links`):

```rust
    /// Enable uploads (drag & drop on the listing page)
    #[arg(long)]
    pub upload: bool,

    /// Reject uploads larger than this, e.g. 500M, 2G (default unlimited)
    #[arg(long, value_parser = parse_size)]
    pub max_upload_size: Option<u64>,
```

- [ ] **Step 3: Verify** — `cargo test parses_sizes` PASS; `cargo test` all green.
- [ ] **Step 4: Commit** — `git commit -am "feat: --upload and --max-upload-size flags with size parser"`

---

### Task 2: upload.rs pure helpers

**Files:**
- Create: `src/upload.rs`; Modify: `src/lib.rs` (add `pub mod upload;`)

**Interfaces:**
- Produces:
  - `upload::sanitize_name(raw: &str, allow_hidden: bool) -> Option<String>`
  - `upload::unique_path(dir: &Path, name: &str) -> PathBuf` — first non-existing candidate

- [ ] **Step 1: Failing tests** in `src/upload.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("photo.jpg", false).unwrap(), "photo.jpg");
        assert_eq!(sanitize_name("../../etc/passwd", false).unwrap(), "passwd");
        assert_eq!(sanitize_name(r"C:\evil\x.exe", false).unwrap(), "x.exe");
        assert_eq!(sanitize_name("a\0b.txt", false).unwrap(), "ab.txt");
        assert!(sanitize_name("", false).is_none());
        assert!(sanitize_name("..", false).is_none());
        assert!(sanitize_name(".bashrc", false).is_none());
        assert_eq!(sanitize_name(".bashrc", true).unwrap(), ".bashrc");
    }

    #[test]
    fn unique_paths() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(t.path(), "a.txt"), t.path().join("a.txt"));
        std::fs::write(t.path().join("a.txt"), "x").unwrap();
        assert_eq!(unique_path(t.path(), "a.txt"), t.path().join("a (1).txt"));
        std::fs::write(t.path().join("a (1).txt"), "x").unwrap();
        assert_eq!(unique_path(t.path(), "a.txt"), t.path().join("a (2).txt"));
        std::fs::write(t.path().join("noext"), "x").unwrap();
        assert_eq!(unique_path(t.path(), "noext"), t.path().join("noext (1)"));
    }
}
```

Run: `cargo test upload::` — FAIL.

- [ ] **Step 2: Implement**

```rust
use std::path::{Path, PathBuf};

pub fn sanitize_name(raw: &str, allow_hidden: bool) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next()?;
    let name: String = last.chars().filter(|c| *c != '\0').collect();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if !allow_hidden && name.starts_with('.') {
        return None;
    }
    Some(name.to_string())
}

pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1u32..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|c| !c.exists())
        .expect("finite collisions")
}
```

- [ ] **Step 3: Verify** — `cargo test upload::` PASS.
- [ ] **Step 4: Commit** — `git commit -am "feat: upload filename sanitization and collision renaming"`

---

### Task 3: upload handler + router wiring + log event

**Files:**
- Modify: `src/upload.rs`, `src/server.rs`, `src/log.rs`, `src/main.rs`, `Cargo.toml`, `tests/http.rs`

**Interfaces:**
- Consumes: `server::{AppState, ShareOpts, resolve}`, `upload::{sanitize_name, unique_path}`, `server::gen_token`.
- Produces:
  - `ShareOpts` gains `pub upload: bool, pub max_upload: Option<u64>` — ALL existing `ShareOpts { … }` literals (server tests, tests/http.rs, main.rs) gain `upload: false, max_upload: None` (main.rs from args).
  - `upload::handle` axum handler for `POST /` and `POST /{*path}`.
  - `log::Event::Upload { ip: IpAddr, name: String, bytes: u64, secs: f64 }`, pretty `⬆ <name> received <size> in <s>s`, JSON `"event":"upload"`.

- [ ] **Step 1: Cargo + ShareOpts groundwork**

`Cargo.toml`: axum gets multipart — `axum = { version = "0.8", features = ["multipart"] }`.

`server.rs` `ShareOpts`:

```rust
#[derive(Clone, Debug)]
pub struct ShareOpts {
    pub show_hidden: bool,
    pub follow_links: bool,
    pub zip: bool,
    pub upload: bool,
    pub max_upload: Option<u64>,
}
```

Fix all struct literals: server.rs unit tests + tests/http.rs `spawn` (parametrize: `fn spawn(root, token, upload)` passing `upload` into opts) + `counts_completed_downloads`. main.rs:

```rust
    let opts = server::ShareOpts {
        show_hidden: args.hidden,
        follow_links: args.follow_links,
        zip: !args.no_zip && !single_file,
        upload: args.upload && !single_file,
        max_upload: args.max_upload_size,
    };
```

`cargo test` green before proceeding (behavior unchanged).

- [ ] **Step 2: Failing integration tests** (append to `tests/http.rs`; update `spawn` signature everywhere):

```rust
#[tokio::test]
async fn upload_roundtrip_and_collision() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false, true).await;
    let c = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(b"fresh content".to_vec())
        .file_name("up.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c.post(format!("{base}/sub/"))
        .header("Accept", "application/json")
        .multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["saved"][0], "up.txt");
    assert_eq!(std::fs::read_to_string(t.path().join("sub/up.txt")).unwrap(), "fresh content");

    // collision → (1)
    let part = reqwest::multipart::Part::bytes(b"second".to_vec()).file_name("up.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let v: serde_json::Value = c.post(format!("{base}/sub/"))
        .header("Accept", "application/json")
        .multipart(form).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["saved"][0], "up (1).txt");
    assert_eq!(std::fs::read_to_string(t.path().join("sub/up (1).txt")).unwrap(), "second");

    // traversal filename neutralized
    let part = reqwest::multipart::Part::bytes(b"evil".to_vec())
        .file_name("../../escape.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let v: serde_json::Value = c.post(format!("{base}/"))
        .header("Accept", "application/json")
        .multipart(form).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["saved"][0], "escape.txt");
    assert!(t.path().join("escape.txt").exists());

    // no temp files left anywhere
    let leftovers: Vec<_> = walkdir_all(t.path())
        .into_iter()
        .filter(|n| n.contains(".fshare-upload-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn upload_disabled_and_cap() {
    let t = fixture();
    // disabled → 405
    let (base, _h) = spawn(t.path().into(), false, false).await;
    let c = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(b"x".to_vec()).file_name("a.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c.post(format!("{base}/")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 405);

    // cap → 413, no temp left
    let (base, _h) = spawn_capped(t.path().into(), 10).await;
    let part = reqwest::multipart::Part::bytes(vec![0u8; 1000]).file_name("big.bin");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c.post(format!("{base}/")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 413);
    assert!(!t.path().join("big.bin").exists());
    assert!(walkdir_all(t.path()).iter().all(|n| !n.contains(".fshare-upload-")));
}

fn walkdir_all(root: &std::path::Path) -> Vec<String> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(root).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() {
            v.extend(walkdir_all(&e.path()));
        }
        v.push(n);
    }
    v
}
```

Helper `spawn_capped(root, cap)` = same as `spawn` but `upload: true, max_upload: Some(cap)`. Refactor `spawn` to take an `opts: ShareOpts` internally if cleaner — keep test call sites as written above.

Run: `cargo test --test http` — FAIL (405 everywhere / compile error).

- [ ] **Step 3: log event** — `src/log.rs`:

Add variant:

```rust
    Upload { ip: IpAddr, name: String, bytes: u64, secs: f64 },
```

`format_pretty` arm:

```rust
        Event::Upload { ip, name, bytes, secs } => {
            format!("{ts}  {ip:15}  {} {name} received  {} in {secs:.0}s",
                "⬆".cyan(), human_size(*bytes))
        }
```

`format_json` arm:

```rust
        Event::Upload { ip, name, bytes, secs } => json!({
            "event": "upload", "ip": ip, "name": name, "bytes": bytes, "seconds": secs
        }),
```

Logger ip-match arm gains `| Event::Upload { ip, .. }`.
Unit test in `log.rs` tests module:

```rust
    #[test]
    fn formats_upload() {
        let ip = "192.168.1.23".parse().unwrap();
        let u = format_pretty(&Event::Upload {
            ip, name: "photo.jpg".into(), bytes: 4 * 1024 * 1024, secs: 3.0,
        });
        assert!(u.contains("⬆") && u.contains("photo.jpg") && u.contains("4.0 MB"));
    }
```

- [ ] **Step 4: handler** — append to `src/upload.rs`:

```rust
use crate::server::{resolve, AppState};
use axum::extract::multipart::Multipart;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncWriteExt;

/// Deletes the temp file on drop unless `persist()` was called.
struct TempGuard {
    path: Option<PathBuf>,
}

impl TempGuard {
    fn persist(&mut self) -> PathBuf {
        self.path.take().expect("persist called once")
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub async fn handle(
    State(st): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    uri: Uri,
    headers: HeaderMap,
    mut mp: Multipart,
) -> Response {
    if !st.opts.upload {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(dir) = resolve(&st.root, uri.path(), &st.opts) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !dir.is_dir() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let start = Instant::now();
    let mut saved: Vec<String> = Vec::new();

    loop {
        let field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        if field.name() != Some("file") {
            continue;
        }
        let raw = field.file_name().unwrap_or_default().to_string();
        let Some(name) = crate::upload::sanitize_name(&raw, st.opts.show_hidden) else {
            return (StatusCode::BAD_REQUEST, format!("bad filename '{raw}'")).into_response();
        };

        match save_part(field, &dir, &name, st.opts.max_upload).await {
            Ok((final_name, bytes)) => {
                let _ = st.events.send(crate::log::Event::Upload {
                    ip: addr.ip(),
                    name: final_name.clone(),
                    bytes,
                    secs: start.elapsed().as_secs_f64(),
                });
                st.stats.bytes.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
                saved.push(final_name);
            }
            Err(e) => return e,
        }
    }

    if wants_json(&headers) {
        axum::Json(serde_json::json!({ "saved": saved })).into_response()
    } else {
        let back = format!("{}{}", st.base, uri.path());
        Redirect::to(&back).into_response()
    }
}

async fn save_part(
    mut field: axum::extract::multipart::Field<'_>,
    dir: &Path,
    name: &str,
    cap: Option<u64>,
) -> Result<(String, u64), Response> {
    let tmp = dir.join(format!(".fshare-upload-{}", crate::server::gen_token()));
    let mut guard = TempGuard { path: Some(tmp.clone()) };
    let mut f = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| io_response(&e))?;
    let mut written: u64 = 0;
    loop {
        let chunk = match field.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(_) => return Err(StatusCode::BAD_REQUEST.into_response()),
        };
        written += chunk.len() as u64;
        if cap.is_some_and(|c| written > c) {
            return Err((StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds size limit").into_response());
        }
        f.write_all(&chunk).await.map_err(|e| io_response(&e))?;
    }
    f.flush().await.map_err(|e| io_response(&e))?;
    drop(f);
    let dest = crate::upload::unique_path(dir, name);
    tokio::fs::rename(guard.persist(), &dest)
        .await
        .map_err(|e| io_response(&e))?;
    Ok((dest.file_name().unwrap().to_string_lossy().into_owned(), written))
}

fn io_response(e: &std::io::Error) -> Response {
    if e.raw_os_error() == Some(28) {
        // ENOSPC
        (StatusCode::INSUFFICIENT_STORAGE, "disk full").into_response()
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
    }
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("application/json"))
        .unwrap_or(false)
}
```

Note: `guard.persist()` returns the temp path AND disarms deletion; on any early return before that, `TempGuard::drop` removes the temp. On rename failure the temp leaks disarmed — acceptable? NO: call `persist` only after successful rename is impossible (rename consumes path). Handle by re-arming on failure:

```rust
    let tmp_path = guard.persist();
    if let Err(e) = tokio::fs::rename(&tmp_path, &dest).await {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(io_response(&e));
    }
```

Use this variant in the implementation (replaces the plain `rename` call above).

- [ ] **Step 5: router wiring** — `src/server.rs` `router()`:

```rust
use axum::routing::post;
use tower_http::limit::RequestBodyLimitLayer; // NOT used; shown to say: do not add

pub fn router(state: Arc<AppState>) -> Router {
    let mut inner = Router::new()
        .route("/", get(handle))
        .route("/{*path}", get(handle));
    if state.opts.upload {
        inner = inner
            .route("/", post(crate::upload::handle))
            .route("/{*path}", post(crate::upload::handle))
            .layer(axum::extract::DefaultBodyLimit::disable());
    }
    let inner = inner
        .layer(axum::middleware::from_fn_with_state(state.clone(), track))
        .with_state(state.clone());
    if state.base.is_empty() {
        inner
    } else {
        Router::new().nest(&state.base, inner)
    }
}
```

(Do not add `RequestBodyLimitLayer`; the streaming cap in `save_part` is the limit.) axum 0.8: `.route("/", get(h).post(h2))` merging also fine if separate `.route` calls conflict — if `route` panics on duplicate path, use:

```rust
    let inner = if state.opts.upload {
        Router::new()
            .route("/", get(handle).post(crate::upload::handle))
            .route("/{*path}", get(handle).post(crate::upload::handle))
            .layer(axum::extract::DefaultBodyLimit::disable())
    } else {
        Router::new()
            .route("/", get(handle))
            .route("/{*path}", get(handle))
    };
```

Use whichever compiles; the merged `get(...).post(...)` form is canonical.

- [ ] **Step 6: Verify** — `cargo test` all PASS (incl. new upload tests).
- [ ] **Step 7: Commit** — `git commit -am "feat: streamed multipart uploads with atomic rename, cap, temp cleanup"`

---

### Task 4: dropzone UI

**Files:**
- Modify: `src/listing.rs`, `src/listing.html`, `src/server.rs` (call site), `tests/http.rs`

**Interfaces:**
- `listing::render_html(rel_path: &str, entries: &[Entry], base: &str, zip: bool, upload: bool) -> String` — new trailing `upload` param; caller in `server.rs` passes `st.opts.upload`.

- [ ] **Step 1: Failing test** (in `src/listing.rs` tests):

```rust
    #[test]
    fn upload_ui_gated() {
        let html = render_html("", &[], "", false, true);
        assert!(html.contains("dropzone") && html.contains("XMLHttpRequest"));
        let none = render_html("", &[], "", false, false);
        assert!(!none.contains("dropzone"));
    }
```

Update existing `render_html` test call sites with `, false` — run `cargo test listing::` — FAIL.

- [ ] **Step 2: template** — in `src/listing.html`, add `{{upload}}` placeholder after `</header>`:

```html
</header>
{{upload}}
<table id="t">
```

And in `src/listing.rs`, build the block:

```rust
const UPLOAD_BLOCK: &str = r#"<div id="dropzone" style="border:2px dashed var(--line);border-radius:8px;padding:1em;text-align:center;color:var(--muted);margin-bottom:1rem;cursor:pointer">
  drop files here or click to upload
  <input type="file" id="fpick" multiple style="display:none">
  <div id="uplist"></div>
</div>
<script>
(() => {
  const dz = document.getElementById('dropzone');
  const fp = document.getElementById('fpick');
  const list = document.getElementById('uplist');
  dz.onclick = () => fp.click();
  fp.onchange = () => sendAll(fp.files);
  ['dragover','dragenter'].forEach(ev => document.addEventListener(ev, e => {
    e.preventDefault(); dz.style.borderColor = 'var(--accent)';
  }));
  ['dragleave','drop'].forEach(ev => document.addEventListener(ev, e => {
    e.preventDefault(); dz.style.borderColor = 'var(--line)';
  }));
  document.addEventListener('drop', e => sendAll(e.dataTransfer.files));
  function sendAll(files) {
    let pending = files.length;
    [...files].forEach(f => {
      const row = document.createElement('div');
      row.textContent = f.name + ' 0%';
      list.appendChild(row);
      const fd = new FormData();
      fd.append('file', f);
      const xhr = new XMLHttpRequest();
      xhr.open('POST', location.pathname);
      xhr.setRequestHeader('Accept', 'application/json');
      xhr.upload.onprogress = e => {
        if (e.lengthComputable) row.textContent = f.name + ' ' + Math.round(100*e.loaded/e.total) + '%';
      };
      xhr.onload = () => {
        row.textContent = f.name + (xhr.status < 300 ? ' ✓' : ' ✗ ' + xhr.responseText);
        if (--pending === 0 && xhr.status < 300) location.reload();
      };
      xhr.onerror = () => { row.textContent = f.name + ' ✗ network error'; --pending; };
      xhr.send(fd);
    });
  }
})();
</script>"#;
```

`render_html` signature + replace:

```rust
pub fn render_html(rel_path: &str, entries: &[Entry], base: &str, zip: bool, upload: bool) -> String {
    // ...existing body...
    template
        .replace("{{title}}", /* unchanged */)
        .replace("{{crumbs}}", &crumbs)
        .replace("{{zip}}", &zip_btn)
        .replace("{{upload}}", if upload { UPLOAD_BLOCK } else { "" })
        .replace("{{rows}}", &rows)
}
```

`server.rs` call site:

```rust
        return Html(crate::listing::render_html(rel, &entries, &st.base, st.opts.zip, st.opts.upload))
            .into_response();
```

- [ ] **Step 3: Verify** — `cargo test` all PASS.
- [ ] **Step 4: Commit** — `git commit -am "feat: drag-and-drop upload UI with per-file progress"`

---

### Task 5: docs + polish

**Files:**
- Modify: `README.md`

- [ ] **Step 1:** README: move upload out of roadmap; add to Usage:

```bash
fshare --upload             # allow uploads into the browsed folder (drag & drop)
fshare --upload --max-upload-size 2G
```

Security notes bullet: "Uploads are opt-in (`--upload`); filenames are sanitized to their final component, collisions never overwrite (auto-rename), size capped with `--max-upload-size`."

- [ ] **Step 2:** `cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release` — all green, fix findings.
- [ ] **Step 3:** Manual smoke: `fshare --upload <tmpdir>`, `curl -F 'file=@somefile' http://127.0.0.1:8000/` then confirm file + log line.
- [ ] **Step 4: Commit** — `git commit -am "docs: upload usage and security notes"`

---

## Self-Review Notes

- Spec coverage: flags/parser (T1), sanitize+collision (T2), streaming handler with temp guard, cap 413, ENOSPC 507, 400 malformed, 405 disabled, JSON/303 responses, log event (T3), dropzone UI + progress (T4), README (T5). Client-abort cleanup covered by TempGuard drop (T3) — no direct test (hard to abort reliably); temp-leftover assertions in T3 tests cover success/cap paths.
- Type consistency: `render_html(rel, entries, base, zip, upload)` used identically in T4 test/impl/call site; `ShareOpts` literal updates enumerated in T3 Step 1.
- 303 vs Redirect::to: axum `Redirect::to` = 303 See Other. Matches spec.
