use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;

pub fn zip_response(_dir: PathBuf, _rel: String, _show_hidden: bool) -> Response {
    (StatusCode::NOT_IMPLEMENTED, "zip pending").into_response()
}
