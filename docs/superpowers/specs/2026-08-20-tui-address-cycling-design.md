# TUI address cycling — design

**Date:** 2026-08-20
**Status:** approved

## Problem

The TUI always treats the first entry of `net::ranked_ifaces()` as the
primary address: the `➜` marker in the header URL list, the side-panel QR,
the `Q` popup and the URL caption under the QR all derive from
`App::primary_url()`, which is hardcoded to index 0.

Ranking cannot always be right. A host commonly has several private
addresses at once — Wi-Fi, wired, WireGuard, corporate 172.16/12 — and only
the operator knows which subnet the phone scanning the QR code is on. As of
v0.1.11 `is_home_lan()` prefers 192.168.0.0/16, which fixes the common case
but still leaves no way to reach the others: whoever needs the wg0 or the
172.16/12 address has to read it off the list and type it by hand.

## Decision

`Tab` / `Shift+Tab` cycle the selected address. The selection is the TUI's
single notion of "primary": the `➜` marker, both QR renderings and the URL
caption all follow it. The cycle walks exactly the entries the header list
already shows.

Rejected alternatives:

- **A plain `usize` index into the rendered list.** The list is rebuilt every
  frame, so pressing `m` to drop the mDNS line shifts every index and
  silently re-targets the QR without the user touching Tab.
- **Storing the selected URL string and matching it on redraw.** The URL
  embeds scheme and base, so `s` (http→https) or `t` (token base) would stop
  matching and silently reset the selection.
- **A QR-local pointer, leaving `➜` on the ranked best.** Lets the marker and
  the encoded address disagree, which is exactly the confusion to avoid.

## Design

### Data model (`src/tui.rs`)

```rust
#[derive(Clone, PartialEq, Eq)]
pub enum SelKey { Mdns, Ip(IpAddr) }

pub struct UrlEntry {
    pub url: String,     // "https://192.168.1.112:8000/s/tok/"
    pub label: String,   // "mDNS" | "LAN, wlan0" | "eth0"
    pub key: SelKey,
}
```

`App` gains one field: `sel: Option<SelKey>`. `None` means "follow the
ranked best" (index 0) — today's behavior, which keeps auto-updating as
interfaces appear and disappear. `Tab` is what takes manual control.

`SelKey` deliberately carries no scheme, port or base, so the `s`, `t` and
`a` toggles never disturb the selection.

### Entry construction

```rust
fn entries_from(all: &[Iface], mdns_on: bool, scheme: &str, port: u16, base: &str) -> Vec<UrlEntry>
fn url_entries(&self) -> Vec<UrlEntry>   // wrapper: ranked_ifaces() + live state
```

`entries_from` holds the rules currently inlined in `url_lines()`:

1. the mDNS `.local` entry first, when announcing;
2. then interfaces that are neither loopback nor virtual
   (`is_virtual_iface`), in `ranked_ifaces()` order;
3. falling back to all non-loopback interfaces when step 2 is empty
   (today's "nothing physical" fallback);
4. falling back to a single `localhost` entry when even that is empty,
   matching today's `primary_url()` fallback.

The split exists for testability as much as for clarity: the current TUI
tests read the host's real interfaces, so ordering and cycling cannot be
asserted deterministically without a pure function over synthetic ifaces.

### Resolution and cycling

```rust
fn selected_index(&self, entries: &[UrlEntry]) -> usize   // key lookup, else 0
fn cycle(&mut self, delta: isize)                          // rem_euclid wrap
```

`primary_url()` becomes `entries[selected_index].url`, keeping its signature
so existing callers are untouched.

A selected address that disappears (interface down, mDNS switched off) falls
back to index 0 for display **without clearing `self.sel`**, so the
selection snaps back when that address returns.

`cycle` is a no-op when fewer than two entries exist. On each switch it logs
`note("primary address: <url>")`, consistent with every other toggle.

### Keys and rendering

- `handle_key`: `KeyCode::Tab` → `cycle(1)`, `KeyCode::BackTab` → `cycle(-1)`.
- One deliberate exception to the popup rule (`tui.rs:187`, where any key
  closes the popup): Tab and BackTab do not close the QR popup, they cycle
  inside it, and the QR redraws in place for the new address. Every other
  key still closes it.
- `draw.rs` computes `entries` and `sel` once per frame and feeds the header
  list, the side panel and the popup. Today `url_lines()` plus three
  `primary_url()` calls hit `getifaddrs` about four times per frame; this
  drops it to one.
- `url_lines()` stays as a public formatter over the entries, placing `➜` on
  the selected index and a space elsewhere. Line format is unchanged.
- Help popup gains `Tab/⇧Tab  switch primary address`.
- Bar hint becomes `[Q]r [?]help [Tab]addr [q]uit`. Tab stays out of
  `hotbar()`, which models on/off toggles.

### Non-goals

Non-TUI mode prints its banner once and exits into serving, so it has no
selection to cycle; `main.rs` is untouched. No CLI flag or config key for a
preferred interface — ranking plus Tab covers the need.

## Testing

Deterministic tests against `entries_from` with synthetic ifaces:

- ordering: mDNS entry first when announcing, then ranked interfaces;
- wrap in both directions, including single-entry no-op;
- sticky selection across a scheme flip and a base/token change;
- fallback to index 0 when the selected interface vanishes, and restore when
  it returns.

Two rendering tests in the existing `TestBackend` style:

- `Tab` moves the `➜` marker in the rendered header;
- the QR popup's title URL follows the selection after `Tab`.
