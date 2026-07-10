use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use percent_encoding::percent_decode_str;
use rand::Rng;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Clone, Debug)]
pub struct ShareOpts {
    pub show_hidden: bool,
    pub follow_links: bool,
    pub zip: bool,
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
    pub opts: ShareOpts,
    pub base: String, // "" or "/s/<token>"
}

impl AppState {
    pub fn new(root: PathBuf, single_file: bool, opts: ShareOpts, token: bool) -> Self {
        let base = if token { format!("/s/{}", gen_token()) } else { String::new() };
        Self { root, single_file, opts, base }
    }
}

pub fn gen_token() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..12).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

pub fn router(state: Arc<AppState>) -> Router {
    let inner = Router::new()
        .route("/", get(handle))
        .route("/{*path}", get(handle))
        .with_state(state.clone());
    if state.base.is_empty() {
        inner
    } else {
        Router::new().nest(&state.base, inner)
    }
}

async fn handle(
    State(st): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
    uri: Uri,
    req: Request,
) -> Response {
    if st.single_file {
        return serve_single(&st, req).await;
    }

    let rel = uri.path().trim_start_matches('/').trim_end_matches('/');
    let Some(path) = resolve(&st.root, uri.path(), &st.opts) else {
        return not_found();
    };

    if path.is_dir() {
        if q.contains_key("zip") {
            if !st.opts.zip {
                return not_found();
            }
            return crate::zip::zip_response(path, rel.to_string(), st.opts.show_hidden);
        }
        let entries = crate::listing::read_dir_entries(&path, st.opts.show_hidden);
        if q.get("format").map(String::as_str) == Some("json") {
            return axum::Json(entries).into_response();
        }
        return Html(crate::listing::render_html(rel, &entries, &st.base, st.opts.zip))
            .into_response();
    }

    // file: delegate to ServeDir for Range/ETag/MIME
    match ServeDir::new(&st.root).oneshot(req).await {
        Ok(res) => res.map(Body::new),
        Err(_) => not_found(),
    }
}

async fn serve_single(st: &AppState, req: Request) -> Response {
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
        Err(_) => not_found(),
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "404 — not found").into_response()
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
        ShareOpts { show_hidden: false, follow_links: false, zip: true }
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
