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
    listener.set_nonblocking(true)?;

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
        follow_links: args.follow_links,
        zip: !args.no_zip && !single_file,
        upload: args.upload && !single_file,
        max_upload: args.max_upload_size,
    };
    let events = flog::Logger::spawn(args.json_log);
    let state = Arc::new(server::AppState::new(
        root.clone(),
        single_file,
        opts,
        args.token,
        events,
    ));

    let others = instance::others();
    let _guard = instance::register(port, &root)?;

    print_banner(&args, &state, port, bumped, &others, single_file, &root);

    let app = server::router(state.clone());
    let listener = tokio::net::TcpListener::from_std(listener)?;

    let expire = expiry::wait(
        args.timeout,
        args.max_downloads,
        state.downloads_done.clone(),
        state.download_signal.clone(),
    );

    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    );

    tokio::select! {
        r = serve => r?,
        reason = expire => println!("\n  {} — shutting down", reason.yellow()),
        _ = tokio::signal::ctrl_c() => println!(),
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

fn print_banner(
    args: &cli::Args,
    state: &server::AppState,
    port: u16,
    bumped: bool,
    others: &[instance::Instance],
    single_file: bool,
    root: &std::path::Path,
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

    let ifaces = net::ranked_ifaces();
    let mut best_url = None;
    for (i, ifc) in ifaces.iter().enumerate() {
        let host = match ifc.ip {
            IpAddr::V6(v6) => format!("[{v6}]"),
            IpAddr::V4(v4) => v4.to_string(),
        };
        let url = format!("http://{host}:{port}{}/", state.base);
        let kind = match ifc.kind {
            net::IfaceKind::Lan => "LAN, ",
            _ => "",
        };
        let marker = if i == 0 { "➜".green().to_string() } else { " ".to_string() };
        println!("  {marker} {url:40} ({kind}{})", ifc.name);
        if i == 0 {
            best_url = Some(url);
        }
    }

    if !args.no_qr && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        if let Some(url) = &best_url {
            if let Ok(code) = qrcode::QrCode::new(url.as_bytes()) {
                let s = code
                    .render::<qrcode::render::unicode::Dense1x2>()
                    .quiet_zone(true)
                    .build();
                println!("\n{}", indent(&s, "  "));
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
    println!("  Ctrl+C to stop\n");
}

fn indent(s: &str, pad: &str) -> String {
    s.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
}
