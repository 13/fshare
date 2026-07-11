# fshare — Banner QR Side-by-Side Layout Design

Date: 2026-07-11
Status: Approved

## Purpose

When the terminal is wide enough, print the QR code left of the address
list instead of below it; otherwise keep the current layout exactly.

## Behavior

- Condition for side-by-side: stdout is a tty AND QR enabled AND
  `terminal columns >= qr_width + gap(2) + longest_address_line_width`.
- Side-by-side block: QR lines (each prefixed with the existing 2-space
  indent) in the left column, padded to a fixed column width; address lines
  (mDNS line first, then interface URLs, same content/format as today) in
  the right column, top-aligned. Whichever column is longer determines the
  block height; the shorter column pads with blanks.
- Line width accounting must ignore ANSI color codes (the `➜` marker is
  colored). Compute display width from the uncolored text; apply color when
  printing.
- Fallback (narrow terminal, not a tty, or `--no-qr`): current behavior
  unchanged — address list, then QR (tty only).
- Notes (port bump, other instances, token, auth, limit) and `Ctrl+C to
  stop` stay below the block in both layouts.

## Architecture

- New crate: `terminal_size` (tiny, cross-platform ioctl wrapper).
- `main.rs` `print_banner` refactor:
  - Build `addr_lines: Vec<(String /*plain*/, String /*colored*/)>` —
    plain for width math, colored for printing.
  - Build `qr_lines: Vec<String>` (existing Dense1x2 render, split lines).
  - `fn side_by_side(qr: &[String], addr: &[(String, String)]) -> Vec<String>`
    pure zip/pad helper — unit-testable in `main.rs`? Binary target tests
    don't exist; put helper in `src/banner.rs` (new module in lib) together
    with the layout decision `fn fits(term_cols, qr_w, addr_w) -> bool`,
    unit-tested there. `print_banner` moves QR/address assembly to use it.
- QR width = char count of the first QR line (Dense1x2 output is
  rectangular).

## Testing

- Unit (`src/banner.rs`): `fits()` boundary cases; `side_by_side()` zips
  unequal-height columns correctly, pads left column to constant width,
  right column content preserved, works when QR taller than addresses and
  vice versa.
- Manual smoke: run in wide terminal (side-by-side), narrow (`COLUMNS` via
  `stty`/tmux or just resize) falls back; `--no-qr` unchanged; non-tty
  output unchanged (no QR at all).

## Out of scope

Vertical centering, QR right of addresses, changing QR size.
