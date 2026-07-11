# fshare: Ratatui TUI with Live Settings Toggles — Design

Date: 2026-07-11
Status: approved

## Problem

Changing any setting (mDNS announcement, uploads, auth, …) requires stopping
the server and restarting with different flags — killing active downloads and
changing nothing for connected clients. There is no way to see or change the
share's behavior while it runs.

## Solution overview

Replace the plain streaming log with a ratatui full-screen TUI when stdout is
an interactive terminal. The TUI shows live state and accepts single-key
toggles that mutate runtime-shared settings read per-request by the server.
Plain log output remains for pipes, `--json-log`, `--no-tui`, and config
`tui = false`. All toggles are session-only — the config file stays
authoritative for defaults.

## Feature 1: Runtime-mutable settings (`LiveSettings`)

New struct owned by `AppState`, readable lock-free (or near) on the request
path, writable from the TUI task:

```rust
pub struct LiveSettings {
    pub mdns: AtomicBool,
    pub upload: AtomicBool,
    pub hidden: AtomicBool,
    pub dir_sizes: AtomicBool,
    pub zip: AtomicBool,
    pub auth: RwLock<Option<String>>,   // "user:pass", None = off
    pub base: RwLock<String>,           // "" or "/s/<token>"
}
```

Initialized from resolved `Settings` at startup. Handlers that today read
frozen `ShareOpts` fields (`hidden`, `dir_sizes`, `zip`, `upload`) read the
corresponding atomic instead (`Ordering::Relaxed` suffices — no ordering
dependency between settings). `ShareOpts` keeps only truly immutable options
(`follow_links`, `max_upload_size`, `single_file` handling stays as-is).

### Required server refactors

1. **Upload routes always registered.** The router no longer branches on
   `opts.upload`; POST `/` and POST `/{*path}` are always routed to
   `upload::handle`, which returns `405 Method Not Allowed` when
   `live.upload` is false (matches both today's GET-only router response
   and the existing guard in `upload::handle`, so behavior when off is
   unchanged). `DefaultBodyLimit::disable()`
   applies unconditionally; `max_upload_size` still enforces the real cap.
2. **Token prefix becomes middleware.** Today `/s/<token>` is baked in via
   `Router::nest`, which cannot change at runtime. Replace with a
   `token_gate` middleware (outermost, before `track`): reads
   `live.base`; if non-empty, the request path must start with that prefix
   (else `404`), and the prefix is stripped before inner routing (rewrite
   `req.uri`). The `<base>/` redirect special case moves into the same
   middleware. Link generation (listing HTML, zip links, QR, banner/TUI
   URLs) reads `live.base` at render time.
3. **Auth middleware reads `live.auth`** instead of `AppState.auth`.
   Constant-time comparison unchanged. Toggling auth on without stored
   credentials generates a fresh password (existing generator) and surfaces
   it in the TUI header.
4. **mDNS toggle.** `mdns::announce` returns a handle (`ServiceDaemon` +
   fullname) kept by the TUI task. Toggle off = `unregister`; toggle on =
   re-announce. Failure to (re-)announce shows an error in the TUI, flag
   reflects actual state, not wish.

## Feature 2: Ratatui TUI (`src/tui.rs`)

Dependencies: `ratatui`, `crossterm` (its default backend).

### Layout

```
┌ fshare v0.1.5 ── /home/ben/photos ── 142 files 1.3 GB ─┐
│ ➜ https://192.168.1.5:8000/s/ab12cd  2 clients  312 MB │  header
├─────────────────────────────────────────────────────────┤
│ 12:04:15 192.168.1.23 GET /vid.mp4 200                  │  log pane
│ 12:04:29 192.168.1.23 ✓ /vid.mp4 312MB 22MB/s           │  (scrolls)
├─────────────────────────────────────────────────────────┤
│ [m]dns:on [u]pload:off [a]uth:off [t]oken:on [h]idden…  │  hotkey bar
└─────────────────────────────────────────────────────────┘
```

- **Header:** primary URL(s, reflecting live token base and TLS scheme),
  share path, file count/size, live stats (unique clients, bytes sent,
  current aggregate speed). When auth was just enabled with a generated
  password, the credentials show in the header until any key is pressed.
- **Log pane:** consumes the existing `log::Event` mpsc channel (same
  rendering text as the plain printer). Keeps last 1000 lines in a ring
  buffer; Up/Down/PgUp/PgDn scroll, any new event snaps back to follow mode
  only when already at bottom.
- **Hotkey bar:** each toggle as `[key]name:state`, dimmed when off,
  highlighted when on. Reflects actual `LiveSettings` state.

### Keys

| Key | Action |
|-----|--------|
| `m` | toggle mDNS announce |
| `u` | toggle uploads |
| `a` | toggle auth (on = generated password shown in header; if credentials came from CLI/config, those are reused) |
| `t` | toggle token URL: off = plain base; on = NEW random token (old links die) |
| `h` | toggle hidden files |
| `d` | toggle dir sizes |
| `z` | toggle zip downloads |
| `Q` | QR popup overlay for current primary URL (any key closes) |
| `?` | help popup listing keys (any key closes) |
| Up/Down/PgUp/PgDn | scroll log |
| `q`, `x`, Ctrl+C | quit (same graceful shutdown + summary as today, summary printed after terminal restore) |

Toggling a setting appends a synthetic line to the log pane
(`12:05:01 ⚙ upload enabled`), and to the JSON log if that mode were active
(it is not — JSON log forces plain mode; the synthetic event type exists in
`log::Event` regardless so plain/TUI share one enum).

### Terminal hygiene

- Alternate screen + raw mode on entry; restored on exit, panic (panic
  hook), and Ctrl+C.
- Shutdown summary (requests, clients, bytes) prints to the normal screen
  after restore, so it survives in scrollback.

## Feature 3: Activation and precedence

- TUI runs when: stdout is a tty AND `json_log` is off AND effective
  `tui` setting is true.
- New setting `tui` (default true): config key `tui = false`, CLI pair
  `--tui/--no-tui` via the existing tri-state machinery.
- Plain mode (current banner + streaming log printer) is used otherwise —
  zero behavior change for scripts, pipes, tests.
- `--secure` implies nothing about `tui`; toggling mDNS back on in the TUI
  after `--secure` is allowed — explicit user action wins, consistent with
  the secure-bundle override rules.

## Interactions and edge cases

- **Timeout / max-downloads shutdown** while TUI active: same code path as
  Ctrl+C — restore terminal, print summary and shutdown reason.
- **Token regeneration mid-download:** in-flight responses continue
  (connection already routed); new requests need the new prefix.
- **Auth toggle mid-download:** in-flight streams continue; next request
  gets 401 without credentials.
- **Terminal resize:** ratatui redraws on resize events; QR popup re-renders
  or shows "terminal too small" if it cannot fit.
- **No color / dumb terminals:** crossterm handles capability degradation;
  `TERM=dumb` fails tty heuristics rarely — if raw mode entry fails, fall
  back to plain mode with a note.

## Testing

- `LiveSettings` behavior tests, no terminal needed (axum router in-memory):
  upload 404 when off / accepted when on, flipped mid-test; auth 401 appears
  after enabling; token middleware — old prefix 404s and new prefix works
  after regeneration; hidden/zip/dir_sizes flips change listing output.
- `token_gate` middleware unit tests: empty base passes through untouched,
  prefix stripped correctly, `<base>/` redirect, non-matching path 404.
- TUI render tests with `ratatui::backend::TestBackend`: header shows URL +
  stats, hotkey bar reflects toggle states, log ring buffer trims at
  capacity, scroll offset math.
- Integration tests unchanged (not a tty → plain mode automatically).

## Feature 4: Styled 404 page

Browser requests (Accept header contains `text/html`) that hit a 404 get a
styled page (`src/404.html`) matching `listing.html`'s look: same CSS
variables (auto light/dark), fshare logo, large muted "404", "nothing here"
line, back link, version footer. Non-browser clients (curl, JSON consumers)
keep the plain `404 — not found` text body.

Security constraint: the page must contain **no link to the share root** —
the same 404 fires for wrong/missing token prefixes, and embedding the real
base would hand the token to guessers. The back link is
`javascript:history.back()` only. The template has no request-derived
content (only the static version string), so no injection surface.

## Implementation order

One branch, five tasks:
1. `LiveSettings` + server refactors (upload always-routed, auth from live,
   handlers read atomics) — plain mode still default output.
2. Token-prefix middleware replacing `nest` (largest server change).
3. TUI module: event loop, layout, log ring, toggles wired, QR/help popups.
4. Activation logic (`tui` setting, tty detection, fallback), docs, README.
5. Styled 404 page with Accept-header content negotiation.
