# fshare — `--limit` Bandwidth Limiting Design

Date: 2026-07-10
Status: Approved

## Purpose

Cap total download throughput so fshare doesn't saturate the host's uplink.

## Behavior

- `--limit <size>`: global bytes-per-second budget shared by all clients
  and connections, e.g. `--limit 5M` = 5 MB/s total. Value parsed with the
  existing `cli::parse_size`; zero rejected at parse time with clear error.
- Applies only to download response bodies (files and zip streams — the
  same responses the `track` middleware already identifies via its
  `is_download` gate). Listings, JSON, 401/404 pages, and uploads are
  unthrottled.
- Logged transfer speeds continue to reflect wall-clock reality (byte
  counting wraps outside the throttle).
- Banner: `note: download speed limited to <human_size>/s`.

## Architecture

- Crate: `async-speed-limit` — clock-based token-bucket `Limiter`,
  clone-shared.
- `AppState.limiter: Option<async_speed_limit::Limiter>` — built in
  `main.rs` from `args.limit`; `None` means the wrapper is never applied
  (zero overhead).
- New `ThrottledBody<B>` in `src/server.rs` beside `CountingBody`:
  - holds `inner: B`, `limiter: Limiter`, `pending: Option<BoxFuture<'static, ()>>`
  - `poll_frame`: first drain `pending` (Pending → return Pending); then
    poll inner; on a data frame of `n` bytes store
    `limiter.clone().consume(n)` as the new `pending` and yield the frame.
    Throttle therefore delays the *next* frame — standard token-bucket
    streaming.
- Wrap order in `track`: `CountingBody<ThrottledBody<B>>` (counting
  outside).
- `cli.rs`: `--limit` with `value_parser = parse_size`.

## Testing

- Integration: fixture file of 64 KiB served with `Limiter` at 256 KiB/s —
  download completes with correct content and takes ≥ 200 ms; same file
  with `limiter: None` unaffected (existing tests already cover fast path).
- Existing suite must stay green (wrapper only active with limiter set).

## Out of scope

Per-client budgets, upload throttling, dynamic adjustment at runtime.
