# TUI Address Cycling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the fshare TUI operator cycle the primary share address with `Tab` / `Shift+Tab`, with the `➜` marker, the side-panel QR and the `Q` popup all following the selection.

**Architecture:** Replace the "primary address = `ranked_ifaces()[0]`" assumption with an explicit list of `UrlEntry` values built by one pure function, plus an `Option<SelKey>` selection on `App` that is resolved to an index on every use. `SelKey` carries only the address identity (mDNS or `IpAddr`), never scheme/port/base, so the existing `s`/`t`/`a` toggles cannot disturb it. `draw.rs` builds the entry list once per frame and feeds all three consumers.

**Tech Stack:** Rust 2021, ratatui + crossterm (TUI), `if-addrs` via `crate::net::ranked_ifaces()`, `qrcode` crate, `cargo test` / `cargo clippy`.

**Spec:** `docs/superpowers/specs/2026-08-20-tui-address-cycling-design.md`

## Global Constraints

- All work happens in `src/tui.rs` and `src/tui/draw.rs`. `src/main.rs` (non-TUI banner) is out of scope — it prints once and never re-renders.
- No new dependencies. No new CLI flag or config key.
- **Do not run `cargo fmt`.** This repo is not formatted with default rustfmt settings; running it rewrites hundreds of unrelated lines. Match the surrounding style by hand (100-col-ish, same brace and wrapping habits).
- `cargo clippy --all-targets` must report zero warnings after every task.
- Existing tests must keep passing unchanged, in particular `url_lines_list_all_interfaces_with_live_base`, `virtual_ifaces_filtered`, `qr_popup_renders` and `qr_side_panel_renders_when_wide`.
- Public method signatures `App::primary_url(&self) -> String` and `App::url_lines(&self) -> Vec<String>` stay as they are; only their bodies change.
- Tests must be deterministic: assertions about ordering, cycling and markers go through the pure helpers with synthetic interfaces, never through the host's real interface list.

---

### Task 1: Entry list model and pure builder

Replaces the string-building inside `url_lines()` with a structured entry list. No selection yet — index 0 stays primary, so behavior is unchanged except that exactly one `➜` marker is rendered (today both the mDNS line and the first interface line get one).

**Files:**
- Modify: `src/tui.rs` (add types + `entries_from` + `url_entries` + `lines_from`; rewrite `url_lines` at `src/tui.rs:123-161` and `primary_url` at `src/tui.rs:107-116`)
- Test: `src/tui.rs` (`mod tests` at the bottom of the same file)

**Interfaces:**
- Consumes: `crate::net::{ranked_ifaces, rank, Iface, IfaceKind}`, `crate::mdns::host_label`, existing private `is_virtual_iface(name: &str) -> bool` at `src/tui.rs:326`.
- Produces:
  - `pub enum SelKey { Mdns, Ip(IpAddr) }` — derives `Clone, PartialEq, Eq, Debug`
  - `pub struct UrlEntry { pub url: String, pub label: String, pub key: SelKey }`
  - `fn entries_from(all: &[crate::net::Iface], mdns_on: bool, scheme: &str, port: u16, base: &str) -> Vec<UrlEntry>` — never returns an empty vec
  - `pub fn url_entries(&self) -> Vec<UrlEntry>` on `App`
  - `pub(super) fn lines_from(entries: &[UrlEntry], sel: usize) -> Vec<String>` — associated function on `App`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/tui.rs`, right after the `fn key(c: char) -> KeyEvent` helper (`src/tui.rs:444`):

```rust
    fn iface(name: &str, ip: &str) -> crate::net::Iface {
        let ip: IpAddr = ip.parse().unwrap();
        crate::net::Iface { name: name.to_string(), ip, kind: crate::net::rank(ip) }
    }

    #[test]
    fn entries_list_mdns_first_then_ranked_ifaces() {
        let all = [
            iface("wlan0", "192.168.1.112"),
            iface("eth0", "172.23.246.136"),
            iface("docker0", "172.17.0.1"),
            iface("lo", "127.0.0.1"),
        ];
        let e = entries_from(&all, true, "http", 8000, "");
        assert_eq!(e.len(), 3, "docker0 and lo are hidden: {e:?}");
        assert_eq!(e[0].key, SelKey::Mdns);
        assert_eq!(e[0].label, "mDNS");
        assert!(e[0].url.contains(".local:8000/"));
        assert_eq!(e[1].key, SelKey::Ip("192.168.1.112".parse().unwrap()));
        assert_eq!(e[1].url, "http://192.168.1.112:8000/");
        assert_eq!(e[1].label, "LAN, wlan0");
        assert_eq!(e[2].label, "LAN, eth0");

        // scheme and base flow through; mDNS off drops the first entry
        let e = entries_from(&all, false, "https", 9000, "/s/tok");
        assert_eq!(e[0].url, "https://192.168.1.112:9000/s/tok/");
        assert!(e.iter().all(|x| x.key != SelKey::Mdns));

        // nothing physical: fall back to whatever is left rather than nothing
        let only_virtual = [iface("docker0", "172.17.0.1")];
        let e = entries_from(&only_virtual, false, "http", 8000, "");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].label, "LAN, docker0");

        // nothing at all: a localhost entry, never an empty list
        let e = entries_from(&[], false, "http", 8000, "");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].url, "http://localhost:8000/");
    }

    #[test]
    fn marker_marks_only_the_selected_line() {
        let all = [iface("wlan0", "192.168.1.112"), iface("eth0", "172.23.246.136")];
        let entries = entries_from(&all, true, "http", 8000, "");
        let lines = App::lines_from(&entries, 2);
        assert_eq!(
            lines.iter().filter(|l| l.starts_with('➜')).count(),
            1,
            "exactly one primary marker: {lines:?}"
        );
        assert_eq!(lines[2], "➜ http://172.23.246.136:8000/    (LAN, eth0)");
        assert!(lines[0].starts_with("  http"), "unselected lines are indented");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib tui::tests::entries_list_mdns_first_then_ranked_ifaces tui::tests::marker_marks_only_the_selected_line`

Expected: compile error — `cannot find function 'entries_from' in this scope`, `cannot find type 'SelKey'`, `no function or associated item named 'lines_from' found for struct 'App'`.

- [ ] **Step 3: Add the types and the pure builder**

In `src/tui.rs`, directly after the `Popup` enum (`src/tui.rs:25-30`), add:

```rust
/// Identity of a selectable address. Carries no scheme, port or base, so
/// the secure/token/auth toggles never disturb a selection.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SelKey {
    Mdns,
    Ip(IpAddr),
}

/// One shareable address: the full URL, the trailing "(…)" label and the
/// identity used to keep a selection pinned across list rebuilds.
#[derive(Debug)]
pub struct UrlEntry {
    pub url: String,
    pub label: String,
    pub key: SelKey,
}

/// Builds the shareable-address list: mDNS name first when announcing, then
/// the interfaces that matter — loopback and virtual interfaces (docker
/// bridges, veth pairs, VM nets) are hidden unless nothing else exists.
/// Pure so ordering and selection can be tested with synthetic interfaces.
/// Never returns an empty list.
fn entries_from(
    all: &[crate::net::Iface],
    mdns_on: bool,
    scheme: &str,
    port: u16,
    base: &str,
) -> Vec<UrlEntry> {
    let mut v = Vec::new();
    if mdns_on {
        v.push(UrlEntry {
            url: format!("{scheme}://{}.local:{port}{base}/", crate::mdns::host_label()),
            label: "mDNS".to_string(),
            key: SelKey::Mdns,
        });
    }
    let mut ifaces: Vec<&crate::net::Iface> = all
        .iter()
        .filter(|i| i.kind != crate::net::IfaceKind::Loopback && !is_virtual_iface(&i.name))
        .collect();
    if ifaces.is_empty() {
        // nothing physical: fall back to whatever exists rather than none
        ifaces = all.iter().filter(|i| i.kind != crate::net::IfaceKind::Loopback).collect();
    }
    for ifc in ifaces {
        let host = match ifc.ip {
            IpAddr::V6(v6) => format!("[{v6}]"),
            IpAddr::V4(v4) => v4.to_string(),
        };
        let kind = match ifc.kind {
            crate::net::IfaceKind::Lan => "LAN, ",
            _ => "",
        };
        v.push(UrlEntry {
            url: format!("{scheme}://{host}:{port}{base}/"),
            label: format!("{kind}{}", ifc.name),
            key: SelKey::Ip(ifc.ip),
        });
    }
    if v.is_empty() {
        v.push(UrlEntry {
            url: format!("{scheme}://localhost:{port}{base}/"),
            label: "local".to_string(),
            key: SelKey::Ip(IpAddr::from([127, 0, 0, 1])),
        });
    }
    v
}
```

- [ ] **Step 4: Rewrite `primary_url` and `url_lines` on top of the entries**

Replace the whole of `primary_url` (`src/tui.rs:107-116`) and `url_lines` (`src/tui.rs:123-161`) — keep the doc comment style shown here:

```rust
    /// The live entry list: mDNS name first when announcing, then the
    /// interfaces that matter. Rebuilt on demand so toggles show up at once.
    pub fn url_entries(&self) -> Vec<UrlEntry> {
        entries_from(
            &crate::net::ranked_ifaces(),
            self.state.live.mdns.load(Relaxed),
            self.scheme(),
            self.port,
            &self.state.base(),
        )
    }

    pub fn primary_url(&self) -> String {
        let entries = self.url_entries();
        let sel = self.selected_index(&entries);
        // entries_from never returns an empty list
        entries[sel].url.clone()
    }

    /// One line per shareable URL, `➜` on the primary one.
    pub fn url_lines(&self) -> Vec<String> {
        let entries = self.url_entries();
        let sel = self.selected_index(&entries);
        Self::lines_from(&entries, sel)
    }

    pub(super) fn lines_from(entries: &[UrlEntry], sel: usize) -> Vec<String> {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let marker = if i == sel { "➜" } else { " " };
                format!("{marker} {}    ({})", e.url, e.label)
            })
            .collect()
    }
```

`selected_index` does not exist yet — add this temporary version immediately after `lines_from`; Task 2 replaces its body:

```rust
    pub fn selected_index(&self, _entries: &[UrlEntry]) -> usize {
        0
    }
```

- [ ] **Step 5: Run the full test suite**

Run: `cargo test --lib`

Expected: PASS, 73 tests. If `url_lines_list_all_interfaces_with_live_base` fails on `lines[0].starts_with('➜')`, the marker logic is wrong — with mDNS on, entry 0 is the mDNS line and must carry the marker.

- [ ] **Step 6: Lint**

Run: `cargo clippy --all-targets 2>&1 | grep -c '^warning'`

Expected: `0`

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs
git commit -m "refactor(tui): build the URL list from structured entries

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Sticky selection state

Adds the selection itself: which entry is primary, how it survives list rebuilds, and how cycling moves it. Still no key bindings — Task 3 wires those.

**Files:**
- Modify: `src/tui.rs` (add `sel` field to `App` at `src/tui.rs:36-54` and to the initializer in `App::new` at `src/tui.rs:56-84`; replace the temporary `selected_index`; add `cycle_in` and `cycle`)
- Test: `src/tui.rs` (`mod tests`)

**Interfaces:**
- Consumes: `SelKey`, `UrlEntry`, `entries_from`, `App::url_entries` from Task 1.
- Produces:
  - `App.sel: Option<SelKey>` — private field, `None` means "follow the ranked best"
  - `pub fn selected_index(&self, entries: &[UrlEntry]) -> usize`
  - `fn cycle_in(&mut self, entries: &[UrlEntry], delta: isize)` — testable core
  - `fn cycle(&mut self, delta: isize)` — thin wrapper over the live entry list

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/tui.rs`, after the tests from Task 1:

```rust
    #[test]
    fn cycle_wraps_both_directions_and_noops_on_single_entry() {
        let all = [iface("wlan0", "192.168.1.112"), iface("eth0", "172.23.246.136")];
        let entries = entries_from(&all, true, "http", 8000, ""); // mDNS + 2 ifaces
        assert_eq!(entries.len(), 3);
        let mut app = test_app(None, false);
        assert_eq!(app.selected_index(&entries), 0, "defaults to the ranked best");

        app.cycle_in(&entries, 1);
        assert_eq!(app.selected_index(&entries), 1);
        app.cycle_in(&entries, 1);
        assert_eq!(app.selected_index(&entries), 2);
        app.cycle_in(&entries, 1);
        assert_eq!(app.selected_index(&entries), 0, "wraps forward");
        app.cycle_in(&entries, -1);
        assert_eq!(app.selected_index(&entries), 2, "wraps backward");

        // switching logs a line, so the operator sees what changed
        assert!(app.log.iter().any(|l| l.contains("primary address:")));

        let single = entries_from(&[iface("wlan0", "192.168.1.112")], false, "http", 8000, "");
        let before = app.sel.clone();
        app.cycle_in(&single, 1);
        assert_eq!(app.sel, before, "single entry: selection untouched");
    }

    #[test]
    fn selection_survives_scheme_base_and_iface_churn() {
        let all = [iface("wlan0", "192.168.1.112"), iface("eth0", "172.23.246.136")];
        let mut app = test_app(None, false);
        let plain = entries_from(&all, false, "http", 8000, "");
        app.cycle_in(&plain, 1); // select eth0
        assert_eq!(app.selected_index(&plain), 1);

        // secure bundle flips scheme and adds a token base: same selection
        let secure = entries_from(&all, false, "https", 8000, "/s/tok");
        assert_eq!(app.selected_index(&secure), 1);
        assert_eq!(secure[1].url, "https://172.23.246.136:8000/s/tok/");

        // the selected interface drops: fall back to the first entry
        let gone = entries_from(&[iface("wlan0", "192.168.1.112")], false, "http", 8000, "");
        assert_eq!(app.selected_index(&gone), 0);

        // …and come back to it when the interface returns
        assert_eq!(app.selected_index(&plain), 1, "choice is not forgotten");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib tui::tests::cycle_wraps_both_directions_and_noops_on_single_entry tui::tests::selection_survives_scheme_base_and_iface_churn`

Expected: compile error — `no method named 'cycle_in' found for struct 'App'` and `no field 'sel' on type 'App'`.

- [ ] **Step 3: Add the field**

In the `App` struct (`src/tui.rs:36-54`), add after `popup: Popup,`:

```rust
    /// Manually chosen primary address; `None` follows the ranked best.
    sel: Option<SelKey>,
```

In `App::new` (`src/tui.rs:56-84`), add to the struct literal after `popup: Popup::None,`:

```rust
            sel: None,
```

- [ ] **Step 4: Implement resolution and cycling**

Replace the temporary `selected_index` from Task 1 with the real one, and add the two cycling methods next to it:

```rust
    /// Resolves the selection against a freshly built list. A selected
    /// address that is momentarily gone (interface down, mDNS off) falls back
    /// to the ranked best *without* clearing `sel`, so it snaps back when the
    /// address returns.
    pub fn selected_index(&self, entries: &[UrlEntry]) -> usize {
        self.sel
            .as_ref()
            .and_then(|k| entries.iter().position(|e| &e.key == k))
            .unwrap_or(0)
    }

    /// Cycling core, split out so it can be tested with synthetic entries
    /// instead of whatever interfaces the test machine happens to have.
    fn cycle_in(&mut self, entries: &[UrlEntry], delta: isize) {
        if entries.len() < 2 {
            return;
        }
        let cur = self.selected_index(entries) as isize;
        let next = (cur + delta).rem_euclid(entries.len() as isize) as usize;
        self.sel = Some(entries[next].key.clone());
        let url = entries[next].url.clone();
        self.note(&format!("primary address: {url}"));
    }

    fn cycle(&mut self, delta: isize) {
        let entries = self.url_entries();
        self.cycle_in(&entries, delta);
    }
```

- [ ] **Step 5: Run the full test suite**

Run: `cargo test --lib`

Expected: PASS, 75 tests.

- [ ] **Step 6: Lint**

Run: `cargo clippy --all-targets 2>&1 | grep -c '^warning'`

Expected: `0`. A `dead_code` warning on `cycle` is expected to be absent because `cycle_in` is used by tests and `cycle` is used in Task 3 — if clippy flags `cycle` as never used, add `#[allow(dead_code)]` **only** if Task 3 is not being done in the same session; otherwise proceed to Task 3 and the warning disappears.

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs
git commit -m "feat(tui): sticky primary-address selection

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Tab / Shift+Tab key bindings

**Files:**
- Modify: `src/tui.rs` (`handle_key` at `src/tui.rs:186-231`)
- Modify: `src/tui/draw.rs` (bar hint at `src/tui/draw.rs:126-129`, help popup at `src/tui/draw.rs:191-210`)
- Test: `src/tui.rs` (`mod tests`)

**Interfaces:**
- Consumes: `App::cycle(&mut self, delta: isize)` from Task 2.
- Produces: `Tab` → next address, `BackTab` (Shift+Tab) → previous, both live and inside the QR popup.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/tui.rs`, and add the key helpers next to `fn key(c: char)`:

```rust
    fn tab() -> KeyEvent {
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    fn back_tab() -> KeyEvent {
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
    }

    #[test]
    fn tab_switches_primary_address() {
        let mut app = test_app(None, false);
        if app.url_entries().len() < 2 {
            return; // single-homed machine: nothing to cycle to
        }
        let before = app.primary_url();
        app.handle_key(tab());
        let after = app.primary_url();
        assert_ne!(after, before, "Tab moves to the next address");
        assert!(app.log.iter().any(|l| l.contains("primary address:")));
        app.handle_key(back_tab());
        assert_eq!(app.primary_url(), before, "Shift+Tab moves back");
    }

    #[test]
    fn tab_cycles_inside_the_qr_popup_without_closing_it() {
        let mut app = test_app(None, false);
        app.handle_key(key('Q'));
        assert!(app.popup == Popup::Qr);
        app.handle_key(tab());
        assert!(app.popup == Popup::Qr, "Tab keeps the QR popup open");
        app.handle_key(back_tab());
        assert!(app.popup == Popup::Qr, "Shift+Tab keeps the QR popup open");
        // any other key still closes it
        app.handle_key(key('u'));
        assert!(app.popup == Popup::None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib tui::tests::tab_switches_primary_address tui::tests::tab_cycles_inside_the_qr_popup_without_closing_it`

Expected: `tab_cycles_inside_the_qr_popup_without_closing_it` FAILS with `assertion failed: app.popup == Popup::Qr` (today any key closes the popup). `tab_switches_primary_address` passes vacuously on a single-homed box and fails on `assert_ne!` otherwise.

- [ ] **Step 3: Handle the keys**

In `handle_key` (`src/tui.rs:186`), replace the popup early-return block:

```rust
        // popups cover the screen: the closing key is swallowed
        if self.popup != Popup::None {
            self.popup = Popup::None;
            self.notice = None;
            return Action::None;
        }
```

with:

```rust
        // popups cover the screen: the closing key is swallowed. Tab is the
        // exception — it cycles the address so the QR redraws in place.
        if self.popup != Popup::None {
            match key.code {
                KeyCode::Tab => {
                    self.cycle(1);
                    return Action::None;
                }
                KeyCode::BackTab => {
                    self.cycle(-1);
                    return Action::None;
                }
                _ => {}
            }
            self.popup = Popup::None;
            self.notice = None;
            return Action::None;
        }
```

Then in the main `match key.code` block, add these two arms directly above `KeyCode::Up => self.scroll_by(1),`:

```rust
            KeyCode::Tab => self.cycle(1),
            KeyCode::BackTab => self.cycle(-1),
```

- [ ] **Step 4: Update the hotkey bar hint and the help popup**

In `src/tui/draw.rs:126-129`, change the trailing hint span:

```rust
    spans.push(Span::styled(
        "  [Q]r [?]help [Tab]addr [q]uit",
        Style::default().fg(Color::DarkGray),
    ));
```

In `draw_help_popup` (`src/tui/draw.rs:191`), add the Tab line after the `Q` line and grow the box — the current 12-row box already clips the last line, and this adds a twelfth line of text:

```rust
 Q  QR code popup
 Tab / ⇧Tab  switch primary address
 ↑↓ PgUp PgDn  scroll log
 q / x / Ctrl+C  quit";
    let r = centered(f.area(), 48, 14);
```

- [ ] **Step 5: Run the full test suite**

Run: `cargo test --lib`

Expected: PASS, 77 tests.

- [ ] **Step 6: Lint**

Run: `cargo clippy --all-targets 2>&1 | grep -c '^warning'`

Expected: `0`

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs src/tui/draw.rs
git commit -m "feat(tui): Tab cycles the primary address

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: One entry list per frame

`draw` currently calls `app.url_lines()` once and `app.primary_url()` three times, so a single frame hits `getifaddrs` about four times and could in principle render a marker and a QR built from two different snapshots. Build the list once and pass it down.

**Files:**
- Modify: `src/tui/draw.rs` (`draw` at `src/tui/draw.rs:12-52`, `draw_qr_popup` at `src/tui/draw.rs:170-187`, popup dispatch at `src/tui/draw.rs:133-137`)
- Test: `src/tui.rs` (`mod tests`)

**Interfaces:**
- Consumes: `App::url_entries`, `App::selected_index`, `App::lines_from` from Tasks 1-2.
- Produces: `fn draw_qr_popup(f: &mut Frame, url: &str)` — signature change from `(f, app)`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/tui.rs`:

```rust
    #[test]
    fn qr_popup_title_follows_selection() {
        let mut app = test_app(None, false);
        app.handle_key(key('Q'));
        app.handle_key(tab()); // no-op on a single-homed box, still valid
        let url = app.primary_url();
        let backend = TestBackend::new(160, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains(&url), "popup title shows the selected URL: {url}");
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test --lib tui::tests::qr_popup_title_follows_selection`

Expected: PASS already (the popup reads `primary_url()` today). This test is a regression guard for the refactor in Step 3 — it must still pass afterwards. If it fails now, the terminal size is too small for the QR; raise `TestBackend::new` dimensions rather than weakening the assertion.

- [ ] **Step 3: Compute the entries once per frame**

In `src/tui/draw.rs`, replace line 14 (`let urls = app.url_lines();`) with:

```rust
    let entries = app.url_entries();
    let sel = app.selected_index(&entries);
    let primary = entries[sel].url.clone(); // entries_from never returns empty
    let urls = App::lines_from(&entries, sel);
```

In the side-panel block, replace `app.primary_url()` in both places (`src/tui/draw.rs:28` and `src/tui/draw.rs:37`):

```rust
        if let Some(q) = qr_text(&primary) {
```

```rust
                let url = primary.clone();
```

In the popup dispatch (`src/tui/draw.rs:134`):

```rust
        Popup::Qr => draw_qr_popup(f, &primary),
```

And change `draw_qr_popup` (`src/tui/draw.rs:170-173`) from taking the app to taking the URL:

```rust
fn draw_qr_popup(f: &mut Frame, url: &str) {
    let Some(rendered) = qr_text(url) else {
        return;
    };
```

The `.title(format!(" {url} "))` call at the end of that function needs no change.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test --lib`

Expected: PASS, 78 tests. `qr_side_panel_renders_when_wide` and `qr_popup_renders` must still pass — they exercise exactly this code path.

- [ ] **Step 5: Lint**

Run: `cargo clippy --all-targets 2>&1 | grep -c '^warning'`

Expected: `0`

- [ ] **Step 6: Try it by hand**

Run: `cargo run -- --tui .` then press `Tab` a few times, then `Q`, then `Tab` inside the popup, then `?`.

Expected: the `➜` marker moves down the header list one line per press and wraps; the side-panel QR and its cyan caption change with it; a `primary address: …` line appears in the log pane; inside the `Q` popup Tab redraws the QR and its title without closing it; the help popup lists `Tab / ⇧Tab  switch primary address` with no clipped last line.

- [ ] **Step 7: Commit**

```bash
git add src/tui.rs src/tui/draw.rs
git commit -m "perf(tui): build the address list once per frame

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| `SelKey` / `UrlEntry` data model | 1 |
| `entries_from` rules 1-4 (mDNS first, virtual/loopback filter, fallbacks) | 1 |
| `url_entries` wrapper | 1 |
| `primary_url` via selection, signature preserved | 1 (index 0) → 2 (real selection) |
| `url_lines` formatter, single `➜` | 1 |
| `sel: Option<SelKey>`, `None` = ranked best | 2 |
| `selected_index`, fallback without clearing `sel` | 2 |
| `cycle` wrap, single-entry no-op, `note(...)` log | 2 |
| Tab / BackTab bindings | 3 |
| Popup exception (Tab cycles, others close) | 3 |
| Help popup line, bar hint | 3 |
| One `getifaddrs` per frame | 4 |
| Non-goals (`main.rs`, no CLI flag) | Global Constraints |
| Test list (ordering, wrap, sticky, fallback, marker, popup title) | 1-4 |

No spec requirement is unassigned.

**Placeholder scan:** none — every code step carries complete code, every run step an exact command and expected output.

**Type consistency:** `SelKey`, `UrlEntry`, `entries_from`, `url_entries`, `selected_index`, `lines_from`, `cycle_in`, `cycle` are spelled identically in Tasks 1-4 and in the test code. `selected_index` is deliberately introduced as a stub in Task 1 Step 4 and replaced in Task 2 Step 4; both tasks say so explicitly. `draw_qr_popup`'s signature change is declared in Task 4's Interfaces block and is its only caller-visible change.
