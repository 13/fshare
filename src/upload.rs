use crate::server::{resolve, AppState};
use axum::extract::multipart::Multipart;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncWriteExt;

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
    let opts = st.opts();
    if !opts.upload {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(dir) = resolve(&st.root, uri.path(), &opts) else {
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
        let Some(name) = sanitize_name(&raw, opts.show_hidden) else {
            return (StatusCode::BAD_REQUEST, format!("bad filename '{raw}'")).into_response();
        };

        match save_part(field, &dir, &name, opts.max_upload).await {
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
        let back = format!("{}{}", st.base(), uri.path());
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
    let mut f = tokio::fs::File::create(&tmp).await.map_err(|e| io_response(&e))?;
    let mut written: u64 = 0;
    loop {
        let chunk = match field.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(_) => return Err(StatusCode::BAD_REQUEST.into_response()),
        };
        written += chunk.len() as u64;
        if cap.is_some_and(|c| written > c) {
            return Err(
                (StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds size limit").into_response()
            );
        }
        f.write_all(&chunk).await.map_err(|e| io_response(&e))?;
    }
    f.flush().await.map_err(|e| io_response(&e))?;
    drop(f);
    let dest = unique_path(dir, name);
    let tmp_path = guard.persist();
    if let Err(e) = tokio::fs::rename(&tmp_path, &dest).await {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(io_response(&e));
    }
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
