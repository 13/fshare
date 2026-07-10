# fshare Basic Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optional HTTP Basic auth (`--auth[=USER[:PASS]]`) guarding every route, with generated phone-typeable passwords printed in the banner.

**Architecture:** New `src/auth.rs`: credential parsing, password generation, constant-time comparison, axum middleware. `AppState.auth: Option<String>` holds `"user:pass"`; middleware layered inside `track` so 401s hit the log. `base64` crate decodes the header.

**Tech Stack:** axum 0.8 middleware `from_fn_with_state`, `base64` 0.22, existing rand.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-10-fshare-auth-design.md`.
- Bare `--auth` must not consume the positional DIR (`require_equals`).
- Comparison constant-time, length-independent. Garbage header → 401 never 500.
- 401 carries `WWW-Authenticate: Basic realm="fshare"`.
- Generated password: 10 chars, alphabet excludes `0 O 1 l I`.
- `track` stays outermost (401s logged). Auth guards ALL routes/methods.
- Existing 26 tests keep passing.

---

### Task 1: auth.rs — parse, generate, constant-time eq

**Files:**
- Create: `src/auth.rs`; Modify: `src/lib.rs` (add `pub mod auth;`), `Cargo.toml` (add `base64 = "0.22"`)

**Interfaces:**
- Produces:
  - `auth::parse_auth(arg: &Option<String>) -> Result<String, String>` — input is the inner value of `--auth` (`None` = bare flag). Returns `"user:pass"`. Bare/user-only get generated password.
  - `auth::gen_password() -> String`
  - `auth::ct_eq(a: &[u8], b: &[u8]) -> bool`

- [ ] **Step 1: Failing tests** in `src/auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credentials() {
        let full = parse_auth(&Some("ben:secret".into())).unwrap();
        assert_eq!(full, "ben:secret");
        let colons = parse_auth(&Some("ben:se:cret".into())).unwrap();
        assert_eq!(colons, "ben:se:cret");
        let user_only = parse_auth(&Some("ben".into())).unwrap();
        assert!(user_only.starts_with("ben:") && user_only.len() == 4 + 10);
        let bare = parse_auth(&None).unwrap();
        assert!(bare.starts_with("fshare:") && bare.len() == 7 + 10);
        assert!(parse_auth(&Some("".into())).is_err());
        assert!(parse_auth(&Some(":x".into())).is_err());
    }

    #[test]
    fn password_alphabet_safe() {
        for _ in 0..50 {
            let p = gen_password();
            assert_eq!(p.len(), 10);
            assert!(!p.chars().any(|c| "0O1lI".contains(c)), "{p}");
        }
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"x"));
        assert!(ct_eq(b"", b""));
    }
}
```

Run: `cargo test auth::` — FAIL.

- [ ] **Step 2: Implement**

```rust
use rand::Rng;

const PW_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn gen_password() -> String {
    let mut rng = rand::thread_rng();
    (0..10)
        .map(|_| PW_ALPHABET[rng.gen_range(0..PW_ALPHABET.len())] as char)
        .collect()
}

/// `arg` is the value of `--auth=...`; `None` means bare `--auth`.
pub fn parse_auth(arg: &Option<String>) -> Result<String, String> {
    let (user, pass) = match arg {
        None => ("fshare".to_string(), None),
        Some(v) => match v.split_once(':') {
            Some((u, p)) => (u.to_string(), Some(p.to_string())),
            None => (v.clone(), None),
        },
    };
    if user.is_empty() {
        return Err("--auth: user must not be empty (use --auth=user[:pass])".into());
    }
    let pass = pass.unwrap_or_else(gen_password);
    Ok(format!("{user}:{pass}"))
}

pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i % b.len().max(1)).unwrap_or(&0);
        diff |= (x ^ y) as usize;
    }
    diff == 0
}
```

Note on `ct_eq`: loop length depends only on the longer input (attacker-controlled side vs fixed secret — both traversed fully); length mismatch folded into `diff` up front. `b.len().max(1)` avoids div-by-zero on empty.

Wait — `Some("".into())` (i.e. `--auth=`): clap delivers empty string; `split_once(':')` → None → user = "" → error. Covered.

- [ ] **Step 3: Verify** — `cargo test auth::` PASS.
- [ ] **Step 4: Commit** — `git commit -am "feat: credential parsing, password generation, constant-time compare"`

---

### Task 2: middleware + wiring + integration tests

**Files:**
- Modify: `src/auth.rs`, `src/cli.rs`, `src/server.rs`, `src/main.rs`, `tests/http.rs`

**Interfaces:**
- Consumes: `auth::ct_eq`, `AppState`.
- Produces:
  - `cli::Args.auth: Option<Option<String>>`
  - `AppState.auth: Option<String>`; `AppState::new` gains trailing `auth: Option<String>` param — update ALL call sites (`tests/http.rs` spawn_opts + counts test, `main.rs`).
  - `auth::require(State<Arc<AppState>>, Request, Next) -> Response` middleware.

- [ ] **Step 1: CLI flag** — `src/cli.rs` after `max_upload_size`:

```rust
    /// Require HTTP Basic auth: --auth (generated), --auth=user or --auth=user:pass
    #[arg(long, require_equals = true, value_name = "USER[:PASS]")]
    pub auth: Option<Option<String>>,
```

- [ ] **Step 2: Failing integration test** (append to `tests/http.rs`; extend `spawn_opts` with `auth: Option<String>` param — `spawn`/`spawn_capped` pass `None`):

```rust
#[tokio::test]
async fn basic_auth_gates_all_routes() {
    let t = fixture();
    let (base, _h) = spawn_auth(t.path().into(), "ben:secret").await;
    let c = reqwest::Client::new();

    // no credentials → 401 with prompt header
    let r = c.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert!(r.headers()["www-authenticate"].to_str().unwrap().contains("Basic"));

    // wrong password → 401
    let r = c.get(format!("{base}/hello.txt")).basic_auth("ben", Some("wrong"))
        .send().await.unwrap();
    assert_eq!(r.status(), 401);

    // garbage header → 401 not 500
    let r = c.get(format!("{base}/")).header("Authorization", "Basic !!!not-base64!!!")
        .send().await.unwrap();
    assert_eq!(r.status(), 401);

    // correct → 200 listing and file
    let r = c.get(format!("{base}/hello.txt")).basic_auth("ben", Some("secret"))
        .send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "hello world");

    // upload also guarded
    let part = reqwest::multipart::Part::bytes(b"x".to_vec()).file_name("a.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c.post(format!("{base}/")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 401);
}
```

`spawn_auth` helper (uploads on so the upload-guard assertion is meaningful):

```rust
async fn spawn_auth(root: PathBuf, creds: &str) -> (String, tokio::task::JoinHandle<()>) {
    spawn_opts(
        root,
        false,
        fshare::server::ShareOpts {
            show_hidden: false,
            follow_links: false,
            zip: true,
            upload: true,
            max_upload: None,
        },
        Some(creds.to_string()),
    )
    .await
}
```

`spawn_opts` gains 4th param `auth: Option<String>` passed into `AppState::new(root, false, opts, token, logger, auth)`; existing helpers pass `None`.

Run: `cargo test --test http` — FAIL (compile: AppState::new arity).

- [ ] **Step 3: AppState + middleware**

`server.rs`: `AppState` gains `pub auth: Option<String>`; `new(..., auth: Option<String>)` stores it. Router: add auth layer INSIDE track (auth added BEFORE track in code order — tower: last `.layer()` = outermost):

```rust
    let inner = inner
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::require))
        .layer(axum::middleware::from_fn_with_state(state.clone(), track))
        .with_state(state.clone());
```

`src/auth.rs` middleware:

```rust
use crate::server::AppState;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use std::sync::Arc;

pub async fn require(
    State(st): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = &st.auth else {
        return next.run(req).await;
    };
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .map(|creds| ct_eq(&creds, expected.as_bytes()))
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, r#"Basic realm="fshare""#)],
            "401 — authentication required",
        )
            .into_response()
    }
}
```

`main.rs`:

```rust
    let auth = match &args.auth {
        Some(v) => Some(fshare::auth::parse_auth(v).map_err(|e| e)?),
        None => None,
    };
```

(place before `AppState::new`, pass as last arg). Banner addition in `print_banner` (pass `auth: &Option<String>` param or read from state — state simpler; add after token note):

```rust
    if let Some(a) = &state.auth {
        let (user, pass) = a.split_once(':').unwrap_or((a.as_str(), ""));
        match args.auth.as_ref().and_then(|v| v.as_ref()) {
            Some(v) if v.contains(':') => {
                println!("  {} auth enabled (user {user})", "note:".yellow());
            }
            _ => {
                println!(
                    "  {} auth enabled — user: {user}  password: {pass}",
                    "note:".yellow()
                );
            }
        }
    }
```

- [ ] **Step 4: Verify** — `cargo test` all PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat: HTTP basic auth with generated passwords and constant-time check"`

---

### Task 3: docs + release

**Files:**
- Modify: `README.md`

- [ ] **Step 1:** Usage block gains:

```bash
fshare --auth               # basic auth, generated password shown in banner
fshare --auth=ben:secret    # explicit credentials
```

Security notes: replace "`--token` ... is not authentication" sentence's follow-up with: "`--auth` adds HTTP Basic authentication (constant-time verified). Note: credentials travel base64-encoded over plain HTTP — fine for a trusted LAN, use a VPN/tunnel beyond that." Roadmap: drop basic auth line.

- [ ] **Step 2:** `cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release`.
- [ ] **Step 3:** Smoke: run with `--auth`, curl 401 without creds, 200 with; banner shows password.
- [ ] **Step 4: Commit** — `git commit -am "docs: basic auth usage and security notes"`

---

## Self-Review Notes

- Spec coverage: CLI shapes + errors (T1 parse, T2 flag), generated alphabet (T1), middleware/401/WWW-Authenticate/garbage-header (T2), track-outermost logging (T2 layer order), banner both variants (T2), README caveat (T3). Combined auth+token: covered implicitly — token nests router, auth layers inner router; add quick assertion? Existing token test plus auth test suffice (layers independent); skip extra test (YAGNI).
- `ct_eq` reviewed: iteration bound = max(len); secret comparison never early-exits.
- Arity changes enumerated: `AppState::new` 6 args; call sites tests/http.rs (spawn_opts, counts_completed_downloads) + main.rs.
