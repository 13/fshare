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
    let root = args
        .path
        .canonicalize()
        .map_err(|e| format!("cannot share '{}': {e}", args.path.display()))?;
    let single_file = root.is_file();
    if !single_file && !root.is_dir() {
        return Err(format!("'{}' is neither file nor directory", root.display()).into());
    }

    let (listener, port, bumped) = net::bind_port(args.bind, args.port).map_err(|e| {
        format!(
            "cannot bind port {}: {e} (try --port <N>)",
            args.port.unwrap_or(net::DEFAULT_PORT)
        )
    })?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main(args, root, single_file, listener, port, bumped))
}

async fn async_main(
    args: cli::Args,
    root: std::path::PathBuf,
    single_file: bool,
    listener: std::net::TcpListener,
    port: u16,
    bumped: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let opts = server::ShareOpts {
        show_hidden: args.hidden,
        dir_sizes: args.dir_sizes,
        follow_links: args.follow_links,
        zip: !args.no_zip && !single_file,
        upload: args.upload && !single_file,
        max_upload: args.max_upload_size,
    };
    let auth = match &args.auth {
        Some(v) => Some(fshare::auth::parse_auth(v)?),
        None => None,
    };
    let events = flog::Logger::spawn(args.json_log);
    let state = Arc::new(server::AppState::new(
        root.clone(),
        single_file,
        opts,
        args.token,
        events,
        auth,
        args.limit,
    ));

    let others = instance::others();
    let _guard = instance::register(port, &root)?;

    let _mdns_guard = if args.no_mdns {
        None
    } else {
        match fshare::mdns::announce(port, &state.base) {
            Ok(g) => Some(g),
            Err(e) => {
                println!("  {} mDNS unavailable: {e}", "note:".yellow());
                None
            }
        }
    };

    let scheme = if args.tls { "https" } else { "http" };

    let tls_config = if args.tls {
        let mut sans = vec![
            "fshare.local".to_string(),
            fshare::mdns::machine_hostname(),
            "localhost".to_string(),
        ];
        sans.extend(
            net::ranked_ifaces()
                .into_iter()
                .filter(|i| i.kind != net::IfaceKind::Loopback)
                .map(|i| i.ip.to_string()),
        );
        let paths = fshare::tls::load_or_generate(&fshare::tls::data_dir(), &sans)?;
        println!(
            "  {} TLS cert fingerprint SHA256: {}{}",
            "note:".yellow(),
            paths.fingerprint,
            if paths.generated { "  (newly generated)" } else { "" },
        );
        Some(
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&paths.cert, &paths.key)
                .await
                .map_err(|e| format!("TLS config: {e}"))?,
        )
    } else {
        None
    };

    print_banner(
        &args,
        &state,
        port,
        bumped,
        &others,
        single_file,
        &root,
        _mdns_guard.is_some(),
        scheme,
    );

    let app = server::router(state.clone());

    let expire = expiry::wait(
        args.timeout,
        args.max_downloads,
        state.downloads_done.clone(),
        state.download_signal.clone(),
    );
    let shutdown = async {
        tokio::select! {
            reason = expire => println!("\n  {} — shutting down", reason.yellow()),
            _ = tokio::signal::ctrl_c() => println!(),
        }
    };

    let make = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    listener.set_nonblocking(true)?;
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

    let s = &state.stats;
    println!(
        "  served {} requests to {} client(s), {} sent",
        s.requests.load(Ordering::Relaxed),
        s.clients.lock().unwrap().len(),
        fshare::listing::human_size(s.bytes.load(Ordering::Relaxed)),
    );
    Ok(())
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
    args: &cli::Args,
    state: &server::AppState,
    port: u16,
    bumped: bool,
    others: &[instance::Instance],
    single_file: bool,
    root: &std::path::Path,
    mdns_on: bool,
    scheme: &str,
) {
    let ver = env!("CARGO_PKG_VERSION");
    if single_file {
        println!("\n  {} v{ver} — sharing file {}", "fshare".bold(), root.display());
    } else {
        let (files, bytes) = dir_summary(root);
        println!(
            "\n  {} v{ver} — serving {} ({files} files, {})",
            "fshare".bold(),
            root.display(),
            fshare::listing::human_size(bytes),
        );
    }
    if bumped {
        println!("  {} port {} was busy, using {port}", "note:".yellow(), net::DEFAULT_PORT);
    }
    println!();

    // address lines as (plain-for-width, colored-for-print)
    let mut addr_lines: Vec<(String, String)> = Vec::new();
    if mdns_on {
        addr_lines.push((
            format!("➜ {scheme}://fshare.local:{port}{}/    (mDNS)", state.base),
            format!("{} {scheme}://fshare.local:{port}{}/    (mDNS)", "➜".green(), state.base),
        ));
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

    for o in others {
        println!(
            "  {} another fshare serving {} on :{} (PID {})",
            "note:".yellow(),
            o.dir.display(),
            o.port,
            o.pid
        );
    }
    if args.token {
        println!("  {} URLs above include the access token", "note:".yellow());
    }
    if let Some(l) = args.limit {
        println!(
            "  {} download speed limited to {}/s",
            "note:".yellow(),
            fshare::listing::human_size(l)
        );
    }
    if let Some(a) = &state.auth {
        let (user, pass) = a.split_once(':').unwrap_or((a.as_str(), ""));
        let explicit = matches!(
            args.auth.as_ref().and_then(|v| v.as_ref()),
            Some(v) if v.contains(':')
        );
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

