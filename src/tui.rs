use crate::log;
use crate::server::AppState;
use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

mod draw;
use draw::draw;

const LOG_CAP: usize = 1000;

/// Share facts for the header line. `summary` is filled by a background
/// walk (big trees take a while); the header shows "counting…" until then.
pub struct ShareInfo {
    pub root: std::path::PathBuf,
    pub single_file: bool,
    pub summary: Arc<std::sync::Mutex<Option<(u64, u64)>>>, // (files, bytes)
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
    port: u16,
    info: ShareInfo,
    show_qr: bool, // side-panel QR when the terminal is wide enough
    log: VecDeque<String>,
    scroll: usize, // lines above the bottom; 0 = follow
    popup: Popup,
    mdns_guard: Option<crate::mdns::MdnsGuard>,
    notice: Option<String>,           // e.g. generated credentials, cleared on any key
    initial_auth: Option<String>,     // "user:pass" from CLI/config, reused on re-enable
    /// Signals the server supervisor to swap the listener: true = TLS,
    /// false = plain HTTP.
    tls_tx: Option<mpsc::UnboundedSender<bool>>,
    /// TLS chosen at startup (--tls / config): secure-off never downgrades it.
    tls_at_start: bool,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: Arc<AppState>,
        port: u16,
        info: ShareInfo,
        show_qr: bool,
        mdns_guard: Option<crate::mdns::MdnsGuard>,
        initial_auth: Option<String>,
        seed_notes: Vec<String>,
        tls_tx: Option<mpsc::UnboundedSender<bool>>,
    ) -> Self {
        let tls_at_start = state.live.tls.load(Relaxed);
        let mut app = Self {
            state,
            port,
            info,
            show_qr,
            log: VecDeque::new(),
            scroll: 0,
            popup: Popup::None,
            mdns_guard,
            notice: None,
            initial_auth,
            tls_tx,
            tls_at_start,
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

    /// Live scheme — flips to https when the supervisor swaps in TLS.
    fn scheme(&self) -> &'static str {
        if self.state.live.tls.load(Relaxed) { "https" } else { "http" }
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
        format!("{}://{host}:{}{base}/", self.scheme(), self.port)
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
                self.scheme(),
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
                self.scheme(), self.port, ifc.name
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
            ('s', "secure", self.secure_on()),
            ('m', "mdns", l.mdns.load(Relaxed)),
            ('u', "upload", l.upload.load(Relaxed)),
            ('a', "auth", l.auth().is_some()),
            ('t', "token", !l.base().is_empty()),
            ('h', "hidden", l.hidden.load(Relaxed)),
            ('z', "zip", l.zip.load(Relaxed)),
        ]
    }

    /// Secure mode is a derived state: auth + token on, mDNS off.
    /// (TLS enable is triggered alongside but flips asynchronously once
    /// the supervisor has swapped the listener.)
    pub fn secure_on(&self) -> bool {
        let l = &self.state.live;
        l.auth().is_some() && !l.base().is_empty() && !l.mdns.load(Relaxed)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // popups cover the screen: the closing key is swallowed
        if self.popup != Popup::None {
            self.popup = Popup::None;
            self.notice = None;
            return Action::None;
        }
        // the header notice never blocks input — clear it and act
        self.notice = None;
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
            KeyCode::Char('s') => self.toggle_secure(),
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
        self.enable_auth();
        self.note("auth enabled");
    }

    fn enable_auth(&mut self) {
        let creds = match &self.initial_auth {
            Some(c) => c.clone(),
            None => crate::auth::parse_auth(&None).expect("bare auth always parses"),
        };
        if self.initial_auth.is_none() {
            let (user, pass) = creds.split_once(':').unwrap_or((creds.as_str(), ""));
            self.notice = Some(format!("auth on — user: {user}  password: {pass}  (any key to dismiss)"));
        }
        *self.state.live.auth.write().unwrap() = Some(creds);
    }

    /// Apply/undo the secure bundle's runtime parts: auth + token on,
    /// mDNS off. TLS is listener-level and cannot flip mid-run — when the
    /// share is plain HTTP a restart hint is logged.
    fn toggle_secure(&mut self) {
        if self.secure_on() {
            *self.state.live.auth.write().unwrap() = None;
            self.state.live.set_token(false);
            // mDNS stays off — announcing is opt-in (press m)
            self.note("secure mode off — auth off, token off");
            // undo the live TLS enable too, unless TLS was the startup choice
            if self.state.live.tls.load(Relaxed) && !self.tls_at_start {
                if let Some(tx) = &self.tls_tx {
                    if tx.send(false).is_ok() {
                        self.note("disabling TLS — swapping back to plain HTTP");
                    }
                }
            }
            return;
        }
        if self.state.live.auth().is_none() {
            self.enable_auth();
        }
        if self.state.live.base().is_empty() {
            self.state.live.set_token(true);
        }
        if self.mdns_guard.take().is_some() {
            self.state.live.mdns.store(false, Relaxed);
        }
        self.note("secure mode — auth on, token URL on, mDNS off");
        if !self.state.live.tls.load(Relaxed) {
            match &self.tls_tx {
                Some(tx) if tx.send(true).is_ok() => {
                    self.note("enabling TLS — swapping listener, open plain connections drop");
                }
                _ => self.note("TLS cannot start mid-run — restart with --tls for encryption"),
            }
        }
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
    let hosts = log::HostCache::default();

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
            Some(e) = events.recv() => {
                let line = hosts.annotate(&e).await;
                app.push_line(line);
            }
            _ = tick.tick() => {} // refresh stats in header
            r = &mut shutdown => { break Ok(Some(r)); }
        }
    };

    drop(app.mdns_guard.take()); // unregister before leaving
    ratatui::restore();
    result
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
            8000,
            ShareInfo {
                root: "/tmp".into(),
                single_file: false,
                summary: Arc::new(std::sync::Mutex::new(Some((3, 1024)))),
            },
            false,
            None,
            auth,
            vec![],
            None,
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
        assert!(!get("secure"));
        assert!(!bar.iter().any(|(_, l, _)| *l == "dirs"), "dir-sizes hotkey removed");
    }

    #[test]
    fn secure_toggle_bundles_auth_token_mdns() {
        let mut app = test_app(None, false);
        let (tx, mut rx) = mpsc::unbounded_channel();
        app.tls_tx = Some(tx);
        assert!(!app.secure_on());
        app.handle_key(key('s'));
        assert!(rx.try_recv().is_ok(), "secure on plain HTTP signals the TLS swap");
        assert!(app.state.live.auth().is_some(), "secure enables auth");
        assert!(app.state.live.base().starts_with("/s/"), "secure enables token");
        assert!(!app.state.live.mdns.load(Relaxed), "secure disables mDNS");
        assert!(app.secure_on());
        assert!(app.notice.is_some(), "generated password shown");

        // supervisor swapped the listener in the meantime
        app.state.live.tls.store(true, Relaxed);

        // notice is showing, but hotkeys act immediately — no double press
        app.handle_key(key('s')); // off
        assert!(!app.secure_on());
        assert_eq!(app.state.live.auth(), None);
        assert_eq!(app.state.live.base(), "");
        assert!(!app.state.live.mdns.load(Relaxed), "secure off must not enable mDNS");
        // TLS was enabled live (not at startup) — secure off downgrades it
        assert_eq!(rx.try_recv(), Ok(false), "secure off signals TLS downgrade");

        // 'd' no longer toggles anything
        let before = app.state.live.dir_sizes.load(Relaxed);
        app.handle_key(key('d'));
        assert_eq!(app.state.live.dir_sizes.load(Relaxed), before);
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
        // the notice never blocks input: the next key clears it AND acts
        app.handle_key(key('u'));
        assert!(app.notice.is_none());
        assert!(app.state.live.upload.load(Relaxed), "key acts despite notice");
        app.handle_key(key('u')); // back off

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
    fn secure_off_keeps_startup_tls() {
        // TLS chosen at startup: secure off must NOT downgrade it
        let (tx, mut rx) = mpsc::unbounded_channel();
        let opts = ShareOpts {
            show_hidden: false,
            dir_sizes: false,
            follow_links: false,
            zip: true,
            upload: false,
            max_upload: None,
        };
        let (etx, _erx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(AppState::new(
            std::path::PathBuf::from("/tmp"),
            false,
            opts,
            false,
            etx,
            None,
            None,
        ));
        state.live.tls.store(true, Relaxed); // startup --tls
        let mut app = App::new(
            state,
            8000,
            ShareInfo {
                root: "/tmp".into(),
                single_file: false,
                summary: Arc::new(std::sync::Mutex::new(Some((3, 1024)))),
            },
            false,
            None,
            None,
            vec![],
            Some(tx),
        );
        app.handle_key(key('s')); // on (tls already on: no signal)
        assert!(rx.try_recv().is_err(), "no TLS signal when already https");
        app.handle_key(key('s')); // off
        assert!(rx.try_recv().is_err(), "startup TLS survives secure off");
        assert!(app.state.live.tls.load(Relaxed));
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
    fn header_shows_counting_until_summary_arrives() {
        let app = test_app(None, false);
        *app.info.summary.lock().unwrap() = None;
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("counting"), "placeholder while walking the tree");

        *app.info.summary.lock().unwrap() = Some((42, 2048));
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("42 files"), "summary appears once counted");
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
        // bottom-aligned: the QR block's bottom border sits on the row just
        // above the hotkey bar (last row), i.e. row height-2
        let (w, h) = (120usize, 40usize);
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf.content()[y * w + x].symbol()).collect())
            .collect();
        let qr_title_row = rows.iter().position(|r| r.contains(" QR ")).unwrap();
        // QR sits in the LEFT column directly below the header, and its
        // border stretches down to the hotkey bar
        assert!(
            rows[qr_title_row].starts_with('┌'),
            "QR top border at the left edge: {:?}",
            rows[qr_title_row]
        );
        assert!(
            rows[h - 2].starts_with('└'),
            "QR border extends to the bottom, flush with hotkey bar: {:?}",
            rows[h - 2]
        );
        // directly below the header (row above is the header's bottom border)
        assert!(rows[qr_title_row - 1].starts_with('└'), "QR starts right under the header");
        // the encoded URL is printed below the QR inside the panel
        let url_below_qr = rows[qr_title_row..h - 1]
            .iter()
            .any(|r| r.chars().take(40).collect::<String>().contains("http"));
        assert!(url_below_qr, "URL shown under the QR when the column is tall enough");

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
