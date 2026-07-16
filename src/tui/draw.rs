//! Rendering for the fshare TUI: full-frame layout, QR panel and popups.
//! Pure functions of `&App` — no state mutation happens here.

use super::{App, Popup};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::sync::atomic::Ordering::Relaxed;

pub(super) fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let urls = app.url_lines();
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

    let mut logs = body;
    if app.show_qr {
        if let Some(q) = qr_text(&app.primary_url()) {
            let (qw, qh) = qr_size(&q);
            if body.width >= qw + 44 && body.height >= qh {
                let [left, right] =
                    Layout::horizontal([Constraint::Length(qw), Constraint::Min(0)]).areas(body);
                logs = right;
                // QR content sits at the top; the block border spans the
                // whole column so it lines up with the log pane's bottom
                let mut text = ratatui::text::Text::raw(q);
                let url = app.primary_url();
                let inner_w = (qw - 6) as usize; // borders + padding
                let url_rows = 1 + url.len().div_ceil(inner_w) as u16;
                if left.height >= qh + url_rows {
                    // the URL the QR encodes, for humans
                    text.push_line(Line::raw(""));
                    text.push_line(Line::styled(url, Style::default().fg(Color::Cyan)));
                }
                f.render_widget(
                    Paragraph::new(text)
                        .wrap(ratatui::widgets::Wrap { trim: false })
                        .block(qr_block()),
                    left,
                );
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
        "  [Q]r [?]help [q]uit",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), bar);

    match app.popup {
        Popup::Qr => draw_qr_popup(f, app),
        Popup::Help => draw_help_popup(f),
        Popup::None => {}
    }
}

/// Compact QR: lowest error-correction level (fewer modules) and no
/// built-in quiet zone — the bordered block's padding provides the
/// light margin scanners need.
fn qr_text(url: &str) -> Option<String> {
    qrcode::QrCode::with_error_correction_level(url.as_bytes(), qrcode::EcLevel::L)
        .ok()
        .map(|c| c.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(false).build())
}

fn qr_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 1))
        .title(" QR ")
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

fn draw_qr_popup(f: &mut Frame, app: &App) {
    let url = app.primary_url();
    let Some(rendered) = qr_text(&url) else {
        return;
    };
    let (w, h) = qr_size(&rendered);
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
    f.render_widget(Paragraph::new(rendered).block(qr_block().title(format!(" {url} "))), r);
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
 ↑↓ PgUp PgDn  scroll log
 q / x / Ctrl+C  quit";
    let r = centered(f.area(), 46, 12);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" keys ")),
        r,
    );
}
