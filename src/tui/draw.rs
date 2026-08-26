//! Rendering for the fshare TUI: full-frame layout, QR panel and popups.
//! Pure functions of `&App` — no state mutation happens here.

use super::{App, Popup};
use ratatui::buffer::{Buffer, CellDiffOption, CellWidth};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::num::NonZeroU16;
use std::sync::atomic::Ordering::Relaxed;

pub(super) fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let entries = app.url_entries();
    let sel = app.selected_index(&entries);
    let primary = entries[sel].url.clone(); // entries_from never returns empty
    let urls = App::lines_from(&entries, sel);
    let header_h = urls.len() as u16 + 1 + app.notice.is_some() as u16 + 2;
    // header and hotkey bar span the full width (URLs are long); the QR
    // panel takes a right column of the log region only, bottom-aligned
    // so it sits flush with the hotkey bar
    let [header, body, bar] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area);

    // rects holding a URL verbatim, linked with OSC 8 once every widget has
    // been rendered
    let mut links: Vec<(Rect, String)> = Vec::new();

    let mut logs = body;
    if app.show_qr {
        if let Some(q) = qr_text(&primary) {
            let (qw, qh) = qr_size(&q);
            let url = primary.clone();
            let cw = qr_col(body.width, qw, url.chars().count());
            if let (Some(cw), true) = (cw, body.height >= qh) {
                let [left, right] =
                    Layout::horizontal([Constraint::Length(cw), Constraint::Min(0)]).areas(body);
                logs = right;
                // the block border spans the whole column so it lines up with
                // the log pane's bottom; the QR itself sits at the top
                let block = qr_block().title(" QR ");
                let inner = block.inner(left);
                f.render_widget(block, left);
                let qr_rows = qh - 4; // borders and padding
                f.render_widget(
                    Paragraph::new(q).alignment(Alignment::Center),
                    Rect { height: qr_rows.min(inner.height), ..inner },
                );
                // the URL the QR encodes, for humans — left aligned and its
                // own widget so a wrapped URL stays contiguous in the buffer,
                // which is what lets it be linked as one run
                if inner.height >= qr_rows + 2 {
                    let r = Rect {
                        y: inner.y + qr_rows + 1,
                        height: inner.height - qr_rows - 1,
                        ..inner
                    };
                    f.render_widget(
                        Paragraph::new(Line::styled(url.clone(), Style::default().fg(Color::Cyan)))
                            .wrap(Wrap { trim: false }),
                        r,
                    );
                    links.push((r, url));
                }
            }
        }
    }

    // header
    let title = if app.info.single_file {
        format!(" fshare v{} — sharing file {} ", env!("CARGO_PKG_VERSION"), app.info.root.display())
    } else {
        match *app.info.summary.lock().unwrap() {
            Some((files, bytes)) => format!(
                " fshare v{} — {} ({files} files, {}) ",
                env!("CARGO_PKG_VERSION"),
                app.info.root.display(),
                crate::listing::human_size(bytes),
            ),
            None => format!(
                " fshare v{} — {} (counting…) ",
                env!("CARGO_PKG_VERSION"),
                app.info.root.display(),
            ),
        }
    };
    let stats = &app.state.stats;
    let status = format!(
        "  {} clients   {} sent",
        stats.clients.lock().unwrap().len(),
        crate::listing::human_size(stats.bytes.load(Relaxed)),
    );
    let mut lines: Vec<Line> = urls
        .iter()
        .map(|u| {
            let style = if u.starts_with('➜') {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            Line::from(Span::styled(u.clone(), style))
        })
        .collect();
    lines.push(Line::from(Span::styled(status, Style::default().fg(Color::Cyan))));
    if let Some(n) = &app.notice {
        lines.push(Line::from(Span::styled(
            n.clone(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        header,
    );
    // one row per URL entry, inside the header's border
    for (i, e) in entries.iter().enumerate() {
        let y = header.y + 1 + i as u16;
        if y < header.y + header.height.saturating_sub(1) {
            links.push((
                Rect::new(header.x + 1, y, header.width.saturating_sub(2), 1),
                e.url.clone(),
            ));
        }
    }

    // log pane: last N visible lines honoring scroll offset
    let h = logs.height.saturating_sub(2) as usize; // borders
    let total = app.log.len();
    let end = total.saturating_sub(app.scroll);
    let start = end.saturating_sub(h);
    let text: Vec<Line> = app.log.iter().skip(start).take(end - start).map(|l| Line::raw(l.clone())).collect();
    let log_title = if app.scroll > 0 { format!(" log (scrolled ↑{}) ", app.scroll) } else { " log ".to_string() };
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(log_title)),
        logs,
    );

    // hotkey bar
    let mut spans: Vec<Span> = Vec::new();
    for (key, label, on) in app.hotbar() {
        let style = if on {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(
            format!(" [{key}]{label}:{}", if on { "on" } else { "off" }),
            style,
        ));
    }
    spans.push(Span::styled(
        "  [Q]r [?]help [Tab]addr [q]uit",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), bar);

    match app.popup {
        Popup::Qr => draw_qr_popup(f, &primary),
        Popup::Help => draw_help_popup(f),
        Popup::None => {}
    }

    // Links behind a popup are skipped. A popup covers a URL run only
    // partially, and Ratatui's diff redraws just the covered cells — the
    // opening sequence lives in the run's first cell, so a partial redraw
    // would leave the uncovered tail without it. Dropping every link
    // underneath while a popup is open makes each cell of those runs differ on
    // open and on close, so they are always rewritten as a unit. A run inside
    // the popup is safe: the popup's own cells are all rewritten together.
    if matches!(app.popup, Popup::None) {
        let buf = f.buffer_mut();
        for (r, url) in links {
            link_text(buf, r, &url);
        }
    }
}

/// Marks an already-rendered run of cells as an OSC 8 hyperlink to `url`.
///
/// Terminals otherwise guess where a URL ends by scanning the visible text,
/// which stops at a row break: `http://192.168.23.239:8000/` wrapped after
/// `:800` opened the truncated `http://192.168.23.239:800`. With the target
/// carried in the escape sequence the click resolves to the whole URL no
/// matter where the text wraps, and clipped text still opens in full.
///
/// The opening sequence goes into the run's first cell and the closing one
/// into its last, each pinned with [`CellDiffOption::ForcedWidth`] so Ratatui
/// keeps accounting for the cell as its visible width rather than the width of
/// the escape bytes. Terminals without OSC 8 support ignore both sequences and
/// fall back to their own detection.
fn link_text(buf: &mut Buffer, area: Rect, url: &str) {
    let area = area.intersection(*buf.area());
    if area.is_empty() || url.is_empty() {
        return;
    }
    // cells in reading order, so a URL wrapped onto the next row is still one
    // contiguous run
    let cells: Vec<(u16, u16)> =
        (area.y..area.y + area.height).flat_map(|y| (area.x..area.x + area.width).map(move |x| (x, y))).collect();
    let mut text = String::new();
    let mut starts = Vec::with_capacity(cells.len());
    for &(x, y) in &cells {
        starts.push(text.len());
        if let Some(c) = buf.cell((x, y)) {
            text.push_str(c.symbol());
        }
    }
    let Some(byte) = text.find(url) else {
        return; // clipped or not rendered here — leave the cells alone
    };
    let Some(first) = starts.iter().position(|&b| b == byte) else {
        return;
    };
    // the run ends on the last cell whose symbol starts inside the match
    let end = byte + url.len();
    let last = starts.iter().rposition(|&b| b < end).unwrap_or(first);

    if let Some(c) = buf.cell_mut(cells[first]) {
        let w = NonZeroU16::new(c.cell_width().max(1)).expect("max(1) is nonzero");
        let sym = format!("{}{}", osc8_open(url), c.symbol());
        c.set_symbol(&sym).set_diff_option(CellDiffOption::ForcedWidth(w));
    }
    if let Some(c) = buf.cell_mut(cells[last]) {
        let w = NonZeroU16::new(c.cell_width().max(1)).expect("max(1) is nonzero");
        let sym = format!("{}{OSC8_CLOSE}", c.symbol());
        c.set_symbol(&sym).set_diff_option(CellDiffOption::ForcedWidth(w));
    }
}

fn osc8_open(url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\")
}

const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";

/// Compact QR: lowest error-correction level (fewer modules) and no
/// built-in quiet zone — the bordered block's padding provides the
/// light margin scanners need.
fn qr_text(url: &str) -> Option<String> {
    qrcode::QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::L)
        .ok()
        .map(|c| c.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(false).build())
}

/// Bordered, padded frame for the QR — untitled, because `Block::title`
/// appends: the popup titles it with the URL and a built-in `" QR "` would
/// both sit in front of it and eat the columns it needs.
fn qr_block() -> Block<'static> {
    Block::default().borders(Borders::ALL).padding(ratatui::widgets::Padding::new(2, 2, 1, 1))
}

/// Width of the QR side column: wide enough for the QR itself *and* for the
/// URL on a single row. A URL that wraps breaks terminal link detection at the
/// row break — clicking `http://192.168.23.239:8000/` split after `:800` opens
/// the truncated `http://192.168.23.239:800` — so the column grows instead.
fn qr_col_width(qr_w: u16, url_chars: usize) -> u16 {
    qr_w.max(url_chars as u16 + 6) // borders (2) + padding (4)
}

/// Chosen width of the QR column, or `None` when the log pane's 44 columns
/// cannot be spared.
///
/// The URL is kept on one row where it fits, since that is the only form every
/// terminal handles. Where it does not, the column falls back to the bare QR
/// rather than dropping the panel: a wrapped URL still opens whole through the
/// OSC 8 link that [`link_text`] puts on it.
fn qr_col(body_w: u16, qr_w: u16, url_chars: usize) -> Option<u16> {
    let want = qr_col_width(qr_w, url_chars);
    if body_w >= want + 44 {
        Some(want)
    } else if body_w >= qr_w + 44 {
        Some(qr_w)
    } else {
        None
    }
}

/// Outer size of the QR panel including borders and padding.
fn qr_size(rendered: &str) -> (u16, u16) {
    let lines: Vec<&str> = rendered.lines().collect();
    let w = lines.first().map(|l| l.chars().count()).unwrap_or(0) as u16 + 2 + 4;
    let h = lines.len() as u16 + 2 + 2;
    (w, h)
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w.min(area.width), h.min(area.height))
}

fn draw_qr_popup(f: &mut Frame, url: &str) {
    let Some(rendered) = qr_text(url) else {
        return;
    };
    let (qw, h) = qr_size(&rendered);
    // the title carries the URL, so the popup has to be wide enough to spell
    // it out: a truncated title is both unreadable and unlinkable
    let w = qw.max(url.chars().count() as u16 + 4); // borders + a space each side
    let area = f.area();
    if w > area.width || h > area.height {
        let r = centered(area, 30, 3);
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new("terminal too small for QR").block(Block::default().borders(Borders::ALL)),
            r,
        );
        return;
    }
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(rendered)
            .alignment(Alignment::Center)
            .block(qr_block().title(format!(" {url} "))),
        r,
    );
    // the title is the only place the popup spells the URL out, so link it
    // too. Searching the top border row keeps any assumption about where
    // Block puts a title out of this: a title too long for the popup is
    // truncated, the search misses, and the row is left alone.
    link_text(f.buffer_mut(), Rect { height: 1, ..r }, url);
}

fn draw_help_popup(f: &mut Frame) {
    let text = "\
 s  secure bundle: auth + token on, mDNS off,
    TLS enabled live (plain connections drop)
 m  toggle mDNS announce
 u  toggle uploads
 a  toggle auth (generated password shown)
 t  toggle token URL (new token each enable)
 h  toggle hidden files
 z  toggle zip downloads
 Q  QR code popup
 Tab / ⇧Tab  switch primary address
 ↑↓ PgUp PgDn  scroll log
 q / x / Ctrl+C  quit";
    let r = centered(f.area(), 48, 14);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" keys ")),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Visible text: cell symbols with the OSC 8 sequences stripped back out.
    fn visible(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                let sym = buf.cell((x, y)).unwrap().symbol().to_string();
                let mut rest = sym.as_str();
                // drop "\x1b]8;;...\x1b\\" wherever it appears
                while let Some(i) = rest.find("\x1b]8;;") {
                    out.push_str(&rest[..i]);
                    match rest[i..].find("\x1b\\") {
                        Some(j) => rest = &rest[i + j + 2..], // ESC + backslash
                        None => return out,
                    }
                }
                out.push_str(rest);
            }
        }
        out
    }

    #[test]
    fn link_text_wraps_a_url_split_across_rows_in_one_run() {
        // "http://ab.co/" hard-wrapped at width 8 — the case a terminal's own
        // URL detection gets wrong, stopping at the row break
        let url = "http://ab.co/";
        let mut buf = Buffer::with_lines(["http://a", "b.co/   "]);
        let area = *buf.area();
        link_text(&mut buf, area, url);

        let first = buf.cell((0, 0)).unwrap();
        assert_eq!(first.symbol(), format!("{}h", osc8_open(url)), "opener leads the run");
        assert_eq!(
            first.diff_option,
            CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap()),
            "the escape bytes must not count toward the cell's width",
        );

        // the run ends on the last character of the URL, on the second row
        let last = buf.cell((4, 1)).unwrap();
        assert_eq!(last.symbol(), format!("/{OSC8_CLOSE}"), "closer ends the run");
        assert_eq!(last.diff_option, CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap()));

        assert_eq!(visible(&buf, area), "http://ab.co/   ", "visible text is unchanged");
    }

    #[test]
    fn link_text_leaves_a_clipped_url_alone() {
        // only part of the URL is on screen: no run to open and close
        let mut buf = Buffer::with_lines(["http://a"]);
        let area = *buf.area();
        link_text(&mut buf, area, "http://ab.co/");
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "h");
        assert_eq!(buf.cell((0, 0)).unwrap().diff_option, CellDiffOption::None);
    }

    #[test]
    fn link_text_ignores_an_empty_url() {
        let mut buf = Buffer::with_lines(["abc"]);
        let area = *buf.area();
        link_text(&mut buf, area, "");
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "abc".get(0..1).unwrap());
    }

    /// The panel used to be sized to the QR alone, so a URL longer than the
    /// QR wrapped and terminal link detection stopped at the row break.
    #[test]
    fn qr_column_fits_a_url_longer_than_the_qr() {
        // 27 chars: the case from the bug report, against a 25-col QR
        let url = "http://192.168.23.239:8000/";
        let w = qr_col_width(25, url.chars().count());
        assert!(
            w as usize >= url.chars().count() + 6,
            "column {w} must hold the {}-char URL plus borders and padding",
            url.chars().count(),
        );
    }

    #[test]
    fn qr_column_falls_back_to_the_bare_qr_when_the_url_will_not_fit() {
        let url_chars = "http://192.168.23.239:8000/".chars().count(); // 27
        // roomy: the column holds the URL on one row
        assert_eq!(qr_col(120, 25, url_chars), Some(33));
        // tight: not enough for a 33-wide column, but the bare QR still fits,
        // so the panel stays and the URL wraps instead of the QR disappearing
        assert_eq!(qr_col(72, 25, url_chars), Some(25));
        // no room for the log pane beside it: no panel
        assert_eq!(qr_col(68, 25, url_chars), None);
    }

    #[test]
    fn qr_column_never_shrinks_below_the_qr() {
        assert_eq!(qr_col_width(40, "http://a:1/".chars().count()), 40);
    }
}
