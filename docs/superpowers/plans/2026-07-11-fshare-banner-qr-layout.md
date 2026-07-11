# fshare Banner QR Side-by-Side Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** QR left of the address list when terminal wide enough; exact current layout otherwise.

**Architecture:** New `src/banner.rs` with pure `fits()` + `side_by_side()` (unit-tested, ANSI-safe via plain/colored line pairs). `print_banner` collects address lines as `(plain, colored)` pairs, decides layout via `terminal_size`.

**Tech Stack:** `terminal_size` crate.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-11-fshare-banner-qr-layout-design.md`.
- Width math on PLAIN text (colors would inflate byte length); print COLORED.
- Fallback path byte-identical to current output.
- Gap = 2 spaces. Condition: tty && QR on && cols >= qr_w + 2 + addr_w.

---

### Task 1: banner.rs helpers

**Files:**
- Create: `src/banner.rs`; Modify: `src/lib.rs` (`pub mod banner;`), `Cargo.toml` (`cargo add terminal_size`)

**Interfaces:**
- Produces:
  - `banner::fits(term_cols: usize, qr_w: usize, addr_w: usize) -> bool`
  - `banner::side_by_side(qr: &[String], addr: &[(String, String)]) -> Vec<String>` — `addr` = (plain, colored); returns printable lines, left column constant width (QR width), 2-space gap, trailing whitespace trimmed.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_boundary() {
        assert!(fits(80, 40, 38)); // 40+2+38 == 80
        assert!(!fits(79, 40, 38));
    }

    fn qr3() -> Vec<String> {
        vec!["  ███".into(), "  █ █".into(), "  ███".into()]
    }

    #[test]
    fn zips_addr_shorter() {
        let addr = vec![("a".into(), "A".into())];
        let out = side_by_side(&qr3(), &addr);
        assert_eq!(out, vec!["  ███  A", "  █ █", "  ███"]);
    }

    #[test]
    fn zips_addr_longer() {
        let addr: Vec<_> =
            (0..5).map(|i| (format!("p{i}"), format!("C{i}"))).collect();
        let out = side_by_side(&qr3(), &addr);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], "  ███  C0");
        // rows past the QR pad the left column to QR width
        assert_eq!(out[3], "       C3");
        assert_eq!(out[4], "       C4");
    }

    #[test]
    fn empty_qr_degenerates() {
        let addr = vec![("p".into(), "C".into())];
        assert_eq!(side_by_side(&[], &addr), vec!["  C"]);
    }
}
```

Wait — empty QR: qr_w 0, left "" pad 0, gap 2 → "  C". OK as written.

- [ ] **Step 2: Implement**

```rust
pub fn fits(term_cols: usize, qr_w: usize, addr_w: usize) -> bool {
    term_cols >= qr_w + 2 + addr_w
}

/// `qr` lines are pre-indented and rectangular; `addr` is (plain, colored).
/// Widths are computed from plain text; colored text is what gets printed.
pub fn side_by_side(qr: &[String], addr: &[(String, String)]) -> Vec<String> {
    let qr_w = qr.first().map(|l| l.chars().count()).unwrap_or(0);
    let height = qr.len().max(addr.len());
    (0..height)
        .map(|i| {
            let left = qr.get(i).map(String::as_str).unwrap_or("");
            let pad = qr_w.saturating_sub(left.chars().count());
            let right = addr.get(i).map(|(_, c)| c.as_str()).unwrap_or("");
            format!("{left}{}  {right}", " ".repeat(pad)).trim_end().to_string()
        })
        .collect()
}
```

- [ ] **Step 3:** `cargo test banner::` PASS.
- [ ] **Step 4:** `git commit -am "feat: side-by-side banner layout helpers"`

---

### Task 2: print_banner integration

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `banner::{fits, side_by_side}`, `terminal_size::terminal_size()`.

- [ ] **Step 1: Refactor `print_banner`** — replace the mDNS line + iface loop + QR section with:

```rust
    // collect address lines as (plain-for-width, colored-for-print)
    let mut addr_lines: Vec<(String, String)> = Vec::new();
    if mdns_on {
        let plain = format!("➜ {scheme}://fshare.local:{port}{}/    (mDNS)", state.base);
        let colored =
            format!("{} {scheme}://fshare.local:{port}{}/    (mDNS)", "➜".green(), state.base);
        addr_lines.push((plain, colored));
    }
    let ifaces = net::ranked_ifaces();
    let mut best_url = None;
    for (i, ifc) in ifaces.iter().enumerate() {
        let host = match ifc.ip {
            IpAddr::V6(v6) => format!("[{v6}]"),
            IpAddr::V4(v4) => v4.to_string(),
        };
        let url = format!("{scheme}://{host}:{port}{}/", state.base);
        let kind = match ifc.kind {
            net::IfaceKind::Lan => "LAN, ",
            _ => "",
        };
        let marker_plain = if i == 0 { "➜" } else { " " };
        let marker_col = if i == 0 { "➜".green().to_string() } else { " ".to_string() };
        addr_lines.push((
            format!("{marker_plain} {url:40} ({kind}{})", ifc.name),
            format!("{marker_col} {url:40} ({kind}{})", ifc.name),
        ));
        if i == 0 {
            best_url = Some(url);
        }
    }

    let show_qr = !args.no_qr && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let qr_lines: Vec<String> = if show_qr {
        best_url
            .as_ref()
            .and_then(|url| qrcode::QrCode::new(url.as_bytes()).ok())
            .map(|code| {
                code.render::<qrcode::render::unicode::Dense1x2>()
                    .quiet_zone(true)
                    .build()
                    .lines()
                    .map(|l| format!("  {l}"))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let qr_w = qr_lines.first().map(|l| l.chars().count()).unwrap_or(0);
    let addr_w = addr_lines.iter().map(|(p, _)| 2 + p.chars().count()).max().unwrap_or(0);
    let cols = terminal_size::terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(0);

    if !qr_lines.is_empty() && fshare::banner::fits(cols, qr_w, addr_w) {
        for line in fshare::banner::side_by_side(&qr_lines, &addr_lines) {
            println!("{line}");
        }
    } else {
        for (_, colored) in &addr_lines {
            println!("  {colored}");
        }
        if !qr_lines.is_empty() {
            println!();
            for l in &qr_lines {
                println!("{l}");
            }
        }
    }
```

Notes:
- `addr_w` includes the 2-space indent the fallback prints; in side-by-side the gap supplies separation, plain width + 2 keeps the fit check conservative.
- Old QR print block and old loop removed; `indent()` helper deleted if now unused.
- Fallback prints `  {colored}` — identical to previous output.

- [ ] **Step 2:** `cargo test` all green; `cargo clippy --all-targets -- -D warnings`.
- [ ] **Step 3: Manual smoke** — wide terminal run shows QR left of addresses; `COLUMNS`-narrow (pipe through `head`? no — non-tty skips QR; instead resize or check fallback via forcing small cols is manual) at minimum verify non-tty output unchanged and wide-tty side-by-side via `script -qc` pty capture:

```bash
script -qec "./target/debug/fshare --port 18141 <dir> & sleep 1; kill %1" /dev/null | head -20
```

- [ ] **Step 4:** `git commit -am "feat: QR beside address list on wide terminals"`

---

## Self-Review Notes

- Spec coverage: condition (T2 fit check incl. tty+no_qr via `show_qr`/empty qr_lines), ANSI-safe widths (plain/colored pairs), fallback identical (T2 prints same strings as before), notes stay below (untouched), unit tests (T1).
- Type consistency: `side_by_side(&[String], &[(String,String)])` matches T1/T2.
