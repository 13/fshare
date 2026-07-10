# fshare v1 — Design

Date: 2026-07-10
Status: Approved

## Purpose

`fshare` is a Rust CLI HTTP file server: a modern, sophisticated replacement for
`python3 -m http.server`. Launched in a directory, it serves that directory over
HTTP on the LAN with a polished terminal and web experience.

## V1 Scope

In: serve directory (read-only), port probing with auto-bump, IP enumeration
with LAN-first ranking, terminal QR code, running-instance detection,
live connection/download log, temporary-sharing controls (`--timeout`,
`--max-downloads`), opt-in tokenized URLs, polished directory listing web UI,
streamed zip download of folders, single-file share mode, JSON listing output.

Out (phase 2, documented at end): upload, basic auth, mDNS, TLS, bandwidth
limit, airdrop-style push.

## Stack

- `axum` + `tokio` — HTTP server
- `tower-http` — `ServeDir` base layer: Range requests, ETag/Last-Modified,
  MIME types come free
- `clap` (derive) — CLI
- `qrcode` — terminal QR rendering
- `if-addrs` (or `local-ip-address`) — NIC enumeration
- `async_zip` — streaming zip without temp files
- `tracing` — event pipeline for the connection log
- `owo-colors` — terminal color output

Single binary, no runtime assets (HTML template embedded via `include_str!`).

## Module Layout

```
src/
  main.rs        — CLI parse, startup sequence, banner
  cli.rs         — clap args
  server.rs      — axum router, middleware stack
  listing.rs     — dir listing handler, embedded HTML template, JSON mode
  zip.rs         — streaming zip handler
  net.rs         — port probe/bump, IP enumeration + LAN ranking
  instance.rs    — running-instance detection via runtime files
  log.rs         — connection/download event formatting
  expiry.rs      — --timeout / --max-downloads shutdown logic
```

## Startup Sequence

1. Parse args; canonicalize target dir (or file); exit with clear error if
   unreadable/nonexistent.
2. Detect other running fshare instances (see below); print informational note.
3. Probe port: default 8000; if busy, try 8001..8010 and print what happened.
   `--port` forces an exact port and errors if busy. All busy: exit, listing
   holders where detectable.
4. Enumerate interface IPs. Rank: private LAN ranges (192.168/16, 10/8,
   172.16/12) first, then other routable addresses (e.g. tailscale), loopback
   last and only shown as an extra. Print one URL per address with interface
   name.
5. Print banner: version, shared path, file count + total size, URL list, QR
   code of the top-ranked URL (auto-suppressed when stdout is not a tty),
   instance note, "Ctrl+C to stop".
6. Serve until Ctrl+C, `--timeout` expiry, or `--max-downloads` reached; then
   graceful shutdown with summary (total requests, unique clients, bytes sent).

## Instance Detection

On start, write `$XDG_RUNTIME_DIR/fshare/<port>.json` containing PID, shared
dir, port. Fall back to `/tmp/fshare-$UID/` when `XDG_RUNTIME_DIR` unset.
On startup, scan existing files, check PID liveness via `/proc/<pid>`;
report live instances informationally ("another fshare serving /home/ben/docs
on :8001, PID 4321") — never block startup. Remove stale files. Remove own
file on shutdown (best effort; stale-cleanup covers crashes).

## CLI

```
fshare [DIR] [flags]           # DIR defaults to .
fshare file.iso                # single-file mode: direct download link

--port, -p <N>      exact port, error if busy (disables auto-bump)
--bind <addr>       default 0.0.0.0
--timeout <dur>     e.g. 30m, 2h — auto-shutdown
--max-downloads <N> shutdown after N completed file downloads
--token             random URL prefix /s/<12 alnum chars>/; requests outside
                    prefix get 404
--zip / --no-zip    folder zip download (default on)
--hidden            show dotfiles (default: hidden in listing AND direct fetch)
--qr / --no-qr      QR display (default on, auto-off if not a tty)
--json-log          machine-readable (JSON lines) event log instead of pretty
```

## Terminal UX

Banner example:

```
  fshare v0.1.0 — serving /home/ben/photos (142 files, 1.3 GB)

  ➜ http://192.168.1.5:8000     (LAN, wlan0)
    http://10.0.3.2:8000        (tailscale0)
    http://localhost:8000

  [QR code]

  note: another fshare serving /home/ben/docs on :8001 (PID 4321)
  Ctrl+C to stop
```

Live log, one line per event; peer IP plus reverse-DNS hostname when
resolvable (async lookup, cached, never blocks request handling):

```
12:04:11  192.168.1.23  GET /            200  listing
12:04:15  192.168.1.23  GET /vid.mp4     206  48 MB/312 MB  22 MB/s
12:04:29  192.168.1.23  ✓ vid.mp4 complete  312 MB in 14s
```

Canceled downloads logged as `✗ canceled at <bytes>`; never crash the server.

## Request Flow

Tracing middleware records peer IP, path, status, bytes, duration for every
request. Router:

- `GET /` and subdirectories → listing page (HTML) or `?format=json` (JSON
  array of entries: name, type, size, mtime)
- `GET /<file>` → `ServeDir`-backed file response (Range, ETag, MIME)
- `GET /<dir>?zip` → streamed zip of that folder (recursive), no temp files
- With `--token`: everything above lives under `/s/<token>/`; anything else 404

Single-file mode serves a minimal page with one download link plus the direct
file URL.

## Web UI

One embedded HTML template, no build step, no framework. Features: breadcrumb
navigation; table with name / size / mtime; folders sorted first; column
sorting via small inline JS; unicode file-type icons; "Download all (.zip)"
button per folder; dark mode via `prefers-color-scheme`; responsive/mobile
friendly.

## Security

- Path traversal: canonicalize every requested path; result must be a prefix
  of the shared root, else 404.
- Symlinks resolving outside the root refused by default; `--follow-links`
  opt-in.
- Dotfiles hidden by default from both listing and direct fetch.
- `--token` provides casual URL-guessing protection. Real auth is phase 2.
- Read-only by construction: no write endpoints exist in v1.

## Error Handling

- Bad target dir/file: pre-bind exit with clear message.
- Ports 8000–8010 all busy: exit, name holders where detectable.
- File deleted mid-serve: 404, warning logged, server continues.
- Client disconnects mid-download: logged, connection cleaned up, no crash.

## Expiry

- `--timeout`: tokio timer triggers graceful shutdown.
- `--max-downloads N`: atomic counter incremented on *completed* file
  downloads (not listings, not canceled transfers); shutdown when reached.

## Testing

- Unit: IP ranking in `net.rs`, path-traversal rejection, expiry counter,
  instance-file parse/stale handling, duration parsing.
- Integration: spawn server on ephemeral port, drive with `reqwest`: listing
  HTML content, JSON mode, Range request resume, zip stream validity
  (unzippable, correct contents), token-prefix 404 behavior, dotfile 404,
  `..%2f` traversal attempts.
- Development follows TDD.

## Phase 2 (not built now)

`--upload` drag-drop upload page; `--user/--pass` basic auth; mDNS
announcement (`fshare.local`); optional self-signed TLS; bandwidth limiting;
airdrop-style push between fshare instances.
