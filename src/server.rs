use axum::{
    body::Body,
    extract::{ConnectInfo, Query, Request, State},
    http::{header, StatusCode, Uri},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use percent_encoding::percent_decode_str;
use pin_project_lite::pin_project;
use rand::RngExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::time::Instant;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Clone, Debug)]
pub struct ShareOpts {
    pub show_hidden: bool,
    pub dir_sizes: bool,
    pub follow_links: bool,
    pub zip: bool,
    pub upload: bool,
    pub max_upload: Option<u64>,
}

/// root MUST be canonicalized by the caller (done once at startup).
pub fn resolve(root: &Path, uri_path: &str, opts: &ShareOpts) -> Option<PathBuf> {
    let decoded = percent_decode_str(uri_path).decode_utf8().ok()?;
    let mut p = root.to_path_buf();
    for comp in decoded.split('/').filter(|c| !c.is_empty()) {
        if comp == "." || comp == ".." || comp.contains('\\') || comp.contains('\0') {
            return None;
        }
        if !opts.show_hidden && comp.starts_with('.') {
            return None;
        }
        p.push(comp);
    }
    if opts.follow_links {
        return p.symlink_metadata().is_ok().then_some(p);
    }
    let canon = p.canonicalize().ok()?; // also fails for missing files
    canon.starts_with(root).then_some(canon)
}

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

pub fn gen_token() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..12).map(|_| CHARS[rng.random_range(0..CHARS.len())] as char).collect()
}

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
        _ => return not_found_res(wants_html(req.headers())),                           // wrong or missing token
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

async fn handle(
    State(st): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
    uri: Uri,
    req: Request,
) -> Response {
    let accept_html = wants_html(req.headers());
    if st.single_file {
        return serve_single(&st, accept_html, req).await;
    }
    let opts = st.opts();

    let rel_raw = uri.path().trim_start_matches('/').trim_end_matches('/');
    // decoded for display (breadcrumbs/title); resolve() decodes separately
    let rel = percent_decode_str(rel_raw).decode_utf8_lossy().into_owned();
    let Some(path) = resolve(&st.root, uri.path(), &opts) else {
        return not_found_res(accept_html);
    };

    if path.is_dir() {
        if q.contains_key("zip") {
            if !opts.zip {
                return not_found_res(accept_html);
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
        Err(_) => not_found_res(accept_html),
    }
}

async fn serve_single(st: &AppState, accept_html: bool, req: Request) -> Response {
    // root is the file itself; serve it for any path
    let name = st.root.file_name().unwrap_or_default().to_string_lossy().into_owned();
    match ServeFile::new(&st.root).oneshot(req).await {
        Ok(res) => {
            let mut res = res.map(Body::new);
            let cd = format!("attachment; filename=\"{name}\"");
            if let Ok(v) = header::HeaderValue::from_str(&cd) {
                res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
            }
            res
        }
        Err(_) => not_found_res(accept_html),
    }
}

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

pin_project! {
    pub struct CountingBody<B> {
        #[pin]
        inner: B,
        sent: u64,
        // known Content-Length; hyper may drop a body after the final frame
        // without polling to None, so "all bytes sent" also counts as complete
        expected: Option<u64>,
        done: bool,
        on_end: Option<Box<dyn FnOnce(u64, bool) + Send + 'static>>,
    }

    impl<B> PinnedDrop for CountingBody<B> {
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            if !*this.done {
                if let Some(f) = this.on_end.take() {
                    let complete = this.expected.is_some_and(|e| *this.sent >= e);
                    f(*this.sent, complete);
                }
            }
        }
    }
}

impl<B> http_body::Body for CountingBody<B>
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
        match ready!(this.inner.poll_frame(cx)) {
            Some(Ok(frame)) => {
                if let Some(d) = frame.data_ref() {
                    *this.sent += d.len() as u64;
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(e)) => Poll::Ready(Some(Err(e))),
            None => {
                *this.done = true;
                if let Some(f) = this.on_end.take() {
                    f(*this.sent, true);
                }
                Poll::Ready(None)
            }
        }
    }
}

type ConsumeFut = Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

pin_project! {
    /// Rate-limits a body by holding each data frame back until the shared
    /// token bucket grants its byte count. The delay happens BEFORE the
    /// frame is yielded: hyper stops polling known-length bodies after the
    /// final frame, so throttling after the yield would never fire for it.
    pub struct ThrottledBody<B> {
        #[pin]
        inner: B,
        limiter: async_speed_limit::Limiter,
        pending: Option<(ConsumeFut, http_body::Frame<Bytes>)>,
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
        if let Some((fut, _)) = this.pending {
            match fut.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(()) => {
                    let (_, frame) = this.pending.take().expect("pending present");
                    return Poll::Ready(Some(Ok(frame)));
                }
            }
        }
        match ready!(this.inner.poll_frame(cx)) {
            Some(Ok(frame)) => {
                let Some(d) = frame.data_ref() else {
                    return Poll::Ready(Some(Ok(frame)));
                };
                let limiter = this.limiter.clone();
                let n = d.len();
                let mut fut: ConsumeFut = Box::pin(async move {
                    limiter.consume(n).await;
                });
                match fut.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Some(Ok(frame))),
                    Poll::Pending => {
                        *this.pending = Some((fut, frame));
                        Poll::Pending
                    }
                }
            }
            other => Poll::Ready(other),
        }
    }
}

#[derive(Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub bytes: AtomicU64,
    pub clients: std::sync::Mutex<std::collections::HashSet<std::net::IpAddr>>,
}

pub async fn track(
    State(st): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let ip = addr.ip();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let is_file_get = method == "GET"
        && !path.ends_with('/')
        && req.uri().query().is_none_or(|q| !q.contains("format="));

    let start = Instant::now();
    let res = next.run(req).await;
    let status = res.status().as_u16();

    st.stats.requests.fetch_add(1, Ordering::Relaxed);
    st.stats.clients.lock().unwrap().insert(ip);
    let _ = st.events.send(crate::log::Event::Request {
        ip,
        method,
        path: path.clone(),
        status,
    });

    // wrap body of successful file/zip responses to detect completion
    let track_body = status == 200 || status == 206;
    if !(track_body && is_download(is_file_get, &res)) {
        return res;
    }

    let st2 = st.clone();
    let events = st.events.clone();
    // 206 = resumed/partial transfer; log it but don't count toward --max-downloads
    let counts_as_download = status == 200;
    let on_end = Box::new(move |bytes: u64, completed: bool| {
        st2.stats.bytes.fetch_add(bytes, Ordering::Relaxed);
        if completed && counts_as_download {
            // increment before notify so expiry's recheck sees the final count
            st2.downloads_done.fetch_add(1, Ordering::Relaxed);
            st2.download_signal.notify_waiters();
        }
        let _ = events.send(crate::log::Event::Done {
            ip,
            path,
            bytes,
            completed,
            secs: start.elapsed().as_secs_f64(),
        });
    });
    let expected = res
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let limiter = st.limiter.clone();
    res.map(|b| {
        let b = match limiter {
            Some(l) => Body::new(ThrottledBody { inner: b, limiter: l, pending: None }),
            None => Body::new(b),
        };
        Body::new(CountingBody { inner: b, sent: 0, expected, done: false, on_end: Some(on_end) })
    })
}

fn is_download(is_file_get: bool, res: &Response) -> bool {
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);
    let is_zip = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("zip"))
        .unwrap_or(false);
    is_zip || (is_file_get && !is_html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), "hello").unwrap();
        fs::write(root.join("sub/b.txt"), "world").unwrap();
        fs::write(root.join(".secret"), "shh").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", root.join("esc")).unwrap();
        (t, root)
    }

    fn opts() -> ShareOpts {
        ShareOpts {
            show_hidden: false,
            dir_sizes: false,
            follow_links: false,
            zip: true,
            upload: false,
            max_upload: None,
        }
    }

    #[test]
    fn resolves_normal_paths() {
        let (_t, root) = setup();
        assert_eq!(resolve(&root, "/a.txt", &opts()).unwrap(), root.join("a.txt"));
        assert_eq!(resolve(&root, "/sub/b.txt", &opts()).unwrap(), root.join("sub/b.txt"));
        assert_eq!(resolve(&root, "/", &opts()).unwrap(), root);
        assert_eq!(resolve(&root, "/sub%2Fb.txt", &opts()).unwrap(), root.join("sub/b.txt"));
    }

    #[test]
    fn rejects_bad_paths() {
        let (_t, root) = setup();
        assert!(resolve(&root, "/../x", &opts()).is_none());
        assert!(resolve(&root, "/%2e%2e/x", &opts()).is_none());
        assert!(resolve(&root, "/.secret", &opts()).is_none()); // dotfile
        assert!(resolve(&root, "/esc", &opts()).is_none()); // symlink escape
        assert!(resolve(&root, "/missing.txt", &opts()).is_none());
        // hidden opt-in
        let show = ShareOpts { show_hidden: true, ..opts() };
        assert!(resolve(&root, "/.secret", &show).is_some());
        // follow-links opt-in
        let follow = ShareOpts { follow_links: true, ..opts() };
        assert!(resolve(&root, "/esc", &follow).is_some());
    }
}
