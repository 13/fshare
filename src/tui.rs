use crate::log;
use crate::server::AppState;
use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const LOG_CAP: usize = 1000;

/// Immutable share facts for the header line.
pub struct ShareInfo {
    pub root: std::path::PathBuf,
    pub single_file: bool,
    pub files: u64,
    pub bytes: u64,
}

#[derive(PartialEq)]
enum Popup {
    None,
    Qr,
    Help,
}

pub enum Action {
    None,
    Quit,
}

pub struct App {
    pub state: Arc<AppState>,
    scheme: &'static str,
    port: u16,
    info: ShareInfo,
    show_qr: bool, // side-panel QR when the terminal is wide enough
    log: VecDeque<String>,
    scroll: usize, // lines above the bottom; 0 = follow
    popup: Popup,
    mdns_guard: Option<crate::mdns::MdnsGuard>,
    notice: Option<String>,           // e.g. generated credentials, cleared on any key
    initial_auth: Option<String>,     // "user:pass" from CLI/config, reused on re-enable
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: Arc<AppState>,
        scheme: &'static str,
        port: u16,
        info: ShareInfo,
        show_qr: bool,
        mdns_guard: Option<crate::mdns::MdnsGuard>,
        initial_auth: Option<String>,
        seed_notes: Vec<String>,
    ) -> Self {
        let mut app = Self {
            state,
            scheme,
            port,
            info,
            show_qr,
            log: VecDeque::new(),
            scroll: 0,
            popup: Popup::None,
            mdns_guard,
            notice: None,
            initial_auth,
        };
        for n in seed_notes {
            app.push_line(n);
        }
        app
    }

    pub fn push_line(&mut self, line: String) {
        self.log.push_back(line);
        if self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
        if self.scroll > 0 {
            // keep the viewed window stable while scrolled back
            self.scroll = (self.scroll + 1).min(self.log.len().saturating_sub(1));
        }
    }

    fn note(&mut self, text: &str) {
        self.push_line(log::format_pretty(&log::Event::Setting { text: text.to_string() }));
    }

    pub fn primary_url(&self) -> String {
        let base = self.state.base();
        let host = crate::net::ranked_ifaces()
            .first()
            .map(|i| match i.ip {
                IpAddr::V6(v6) => format!("[{v6}]"),
                IpAddr::V4(v4) => v4.to_string(),
            })
            .unwrap_or_else(|| "localhost".to_string());
        format!("{}://{host}:{}{base}/", self.scheme, self.port)
    }

    /// One line per shareable URL: mDNS name first (when announcing), then
    /// the interfaces that matter — loopback and virtual interfaces
    /// (docker bridges, veth pairs, VM nets) are hidden unless nothing
    /// else exists. Reads the live base so token toggles update the list.
    pub fn url_lines(&self) -> Vec<String> {
        let base = self.state.base();
        let mut v = Vec::new();
        if self.state.live.mdns.load(Relaxed) {
            v.push(format!(
                "➜ {}://{}.local:{}{base}/    (mDNS)",
                self.scheme,
                crate::mdns::host_label(),
                self.port
            ));
        }
        let all = crate::net::ranked_ifaces();
        let mut ifaces: Vec<&crate::net::Iface> = all
            .iter()
            .filter(|i| i.kind != crate::net::IfaceKind::Loopback && !is_virtual_iface(&i.name))
            .collect();
        if ifaces.is_empty() {
            // nothing physical: fall back to whatever exists rather than none
            ifaces = all.iter().filter(|i| i.kind != crate::net::IfaceKind::Loopback).collect();
        }
        for (i, ifc) in ifaces.iter().enumerate() {
            let host = match ifc.ip {
                IpAddr::V6(v6) => format!("[{v6}]"),
                IpAddr::V4(v4) => v4.to_string(),
            };
            let kind = match ifc.kind {
                crate::net::IfaceKind::Lan => "LAN, ",
                _ => "",
            };
            let marker = if i == 0 { "➜" } else { " " };
            v.push(format!(
                "{marker} {}://{host}:{}{base}/    ({kind}{})",
                self.scheme, self.port, ifc.name
            ));
        }
        if v.is_empty() {
            v.push(format!("➜ {}", self.primary_url()));
        }
        v
    }

    /// (key, label, on) triples for the hotkey bar, in display order.
    pub fn hotbar(&self) -> Vec<(char, &'static str, bool)> {
        let l = &self.state.live;
        vec![
            ('m', "mdns", l.mdns.load(Relaxed)),
            ('u', "upload", l.upload.load(Relaxed)),
            ('a', "auth", l.auth().is_some()),
            ('t', "token", !l.base().is_empty()),
            ('h', "hidden", l.hidden.load(Relaxed)),
            ('d', "dirs", l.dir_sizes.load(Relaxed)),
            ('z', "zip", l.zip.load(Relaxed)),
        ]
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // any key clears transient overlays first
        if self.popup != Popup::None || self.notice.is_some() {
            self.popup = Popup::None;
            self.notice = None;
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('x') => return Action::Quit,
            KeyCode::Char('m') => self.toggle_mdns(),
            KeyCode::Char('u') => {
                let on = crate::live::toggle(&self.state.live.upload);
                self.note(if on { "upload enabled" } else { "upload disabled" });
            }
            KeyCode::Char('h') => {
                let on = crate::live::toggle(&self.state.live.hidden);
                self.note(if on { "hidden files shown" } else { "hidden files hidden" });
            }
            KeyCode::Char('d') => {
                let on = crate::live::toggle(&self.state.live.dir_sizes);
                self.note(if on { "dir sizes on" } else { "dir sizes off" });
            }
            KeyCode::Char('z') => {
                let on = crate::live::toggle(&self.state.live.zip);
                self.note(if on { "zip downloads enabled" } else { "zip downloads disabled" });
            }
            KeyCode::Char('a') => self.toggle_auth(),
            KeyCode::Char('t') => {
                let turn_on = self.state.live.base().is_empty();
                self.state.live.set_token(turn_on);
                self.note(if turn_on {
                    "token URL enabled (new token — old links die)"
                } else {
                    "token URL disabled"
                });
            }
            KeyCode::Char('Q') => self.popup = Popup::Qr,
            KeyCode::Char('?') => self.popup = Popup::Help,
            KeyCode::Up => self.scroll_by(1),
            KeyCode::Down => self.scroll_by(-1),
            KeyCode::PageUp => self.scroll_by(10),
            KeyCode::PageDown => self.scroll_by(-10),
            _ => {}
        }
        Action::None
    }

    fn scroll_by(&mut self, delta: isize) {
        let max = self.log.len().saturating_sub(1);
        let cur = self.scroll as isize + delta;
        self.scroll = cur.clamp(0, max as isize) as usize;
    }

    fn toggle_mdns(&mut self) {
        if self.mdns_guard.take().is_some() {
            // drop unregisters
            self.state.live.mdns.store(false, Relaxed);
            self.note("mDNS announce disabled");
            return;
        }
        match crate::mdns::announce(self.port, "") {
            Ok(g) => {
                self.mdns_guard = Some(g);
                self.state.live.mdns.store(true, Relaxed);
                self.note("mDNS announce enabled");
            }
            Err(e) => {
                self.state.live.mdns.store(false, Relaxed);
                self.note(&format!("mDNS failed: {e}"));
            }
        }
    }

    fn toggle_auth(&mut self) {
        if self.state.live.auth().is_some() {
            *self.state.live.auth.write().unwrap() = None;
            self.note("auth disabled");
            return;
        }
        let creds = match &self.initial_auth {
            Some(c) => c.clone(),
            None => crate::auth::parse_auth(&None).expect("bare auth always parses"),
        };
        if self.initial_auth.is_none() {
            let (user, pass) = creds.split_once(':').unwrap_or((creds.as_str(), ""));
            self.notice = Some(format!("auth on — user: {user}  password: {pass}  (any key to dismiss)"));
        }
        *self.state.live.auth.write().unwrap() = Some(creds);
        self.note("auth enabled");
    }
}

/// Virtual/container interfaces are noise in the URL list: bridges,
/// veth pairs, VM and overlay networks. Physical NICs, wifi, and VPN
/// tunnels stay.
fn is_virtual_iface(name: &str) -> bool {
    ["docker", "br-", "veth", "virbr", "vmnet", "lxc", "lxd"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// Can we enter raw mode? Used by main to fall back to plain output.
pub fn probe() -> bool {
    use ratatui::crossterm::terminal;
    terminal::enable_raw_mode().and_then(|_| terminal::disable_raw_mode()).is_ok()
}

pub async fn run(
    mut app: App,
    mut events: mpsc::UnboundedReceiver<log::Event>,
    shutdown: impl std::future::Future<Output = String>,
) -> std::io::Result<Option<String>> {
    let mut terminal = ratatui::try_init()?;

    // blocking input thread -> channel (crossterm events aren't async)
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<CEvent>();
    std::thread::spawn(move || {
        use ratatui::crossterm::event;
        loop {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => {
                    if let Ok(ev) = event::read() {
                        if key_tx.send(ev).is_err() {
                            break;
                        }
                    }
                }
                Ok(false) => {
                    if key_tx.is_closed() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    tokio::pin!(shutdown);
    let mut tick = tokio::time::interval(Duration::from_millis(500));

    let result: std::io::Result<Option<String>> = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &app)) {
            break Err(e);
        }
        tokio::select! {
            Some(ev) = key_rx.recv() => {
                if let CEvent::Key(k) = ev {
                    if k.kind == ratatui::crossterm::event::KeyEventKind::Press {
                        if let Action::Quit = app.handle_key(k) {
                            break Ok(None);
                        }
                    }
                }
                // resize events fall through; next draw() picks up the new size
            }
            Some(e) = events.recv() => app.push_line(log::format_pretty(&e)),
            _ = tick.tick() => {} // refresh stats in header
            r = &mut shutdown => { break Ok(Some(r)); }
        }
    };

    drop(app.mdns_guard.take()); // unregister before leaving
    ratatui::restore();
    result
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    // QR side panel when enabled and the terminal is wide enough for
    // both the QR and a useful main column
    let mut main_area = area;
    if app.show_qr {
        if let Some(q) = qr_text(&app.primary_url()) {
            let qlines: Vec<&str> = q.lines().collect();
            let qw = qlines.first().map(|l| l.chars().count()).unwrap_or(0) as u16 + 2;
            let qh = qlines.len() as u16 + 2;
            if area.width >= qw + 46 && area.height >= qh {
                let [left, right] =
                    Layout::horizontal([Constraint::Min(0), Constraint::Length(qw)]).areas(area);
                let qr_rect = Rect::new(right.x, right.y, right.width, qh.min(right.height));
                f.render_widget(
                    Paragraph::new(q).block(Block::default().borders(Borders::ALL).title(" QR ")),
                    qr_rect,
                );
                main_area = left;
            }
        }
    }
    draw_main(f, app, main_area);

    match app.popup {
        Popup::Qr => draw_qr_popup(f, app),
        Popup::Help => draw_help_popup(f),
        Popup::None => {}
    }
}

fn draw_main(f: &mut Frame, app: &App, area: Rect) {
    let urls = app.url_lines();
    let header_h = urls.len() as u16 + 1 + app.notice.is_some() as u16 + 2;
    let [header, logs, bar] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area);

    // header
    let title = if app.info.single_file {
        format!(" fshare v{} — sharing file {} ", env!("CARGO_PKG_VERSION"), app.info.root.display())
    } else {
        format!(
            " fshare v{} — {} ({} files, {}) ",
            env!("CARGO_PKG_VERSION"),
            app.info.root.display(),
            app.info.files,
            crate::listing::human_size(app.info.bytes),
        )
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
}

fn qr_text(url: &str) -> Option<String> {
    qrcode::QrCode::new(url.as_bytes()).ok().map(|c| {
        c.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build()
    })
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
    let lines: Vec<&str> = rendered.lines().collect();
    let w = lines.first().map(|l| l.chars().count()).unwrap_or(0) as u16 + 2;
    let h = lines.len() as u16 + 2;
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
        Paragraph::new(rendered).block(Block::default().borders(Borders::ALL).title(format!(" {url} "))),
        r,
    );
}

fn draw_help_popup(f: &mut Frame) {
    let text = "\
 m  toggle mDNS announce
 u  toggle uploads
 a  toggle auth (generated password shown)
 t  toggle token URL (new token each enable)
 h  toggle hidden files
 d  toggle dir sizes
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{AppState, ShareOpts};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn test_app(auth: Option<String>, token: bool) -> App {
        let opts = ShareOpts {
            show_hidden: false,
            dir_sizes: false,
            follow_links: false,
            zip: true,
            upload: false,
            max_upload: None,
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(AppState::new(
            std::path::PathBuf::from("/tmp"),
            false,
            opts,
            token,
            tx,
            auth.clone(),
            None,
        ));
        App::new(
            state,
            "http",
            8000,
            ShareInfo { root: "/tmp".into(), single_file: false, files: 3, bytes: 1024 },
            false,
            None,
            auth,
            vec![],
        )
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn hotbar_reflects_state() {
        let app = test_app(None, false);
        let bar = app.hotbar();
        let get = |name| bar.iter().find(|(_, l, _)| *l == name).unwrap().2;
        assert!(!get("upload") && get("zip") && !get("auth") && !get("token"));
    }

    #[test]
    fn toggles_flip_live_state() {
        let mut app = test_app(None, false);
        app.handle_key(key('u'));
        assert!(app.state.live.upload.load(Relaxed));
        app.handle_key(key('u'));
        assert!(!app.state.live.upload.load(Relaxed));
        app.handle_key(key('t'));
        assert!(app.state.live.base().starts_with("/s/"));
        app.handle_key(key('t'));
        assert_eq!(app.state.live.base(), "");
    }

    #[test]
    fn auth_toggle_generates_and_reuses() {
        let mut app = test_app(None, false);
        app.handle_key(key('a'));
        let creds = app.state.live.auth().unwrap();
        assert!(creds.starts_with("fshare:"));
        assert!(app.notice.is_some(), "generated password surfaces in header");
        // any key dismisses the notice without acting
        app.handle_key(key('u'));
        assert!(app.notice.is_none());
        assert!(!app.state.live.upload.load(Relaxed), "dismissal key must not toggle");

        let mut app2 = test_app(Some("ben:pw".into()), false);
        app2.handle_key(key('a')); // off (was on via initial auth)
        assert_eq!(app2.state.live.auth(), None);
        app2.handle_key(key('a')); // back on — reuses explicit creds, no notice
        assert_eq!(app2.state.live.auth(), Some("ben:pw".to_string()));
        assert!(app2.notice.is_none());
    }

    #[test]
    fn quit_keys() {
        let mut app = test_app(None, false);
        assert!(matches!(app.handle_key(key('q')), Action::Quit));
        assert!(matches!(app.handle_key(key('x')), Action::Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(app.handle_key(ctrl_c), Action::Quit));
        // plain 'c' is not quit
        assert!(matches!(app.handle_key(key('c')), Action::None));
    }

    #[test]
    fn log_ring_trims_and_scroll_clamps() {
        let mut app = test_app(None, false);
        for i in 0..(LOG_CAP + 50) {
            app.push_line(format!("line {i}"));
        }
        assert_eq!(app.log.len(), LOG_CAP);
        assert_eq!(app.log.front().unwrap(), "line 50");
        app.scroll_by(10);
        assert_eq!(app.scroll, 10);
        app.scroll_by(-100);
        assert_eq!(app.scroll, 0);
        app.scroll_by(isize::MAX);
        assert_eq!(app.scroll, LOG_CAP - 1);
    }

    #[test]
    fn scroll_window_stable_across_ring_trim() {
        let mut app = test_app(None, false);
        // fill the ring past capacity so subsequent pushes trigger pop_front
        for i in 0..LOG_CAP {
            app.push_line(format!("line {i}"));
        }
        assert_eq!(app.log.len(), LOG_CAP);

        // scroll back a bit and record the line at the top of the visible window
        app.scroll_by(5);
        assert_eq!(app.scroll, 5);
        let total = app.log.len();
        let top_before = app.log[total - 1 - app.scroll].clone();

        // push more lines, forcing the ring to trim (pop_front)
        for i in 0..10 {
            app.push_line(format!("new {i}"));
        }
        assert_eq!(app.log.len(), LOG_CAP, "ring stays capped");

        // the viewed window must stay stable: same line still at the top offset
        let total = app.log.len();
        let top_after = app.log[total - 1 - app.scroll].clone();
        assert_eq!(top_before, top_after, "scrolled window must not shift on ring trim");
    }

    #[test]
    fn url_lines_list_all_interfaces_with_live_base() {
        let app = test_app(None, true); // token on
        let lines = app.url_lines();
        assert!(!lines.is_empty());
        let base = app.state.base();
        assert!(base.starts_with("/s/"));
        for l in &lines {
            assert!(l.contains(":8000"), "port in every URL: {l}");
            assert!(l.contains(&base), "live token base in every URL: {l}");
        }
        assert!(lines[0].starts_with('➜'), "primary URL marked");
        // no mDNS line while the flag is off
        assert!(!lines.iter().any(|l| l.contains("(mDNS)")));

        app.state.live.mdns.store(true, Relaxed);
        let lines = app.url_lines();
        assert!(lines[0].contains(".local:") && lines[0].contains("(mDNS)"));

        // token off: base vanishes from all URLs immediately
        app.state.live.set_token(false);
        assert!(app.url_lines().iter().all(|l| !l.contains(&base)));
    }

    #[test]
    fn virtual_ifaces_filtered() {
        for v in ["docker0", "br-48f804a6be88", "veth1a2b", "virbr0", "vmnet8", "lxcbr0"] {
            assert!(is_virtual_iface(v), "{v} should be hidden");
        }
        for p in ["wlan0", "eth0", "enp3s0", "wg0", "tun0", "tailscale0", "lo"] {
            assert!(!is_virtual_iface(p), "{p} should stay");
        }
        // lo is excluded by kind, not by name
        let app = test_app(None, false);
        assert!(app.url_lines().iter().all(|l| !l.contains("(lo)")));
    }

    #[test]
    fn qr_side_panel_renders_when_wide() {
        let mut app = test_app(None, false);
        app.show_qr = true;
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(text.contains(" QR "), "QR side panel visible on wide terminal");
        assert!(text.contains("[m]mdns"), "hotbar still present");

        // too narrow: main layout only, no QR panel
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(!text.contains(" QR "), "no QR panel on narrow terminal");
    }

    #[test]
    fn renders_header_and_hotbar() {
        let app = test_app(None, false);
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(text.contains("fshare v"));
        assert!(text.contains("[m]mdns"));
        assert!(text.contains("[u]upload:off"));
        assert!(text.contains("clients"));
    }

    #[test]
    fn qr_popup_renders() {
        let mut app = test_app(None, false);
        app.handle_key(key('Q'));
        assert!(matches!(app.popup, Popup::Qr));
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap(); // must not panic
        app.handle_key(key('m'));
        assert!(matches!(app.popup, Popup::None), "any key closes popup");
        assert!(!app.state.live.mdns.load(Relaxed), "close key must not toggle");
    }
}
