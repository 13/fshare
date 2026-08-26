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

/// Identity of a selectable address. Carries no scheme, port or base, so
/// the secure/token/auth toggles never disturb a selection.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum SelKey {
    Mdns,
    Ip(IpAddr),
}

/// One shareable address: the full URL, the trailing "(…)" label and the
/// identity used to keep a selection pinned across list rebuilds.
#[derive(Debug)]
pub(crate) struct UrlEntry {
    pub url: String,
    pub label: String,
    pub key: SelKey,
}

/// Builds the shareable-address list: mDNS name first when announcing, then
/// the interfaces that matter — loopback and virtual interfaces (docker
/// bridges, veth pairs, VM nets) are hidden unless nothing else exists.
/// Entries that render an identical URL (e.g. two interfaces sharing an IP)
/// are deduped, keeping the first. Pure so ordering and selection can be
/// tested with synthetic interfaces. Never returns an empty list.
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
    // interfaces that render an identical URL (macvlan/ipvlan, or plain
    // misconfiguration) add nothing but a second SelKey mapping to the same
    // spot — position()-based selection would resolve back to the first
    // one and jam forward Tab. Keep the first, drop the rest.
    let mut seen = std::collections::HashSet::new();
    v.retain(|e| seen.insert(e.url.clone()));
    v
}

/// The default primary entry when the operator hasn't pressed Tab: the
/// first IP-address entry, so mDNS announcing never silently repoints the
/// marker/QR at `.local`. Falls back to index 0 for the (rare) case of an
/// all-mDNS list with no IP entry at all.
fn default_index(entries: &[UrlEntry]) -> usize {
    entries.iter().position(|e| matches!(e.key, SelKey::Ip(_))).unwrap_or(0)
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
    /// Manually chosen primary address; `None` follows the ranked best.
    sel: Option<SelKey>,
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
            sel: None,
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

    /// The live entry list: mDNS name first when announcing, then the
    /// interfaces that matter. Rebuilt on demand so toggles show up at once.
    pub(crate) fn url_entries(&self) -> Vec<UrlEntry> {
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

    /// Resolves the selection against a freshly built list. `None` (no Tab
    /// pressed yet) defaults to the first IP-address entry, never the mDNS
    /// `.local` entry — `.local` resolves unreliably on some phone browsers
    /// and the QR code is the primary sharing path. A selected address that
    /// is momentarily gone (interface down, mDNS off) falls back to that
    /// same default *without* clearing `sel`, so it snaps back when the
    /// address returns.
    pub(crate) fn selected_index(&self, entries: &[UrlEntry]) -> usize {
        self.sel
            .as_ref()
            .and_then(|k| entries.iter().position(|e| &e.key == k))
            .unwrap_or_else(|| default_index(entries))
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
        // popups cover the screen: the closing key is swallowed. Tab is the
        // exception in the QR popup — it cycles the address so the QR
        // redraws in place.
        if self.popup != Popup::None {
            if self.popup == Popup::Qr {
                match key.code {
                    KeyCode::Tab => {
                        self.notice = None;
                        self.cycle(1);
                        return Action::None;
                    }
                    KeyCode::BackTab => {
                        self.notice = None;
                        self.cycle(-1);
                        return Action::None;
                    }
                    _ => {}
                }
            }
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
            KeyCode::Tab => self.cycle(1),
            KeyCode::BackTab => self.cycle(-1),
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

    fn tab() -> KeyEvent {
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    fn back_tab() -> KeyEvent {
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
    }

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
    fn duplicate_urls_are_deduped_keeping_the_first_label() {
        // two interfaces sharing an IP (macvlan/ipvlan, misconfiguration)
        // render an identical URL — keep only the first.
        let all = [iface("eth0", "192.168.1.50"), iface("eth0:1", "192.168.1.50")];
        let e = entries_from(&all, false, "http", 8000, "");
        assert_eq!(e.len(), 1, "duplicate URL collapses to one entry: {e:?}");
        assert_eq!(e[0].label, "LAN, eth0", "the first entry's label wins");
    }

    #[test]
    fn cycling_reaches_every_distinct_url_past_a_duplicate() {
        let all = [
            iface("eth0", "192.168.1.50"),
            iface("eth0:1", "192.168.1.50"), // duplicate IP, different name
            iface("wlan0", "192.168.1.60"),
        ];
        let entries = entries_from(&all, false, "http", 8000, "");
        assert_eq!(entries.len(), 2, "the duplicate is dropped: {entries:?}");

        let mut app = test_app(None, false);
        let mut visited = std::collections::HashSet::new();
        visited.insert(entries[app.selected_index(&entries)].url.clone());
        app.cycle_in(&entries, 1);
        visited.insert(entries[app.selected_index(&entries)].url.clone());
        assert_eq!(visited.len(), 2, "Tab reaches every distinct URL: {visited:?}");
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

    /// Host-independent: whatever the machine's real primary address is, it
    /// must render intact under the QR — a URL split across rows breaks
    /// terminal link detection. The width rule itself is unit-tested in
    /// `draw::tests`, which can pin a URL longer than the QR.
    #[test]
    fn qr_side_panel_url_is_never_wrapped() {
        let mut app = test_app(None, false);
        app.show_qr = true;
        let url = app.primary_url();
        let backend = TestBackend::new(160, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..40)
            .map(|y| (0..160).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        let hits = rows.iter().filter(|r| r.contains(&url)).count();
        assert!(
            hits >= 2,
            "{url} must appear whole in the header and under the QR:\n{}",
            rows.join("\n"),
        );
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

    #[test]
    fn cycle_wraps_both_directions_and_noops_on_single_entry() {
        let all = [iface("wlan0", "192.168.1.112"), iface("eth0", "172.23.246.136")];
        let entries = entries_from(&all, true, "http", 8000, ""); // mDNS + 2 ifaces
        assert_eq!(entries.len(), 3);
        let mut app = test_app(None, false);
        assert_eq!(app.selected_index(&entries), 1, "defaults to the first IP entry");

        app.cycle_in(&entries, 1);
        assert_eq!(app.selected_index(&entries), 2);
        app.cycle_in(&entries, 1);
        assert_eq!(app.selected_index(&entries), 0);
        app.cycle_in(&entries, 1);
        assert_eq!(app.selected_index(&entries), 1, "wraps forward back to the default");
        app.cycle_in(&entries, -1);
        assert_eq!(app.selected_index(&entries), 0, "wraps backward");

        // switching logs a line, so the operator sees what changed
        assert!(app.log.iter().any(|l| l.contains("primary address:")));

        let single = entries_from(&[iface("wlan0", "192.168.1.112")], false, "http", 8000, "");
        let before = app.sel.clone();
        app.cycle_in(&single, 1);
        assert_eq!(app.sel, before, "single entry: selection untouched");
    }

    #[test]
    fn default_selection_is_first_ip_not_mdns() {
        // mDNS announcing must not silently repoint the marker/QR at
        // `.local` — the default stays on the first IP address.
        let all = [iface("wlan0", "192.168.1.112"), iface("eth0", "172.23.246.136")];
        let entries = entries_from(&all, true, "http", 8000, ""); // mDNS + 2 ifaces
        assert_eq!(entries[0].key, SelKey::Mdns);

        let mut app = test_app(None, false);
        let idx = app.selected_index(&entries);
        assert_eq!(idx, 1, "defaults to the first IP entry, not mDNS at index 0");
        assert!(matches!(entries[idx].key, SelKey::Ip(_)));

        // marker, QR and caption all read the same index: check the marker
        let lines = App::lines_from(&entries, idx);
        assert_eq!(lines.iter().filter(|l| l.starts_with('➜')).count(), 1);
        assert!(lines[idx].starts_with('➜'));
        assert!(!lines[0].starts_with('➜'), "mDNS line is not marked by default");

        // Tab can still reach the mDNS entry
        app.cycle_in(&entries, -1);
        assert_eq!(app.selected_index(&entries), 0);
        assert_eq!(entries[app.selected_index(&entries)].key, SelKey::Mdns);
    }

    #[test]
    fn default_index_falls_back_to_zero_with_no_ip_entry() {
        // no interfaces at all: entries_from's localhost fallback only
        // fires when the list would otherwise be empty, so an mDNS-only
        // list with zero IP entries is a real (if rare) case.
        let e = entries_from(&[], true, "http", 8000, "");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].key, SelKey::Mdns);
        let app = test_app(None, false);
        assert_eq!(app.selected_index(&e), 0, "no IP entry: falls back to index 0, no panic");
    }

    #[test]
    fn cycle_from_a_stale_selection_starts_at_the_ranked_best() {
        let all = [
            iface("wlan0", "192.168.1.112"),
            iface("eth0", "172.23.246.136"),
            iface("wg0", "10.200.200.9"),
        ];
        let full = entries_from(&all, false, "http", 8000, "");
        let gone = entries_from(
            &[iface("wlan0", "192.168.1.112"), iface("wg0", "10.200.200.9")],
            false,
            "http",
            8000,
            "",
        );

        let mut app = test_app(None, false);
        app.cycle_in(&full, 1); // select eth0
        assert_eq!(app.selected_index(&full), 1);

        // eth0 drops, but two addresses remain: cycling is still possible and
        // must resume from the ranked best, not from a stale index
        assert_eq!(app.selected_index(&gone), 0);
        app.cycle_in(&gone, 1);
        assert_eq!(app.selected_index(&gone), 1);
        assert_eq!(app.sel, Some(gone[1].key.clone()));

        // backward from a stale selection wraps to the last entry
        let mut back = test_app(None, false);
        back.cycle_in(&full, 1); // eth0 again
        back.cycle_in(&gone, -1);
        assert_eq!(back.selected_index(&gone), 1, "wraps to the end, no underflow");
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

    #[test]
    fn tab_in_qr_popup_clears_the_notice() {
        // opening the popup itself already clears any prior notice (every
        // other key path does), so set one directly on the open popup to
        // exercise the Tab/BackTab carve-out specifically — the one path
        // that used to skip the clear.
        let mut app = test_app(None, false);
        app.handle_key(key('Q'));
        assert!(app.popup == Popup::Qr);

        app.notice = Some("auth on — user: fshare  password: xxxx".to_string());
        app.handle_key(tab());
        assert!(app.popup == Popup::Qr, "Tab keeps the popup open");
        assert!(app.notice.is_none(), "Tab in the QR popup clears the notice");

        app.notice = Some("another notice".to_string());
        app.handle_key(back_tab());
        assert!(app.popup == Popup::Qr, "Shift+Tab keeps the popup open");
        assert!(app.notice.is_none(), "Shift+Tab in the QR popup clears the notice");
    }

    #[test]
    fn tab_closes_the_help_popup_like_any_other_key() {
        let mut app = test_app(None, false);
        app.handle_key(key('?'));
        assert!(app.popup == Popup::Help);
        app.handle_key(tab());
        assert!(app.popup == Popup::None, "Tab closes Help rather than cycling behind it");
        app.handle_key(key('?'));
        app.handle_key(back_tab());
        assert!(app.popup == Popup::None, "Shift+Tab closes Help too");
    }

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

    /// Renders the header and reads back the line carrying the `➜` marker.
    fn marked_header_line(app: &App) -> String {
        let backend = TestBackend::new(160, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let (w, h) = (160usize, 60usize);
        (0..h)
            .map(|y| (0..w).map(|x| buf.content()[y * w + x].symbol()).collect::<String>())
            .find(|row| row.contains('➜'))
            .expect("a header line carries the primary marker")
    }

    #[test]
    fn rendered_marker_follows_the_selection() {
        let mut app = test_app(None, false);
        let before = marked_header_line(&app);
        assert!(
            before.contains(&app.primary_url()),
            "marked line is the primary URL: {before}"
        );

        app.handle_key(tab());
        let after = marked_header_line(&app);
        assert!(
            after.contains(&app.primary_url()),
            "marker still on the primary URL after Tab: {after}"
        );

        // on a multi-homed host Tab must actually move the marker; on a
        // single-homed one there is nowhere to move and it must not
        if app.url_entries().len() > 1 {
            assert_ne!(before, after, "Tab moves the marker to another address");
            app.handle_key(back_tab());
            assert_eq!(marked_header_line(&app), before, "Shift+Tab moves it back");
        } else {
            assert_eq!(before, after, "single address: the marker stays put");
        }
    }
}
