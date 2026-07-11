use clap::Parser;
use fshare::{cli, expiry, instance, log as flog, net, server};
use owo_colors::OwoColorize;
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn main() {
    let args = cli::Args::parse();
    if let Err(e) = run(args) {
        eprintln!("{} {e}", "fshare:".red().bold());
        std::process::exit(1);
    }
}

fn run(args: cli::Args) -> Result<(), Box<dyn std::error::Error>> {
    let root = fshare::fsutil::resolve_root(&args.path)?;
    let single_file = root.is_file();
    if !single_file && !root.is_dir() {
        return Err(format!("'{}' is neither file nor directory", root.display()).into());
    }

    let cfg_path = fshare::config::default_path();
    let (cfg, cfg_loaded) = match &cfg_path {
        Some(p) => match fshare::config::load(p)? {
            Some(c) => (c, Some(p.clone())),
            None => (fshare::config::Config::default(), None),
        },
        None => (fshare::config::Config::default(), None),
    };
    let settings = fshare::config::resolve(&args, &cfg)?;

    let (listener, port, bumped) = net::bind_port(settings.bind, settings.port).map_err(|e| {
        format!(
            "cannot bind port {}: {e} (try --port <N>)",
            settings.port.unwrap_or(net::DEFAULT_PORT)
        )
    })?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(args, settings, cfg_loaded, root, single_file, listener, port, bumped))
}

#[allow(clippy::too_many_arguments)]
async fn async_main(
    args: cli::Args,
    settings: fshare::config::Settings,
    cfg_loaded: Option<std::path::PathBuf>,
    root: std::path::PathBuf,
    single_file: bool,
    listener: std::net::TcpListener,
    port: u16,
    bumped: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let opts = server::ShareOpts {
        show_hidden: settings.hidden,
        dir_sizes: settings.dir_sizes,
        follow_links: settings.follow_links,
        zip: settings.zip && !single_file,
        upload: settings.upload && !single_file,
        max_upload: settings.max_upload_size,
    };
    let auth = match &settings.auth {
        Some(v) => Some(fshare::auth::parse_auth(v)?),
        None => None,
    };
    let tui_wanted = settings.tui
        && !settings.json_log
        && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let use_tui = tui_wanted && fshare::tui::probe();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel::<flog::Event>();
    let mut events_rx = Some(events_rx);
    if !use_tui {
        flog::Logger::spawn_printer(events_rx.take().expect("rx present"), settings.json_log);
        if tui_wanted {
            println!("  {} terminal does not support raw mode — plain output", "note:".yellow());
        }
    }
    let state = Arc::new(server::AppState::new(
        root.clone(),
        single_file,
        opts,
        settings.token,
        events,
        auth,
        settings.limit,
    ));

    let others = instance::others();
    let _guard = instance::register(port, &root)?;

    let mut seed_notes: Vec<String> = Vec::new();
    let mut mdns_guard = if !settings.mdns {
        None
    } else {
        match fshare::mdns::announce(port, &state.base()) {
            Ok(g) => Some(g),
            Err(e) => {
                if use_tui {
                    seed_notes.push(format!("mDNS unavailable: {e}"));
                } else {
                    println!("  {} mDNS unavailable: {e}", "note:".yellow());
                }
                None
            }
        }
    };

    state.live.mdns.store(mdns_guard.is_some(), std::sync::atomic::Ordering::Relaxed);

    let scheme = if settings.tls { "https" } else { "http" };
    state.live.tls.store(settings.tls, std::sync::atomic::Ordering::Relaxed);

    let tls_config = if settings.tls {
        let paths = fshare::tls::load_or_generate(&fshare::tls::data_dir(), &tls_sans())?;
        let fp_note = format!(
            "TLS cert fingerprint SHA256: {}{}",
            paths.fingerprint,
            if paths.generated { "  (newly generated)" } else { "" },
        );
        if use_tui {
            seed_notes.push(fp_note);
        } else {
            println!("  {} {}", "note:".yellow(), fp_note);
        }
        Some(
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&paths.cert, &paths.key)
                .await
                .map_err(|e| format!("TLS config: {e}"))?,
        )
    } else {
        None
    };

    if !use_tui {
        print_banner(
            &settings,
            cfg_loaded.as_deref(),
            &state,
            port,
            bumped,
            &others,
            single_file,
            &root,
            mdns_guard.is_some(),
            scheme,
        );
        // count in the background — big trees shouldn't delay startup
        if !single_file {
            let r = root.clone();
            tokio::task::spawn_blocking(move || {
                let (files, bytes) = dir_summary(&r);
                println!(
                    "  {} sharing {files} files, {}",
                    "note:".yellow(),
                    fshare::listing::human_size(bytes)
                );
            });
        }
    } else {
        if let Some(p) = cfg_loaded.as_deref() {
            seed_notes.push(format!("loaded {}", p.display()));
        }
        if settings.secure {
            seed_notes.push(
                "secure mode — TLS/auth/token per settings, mDNS off unless overridden"
                    .to_string(),
            );
        }
        if settings.token {
            seed_notes.push("URLs include the access token".to_string());
        }
        if let Some(l) = settings.limit {
            seed_notes.push(format!(
                "download speed limited to {}/s",
                fshare::listing::human_size(l)
            ));
        }
        if let Some(a) = state.live.auth() {
            let (user, pass) = a.split_once(':').unwrap_or((a.as_str(), ""));
            let explicit = matches!(&settings.auth, Some(Some(v)) if v.contains(':'));
            if explicit {
                seed_notes.push(format!("auth enabled (user {user})"));
            } else {
                seed_notes.push(format!("auth enabled — user: {user}  password: {pass}"));
            }
        }
        for o in &others {
            seed_notes.push(format!(
                "another fshare serving {} on :{} (PID {})",
                o.dir.display(),
                o.port,
                o.pid
            ));
        }
        if bumped {
            seed_notes.push(format!("port {} was busy, using {port}", net::DEFAULT_PORT));
        }
    }

    let app = server::router(state.clone());

    let expire = expiry::wait(
        args.timeout,
        args.max_downloads,
        state.downloads_done.clone(),
        state.download_signal.clone(),
    );

    let make = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    listener.set_nonblocking(true)?;

    if use_tui {
        // server runs as a background task supervised for live TLS
        // enablement; TUI owns the foreground
        let (tls_tx, tls_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let server = tokio::spawn(serve_tui(
            listener,
            make,
            tls_config,
            tls_rx,
            state.clone(),
            settings.bind,
            port,
        ));

        let initial_auth = match &settings.auth {
            Some(Some(v)) if v.contains(':') => Some(v.clone()),
            _ => state.live.auth(), // generated or None — reuse what's active
        };
        // big trees take a while to walk — fill the summary in the
        // background so the TUI appears instantly ("counting…" meanwhile)
        let summary: Arc<std::sync::Mutex<Option<(u64, u64)>>> = Arc::default();
        if single_file {
            *summary.lock().unwrap() =
                Some((1, root.metadata().map(|m| m.len()).unwrap_or(0)));
        } else {
            let s = summary.clone();
            let r = root.clone();
            tokio::task::spawn_blocking(move || {
                let v = dir_summary(&r);
                *s.lock().unwrap() = Some(v);
            });
        }
        let info = fshare::tui::ShareInfo { root: root.clone(), single_file, summary };
        let tapp = fshare::tui::App::new(
            state.clone(),
            port,
            info,
            settings.qr,
            mdns_guard.take(),
            initial_auth,
            seed_notes,
            Some(tls_tx),
        );
        let expire_owned = async move { expire.await.to_string() };
        let reason = fshare::tui::run(
            tapp,
            events_rx.take().expect("rx reserved for tui"),
            expire_owned,
        )
        .await?;
        server.abort();
        if let Some(r) = reason {
            println!("\n  {} — shutting down", r.yellow());
        }
    } else {
        let shutdown = async {
            tokio::select! {
                reason = expire => println!("\n  {} — shutting down", reason.yellow()),
                _ = tokio::signal::ctrl_c() => println!(),
            }
        };
        if let Some(cfg) = tls_config {
            tokio::select! {
                r = axum_server::from_tcp_rustls(listener, cfg)?.serve(make) => r?,
                _ = shutdown => {}
            }
        } else {
            let l = tokio::net::TcpListener::from_std(listener)?;
            tokio::select! {
                r = axum::serve(l, make) => r?,
                _ = shutdown => {}
            }
        }
    }

    let s = &state.stats;
    println!(
        "  served {} requests to {} client(s), {} sent",
        s.requests.load(Ordering::Relaxed),
        s.clients.lock().unwrap().len(),
        fshare::listing::human_size(s.bytes.load(Ordering::Relaxed)),
    );
    Ok(())
}

fn tls_sans() -> Vec<String> {
    let mut sans = vec![
        format!("{}.local", fshare::mdns::host_label()),
        fshare::mdns::machine_hostname(),
        "localhost".to_string(),
    ];
    sans.extend(
        net::ranked_ifaces()
            .into_iter()
            .filter(|i| i.kind != net::IfaceKind::Loopback)
            .map(|i| i.ip.to_string()),
    );
    sans
}

/// TUI-mode server supervisor: serves plain HTTP until the TUI signals a
/// live TLS enable (secure hotkey), then rebinds the same port with the
/// persisted certificate. Open plain connections drop; the TUI is told
/// via Setting events. Falls back to plain HTTP if TLS setup fails.
async fn serve_tui(
    listener: std::net::TcpListener,
    make: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<
        axum::Router,
        std::net::SocketAddr,
    >,
    tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    mut tls_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    state: Arc<server::AppState>,
    bind: std::net::IpAddr,
    port: u16,
) -> Result<(), std::io::Error> {
    if let Some(cfg) = tls_config {
        return axum_server::from_tcp_rustls(listener, cfg)?.serve(make).await;
    }
    {
        let l = tokio::net::TcpListener::from_std(listener)?;
        tokio::select! {
            r = axum::serve(l, make.clone()) => return r,
            Some(()) = tls_rx.recv() => {} // dropping the select arm closes the listener
        }
    }
    let note = |t: String| {
        let _ = state.events.send(fshare::log::Event::Setting { text: t });
    };
    let rebind = || -> std::io::Result<std::net::TcpListener> {
        let sock = std::net::TcpListener::bind(std::net::SocketAddr::new(bind, port))?;
        sock.set_nonblocking(true)?;
        Ok(sock)
    };
    let tls_cfg = match fshare::tls::load_or_generate(&fshare::tls::data_dir(), &tls_sans()) {
        Ok(paths) => {
            match axum_server::tls_rustls::RustlsConfig::from_pem_file(&paths.cert, &paths.key)
                .await
            {
                Ok(cfg) => {
                    note(format!(
                        "TLS enabled — cert SHA256 {}{}",
                        paths.fingerprint,
                        if paths.generated { " (newly generated)" } else { "" },
                    ));
                    Some(cfg)
                }
                Err(e) => {
                    note(format!("TLS enable failed: {e} — staying on plain HTTP"));
                    None
                }
            }
        }
        Err(e) => {
            note(format!("TLS enable failed: {e} — staying on plain HTTP"));
            None
        }
    };
    match tls_cfg {
        Some(cfg) => {
            state.live.tls.store(true, Ordering::Relaxed);
            note("URLs are https now — reload open pages".to_string());
            axum_server::from_tcp_rustls(rebind()?, cfg)?.serve(make).await
        }
        None => {
            let l = tokio::net::TcpListener::from_std(rebind()?)?;
            axum::serve(l, make).await
        }
    }
}

fn dir_summary(root: &std::path::Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    for e in walkdir::WalkDir::new(root).max_depth(8).into_iter().flatten() {
        if e.file_type().is_file() {
            files += 1;
            bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (files, bytes)
}

#[allow(clippy::too_many_arguments)]
fn print_banner(
    settings: &fshare::config::Settings,
    cfg_loaded: Option<&std::path::Path>,
    state: &server::AppState,
    port: u16,
    bumped: bool,
    others: &[instance::Instance],
    single_file: bool,
    root: &std::path::Path,
    mdns_on: bool,
    scheme: &str,
) {
    let base = state.base();
    let ver = env!("CARGO_PKG_VERSION");
    if single_file {
        println!("\n  {} v{ver} — sharing file {}", "fshare".bold(), root.display());
    } else {
        // file count arrives later as a note (async walk, see async_main)
        println!("\n  {} v{ver} — serving {}", "fshare".bold(), root.display());
    }
    if bumped {
        println!("  {} port {} was busy, using {port}", "note:".yellow(), net::DEFAULT_PORT);
    }
    println!();

    // address lines as (plain-for-width, colored-for-print)
    let mut addr_lines: Vec<(String, String)> = Vec::new();
    if mdns_on {
        let host = fshare::mdns::host_label();
        addr_lines.push((
            format!("➜ {scheme}://{host}.local:{port}{base}/    (mDNS)"),
            format!("{} {scheme}://{host}.local:{port}{base}/    (mDNS)", "➜".green()),
        ));
    }
    let ifaces = net::ranked_ifaces();
    let mut best_url = None;
    for (i, ifc) in ifaces.iter().enumerate() {
        let host = match ifc.ip {
            IpAddr::V6(v6) => format!("[{v6}]"),
            IpAddr::V4(v4) => v4.to_string(),
        };
        let url = format!("{scheme}://{host}:{port}{base}/");
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

    let show_qr = settings.qr && std::io::IsTerminal::is_terminal(&std::io::stdout());
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

    for o in others {
        println!(
            "  {} another fshare serving {} on :{} (PID {})",
            "note:".yellow(),
            o.dir.display(),
            o.port,
            o.pid
        );
    }
    if let Some(p) = cfg_loaded {
        println!("  {} loaded {}", "note:".yellow(), p.display());
    }
    if settings.secure {
        println!(
            "  {} secure mode — TLS {}, auth {}, token URL, mDNS {}",
            "note:".yellow(),
            if settings.tls { "on" } else { "off (overridden)" },
            if settings.auth.is_some() { "on" } else { "off (overridden)" },
            if mdns_on { "on (overridden)" } else { "off" },
        );
    }
    if settings.token {
        println!("  {} URLs above include the access token", "note:".yellow());
    }
    if let Some(l) = settings.limit {
        println!(
            "  {} download speed limited to {}/s",
            "note:".yellow(),
            fshare::listing::human_size(l)
        );
    }
    if let Some(a) = state.live.auth() {
        let (user, pass) = a.split_once(':').unwrap_or((a.as_str(), ""));
        let explicit = matches!(&settings.auth, Some(Some(v)) if v.contains(':'));
        if explicit {
            println!("  {} auth enabled (user {user})", "note:".yellow());
        } else {
            println!(
                "  {} auth enabled — user: {user}  password: {pass}",
                "note:".yellow()
            );
        }
    }
    println!("  Ctrl+C to stop\n");
}

