# fshare phase 2 — `--upload` Design

Date: 2026-07-10
Status: Approved

## Purpose

Opt-in file uploads: drag-and-drop (and file picker) on the listing page,
files land in the directory being browsed.

## Scope

In: `--upload` flag, `--max-upload-size` cap, multipart POST endpoint,
dropzone UI with per-file progress, upload log events, collision auto-rename.
Out (future): basic auth, mDNS, TLS, bandwidth limit, airdrop push.

## Behavior

- `--upload` registers the POST route; without it no write endpoint exists
  (read-only guarantee preserved by construction).
- `--max-upload-size <size>` parses `500M`, `2G`, `100K` style values;
  default unlimited. Enforced while streaming; over-cap aborts with 413 and
  removes the partial temp file.
- Endpoint: `POST /<dir>/` (token prefix respected) with
  `multipart/form-data`, field name `file`, multiple files per request.
- Destination validated with existing `server::resolve()`: must exist, be a
  directory, inside root, not hidden. Otherwise 404. POST when uploads
  disabled: 405.
- Streaming: each part goes to `.fshare-upload-<rand>` in the destination
  directory (same filesystem, so final rename is atomic). On success:
  sanitize filename, resolve collisions, atomic rename. On any error or
  client abort: temp file deleted.
- Filename sanitization: take only the final path component; strip `\` and
  NUL; reject empty and dot-leading results (dot-leading allowed when
  `--hidden`). Rejected part → 400, other parts in same request still
  processed.
- Collision policy: `photo.jpg` exists → `photo (1).jpg`, then
  `photo (2).jpg`, … (extension preserved; names without extension get the
  suffix at the end).
- Response: 303 See Other back to the listing URL (works with plain HTML
  form); `{"saved": ["<final names>"]}` as JSON when the request has
  `Accept: application/json`.
- Log: new `Event::Upload { ip, name, bytes, secs }`, pretty line
  `⬆ photo.jpg received 4.2 MB in 3s`; JSON event `"upload"`. Uploads count
  in `stats.bytes` (received) but NOT toward `--max-downloads`.

## Architecture

- New module `src/upload.rs`: axum handler (`Multipart` extractor with
  per-field streaming), `sanitize_name()`, `unique_path()` (collision
  rename), temp-file guard (Drop removes file unless persisted).
- `server.rs`: `ShareOpts` gains `upload: bool`; `AppState` unchanged
  otherwise; router adds `.route("/", post(upload::handle))` and
  `.route("/{*path}", post(upload::handle))` only when uploads enabled;
  axum `DefaultBodyLimit` disabled in favor of our own streaming cap.
- `cli.rs`: `--upload` bool, `--max-upload-size` with size parser
  (`parse_size("2G") -> u64`).
- `listing.rs`: `render_html` gains `upload: bool`; template shows dropzone
  block + inline JS (XHR with `upload.onprogress` per file, listing refresh
  when all complete) only when enabled.

## UI

Dropzone strip above the file table: "drop files here or click to upload".
Whole page is a drop target. Per-file progress bars, error text inline.
No framework; extends the embedded template.

## Error handling

- Not a dir / hidden / outside root: 404.
- Uploads disabled: 405.
- Over size cap: 413, partial temp removed.
- Disk full (io error while writing): 507, temp removed.
- Malformed multipart: 400.
- Client abort mid-upload: temp removed, `✗` style log line, no crash.

## Testing

Unit: `sanitize_name` (traversal, backslash, dotfile, empty), `unique_path`
sequence, `parse_size`.
Integration: upload lands with exact content; multi-file request; collision
creates `(1)` variant; POST without `--upload` → 405; traversal filename
saved under sanitized name only; size cap → 413 and no temp left; temp
files absent after success and after abort; JSON response shape.
