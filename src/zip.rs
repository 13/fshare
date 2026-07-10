use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use axum::body::Body;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;
use tokio_util::compat::FuturesAsyncWriteCompatExt;
use tokio_util::io::ReaderStream;
use walkdir::WalkDir;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub fn zip_response(dir: PathBuf, rel: String, show_hidden: bool) -> Response {
    let name = if rel.is_empty() {
        "fshare.zip".to_string()
    } else {
        format!("{}.zip", rel.rsplit('/').next().unwrap_or("fshare"))
    };

    let (w, r) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Err(e) = write_zip(w, dir, show_hidden).await {
            eprintln!("fshare: zip stream aborted: {e}");
        }
    });

    (
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        Body::from_stream(ReaderStream::new(r)),
    )
        .into_response()
}

async fn write_zip(
    w: tokio::io::DuplexStream,
    dir: PathBuf,
    show_hidden: bool,
) -> Result<(), BoxError> {
    let mut zip = ZipFileWriter::with_tokio(w);
    // collect entries off the async thread
    let base = dir.clone();
    let files = tokio::task::spawn_blocking(move || {
        WalkDir::new(&base)
            .into_iter()
            .filter_entry(move |e| {
                show_hidden
                    || e.depth() == 0
                    || !e.file_name().to_string_lossy().starts_with('.')
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect::<Vec<_>>()
    })
    .await?;

    for path in files {
        let rel = path.strip_prefix(&dir)?.to_string_lossy().into_owned();
        let entry = ZipEntryBuilder::new(rel.into(), Compression::Deflate);
        let mut writer = zip.write_entry_stream(entry).await?.compat_write();
        let mut f = tokio::fs::File::open(&path).await?;
        tokio::io::copy(&mut f, &mut writer).await?;
        writer.into_inner().close().await?;
    }
    zip.close().await?;
    Ok(())
}
