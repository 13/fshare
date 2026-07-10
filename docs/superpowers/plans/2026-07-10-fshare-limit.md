# fshare `--limit` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `--limit 5M` caps total download throughput (global token bucket) on file/zip bodies.

**Architecture:** `AppState.limiter: Option<Limiter>` (async-speed-limit). `ThrottledBody<B>` in `server.rs` delays each next frame until `limiter.consume(prev_len)` resolves; `track` wraps downloads as `CountingBody<ThrottledBody<B>>`.

**Tech Stack:** `async-speed-limit` 0.4.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-10-fshare-limit-design.md`.
- Throttle only bodies passing the existing `is_download` gate. Counting wraps OUTSIDE throttle.
- `limiter: None` → wrapper never applied.
- `--limit 0` rejected (`parse_size` then explicit `>0` check via clap `value_parser` range or manual).
- AppState::new arity grows again — update main.rs + all tests/http.rs constructors (`spawn_opts`, `counts_completed_downloads`, `tls_serves_https`).

---

### Task 1: ThrottledBody + flag + wiring + tests

**Files:**
- Modify: `Cargo.toml`, `src/cli.rs`, `src/server.rs`, `src/main.rs`, `tests/http.rs`, `README.md`

**Interfaces:**
- Produces: `Args.limit: Option<u64>`; `AppState.limiter: Option<async_speed_limit::Limiter>`; `AppState::new(root, single_file, opts, token, events, auth, limit: Option<u64>)` builds `limiter = limit.map(|n| Limiter::new(n as f64))`.

- [ ] **Step 1: Dep** — `cargo add async-speed-limit`

- [ ] **Step 2: Failing integration test** (`tests/http.rs`):

```rust
#[tokio::test]
async fn bandwidth_limit_slows_downloads() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("blob.bin"), vec![42u8; 64 * 1024]).unwrap();
    let root = t.path().canonicalize().unwrap();
    let opts = fshare::server::ShareOpts {
        show_hidden: false,
        follow_links: false,
        zip: true,
        upload: false,
        max_upload: None,
    };
    // 256 KiB/s over 64 KiB ≥ ~0.25s minus initial burst; assert ≥120ms, well above unthrottled (<20ms)
    let state = Arc::new(fshare::server::AppState::new(
        root,
        false,
        opts,
        false,
        fshare::log::Logger::spawn(false),
        None,
        Some(256 * 1024),
    ));
    let app = fshare::server::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    let start = std::time::Instant::now();
    let body = reqwest::get(format!("http://{addr}/blob.bin"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.len(), 64 * 1024);
    assert!(body.iter().all(|b| *b == 42));
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(120),
        "took {:?} — limiter not applied?",
        start.elapsed()
    );
}
```

All other `AppState::new` call sites gain trailing `None` (no limit). Run: compile FAIL (arity).

- [ ] **Step 3: CLI** — `src/cli.rs` after `tls`:

```rust
    /// Cap total download speed, e.g. --limit 5M (bytes/second, all clients combined)
    #[arg(long, value_parser = parse_limit)]
    pub limit: Option<u64>,
```

and:

```rust
fn parse_limit(s: &str) -> Result<u64, String> {
    match parse_size(s)? {
        0 => Err("limit must be > 0".into()),
        n => Ok(n),
    }
}
```

- [ ] **Step 4: server.rs** — `AppState` gains `pub limiter: Option<async_speed_limit::Limiter>`; `new` gains `limit: Option<u64>` last param, sets `limiter: limit.map(|n| async_speed_limit::Limiter::new(n as f64))`.

`ThrottledBody` beside `CountingBody`:

```rust
type ConsumeFut = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

pin_project! {
    pub struct ThrottledBody<B> {
        #[pin]
        inner: B,
        limiter: async_speed_limit::Limiter,
        pending: Option<ConsumeFut>,
    }
}

impl<B> http_body::Body for ThrottledBody<B>
where
    B: http_body::Body<Data = Bytes>,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, B::Error>>> {
        let this = self.project();
        if let Some(fut) = this.pending {
            match fut.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(()) => *this.pending = None,
            }
        }
        match ready!(this.inner.poll_frame(cx)) {
            Some(Ok(frame)) => {
                if let Some(d) = frame.data_ref() {
                    let limiter = this.limiter.clone();
                    let n = d.len();
                    *this.pending = Some(Box::pin(async move {
                        limiter.consume(n).await;
                    }));
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => Poll::Ready(other),
        }
    }
}
```

In `track`, wrap throttle inside counting:

```rust
    let limiter = st.limiter.clone();
    res.map(|b| {
        let b: Body = match limiter {
            Some(l) => Body::new(ThrottledBody { inner: b, limiter: l, pending: None }),
            None => Body::new(b),
        };
        Body::new(CountingBody { inner: b, sent: 0, expected, done: false, on_end: Some(on_end) })
    })
```

(CountingBody generic over `Body` works — axum `Body: http_body::Body<Data = Bytes>`.) Add `use std::future::Future;` if needed for `.poll`.

- [ ] **Step 5: main.rs** — pass `args.limit` as last `AppState::new` arg; banner after auth note:

```rust
    if let Some(l) = args.limit {
        println!(
            "  {} download speed limited to {}/s",
            "note:".yellow(),
            fshare::listing::human_size(l)
        );
    }
```

- [ ] **Step 6: README** — Usage: `fshare --limit 5M           # cap total download speed`; Extras bullet: "Global download speed cap (`--limit 5M`)". Roadmap: drop bandwidth line.

- [ ] **Step 7: Verify** — `cargo test && cargo clippy --all-targets -- -D warnings`; smoke:

```bash
head -c 3M /dev/zero > /tmp/big.bin
./target/debug/fshare --limit 1M --port 18129 <dir-with-big.bin> &
time curl -s -o /dev/null http://127.0.0.1:18129/big.bin   # ~3s
```

- [ ] **Step 8: Commit** — `git commit -am "feat: global download bandwidth limit (--limit)"`

---

## Self-Review Notes

- Spec coverage: flag+parse+reject-0 (S3), global shared limiter (S4 state), throttle-only-downloads via existing gate placement (S4 track), counting outside (S4 wrap order), banner (S5), README (S6), timing integration test (S2).
- async-speed-limit 0.4 `Limiter::new(f64)`, `consume(usize) -> impl Future` — adapt per compiler; consume future must be 'static → clone limiter into async move.
