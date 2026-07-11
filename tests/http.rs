use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

async fn spawn(root: PathBuf, token: bool, upload: bool) -> (String, tokio::task::JoinHandle<()>) {
    spawn_opts(
        root,
        token,
        fshare::server::ShareOpts {
            show_hidden: false,
            dir_sizes: false,
            follow_links: false,
            zip: true,
            upload,
            max_upload: None,
        },
        None,
    )
    .await
}

async fn spawn_capped(root: PathBuf, cap: u64) -> (String, tokio::task::JoinHandle<()>) {
    spawn_opts(
        root,
        false,
        fshare::server::ShareOpts {
            show_hidden: false,
            dir_sizes: false,
            follow_links: false,
            zip: true,
            upload: true,
            max_upload: Some(cap),
        },
        None,
    )
    .await
}

async fn spawn_opts(
    root: PathBuf,
    token: bool,
    opts: fshare::server::ShareOpts,
    auth: Option<String>,
) -> (String, tokio::task::JoinHandle<()>) {
    let root = root.canonicalize().unwrap();
    let state =
        fshare::server::AppState::new(root, false, opts, token, fshare::log::Logger::spawn(false), auth, None);
    let base = state.base.clone();
    let app = fshare::server::router(Arc::new(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    (format!("http://{addr}{base}"), h)
}

fn fixture() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("hello.txt"), "hello world").unwrap();
    std::fs::create_dir(t.path().join("sub")).unwrap();
    std::fs::write(t.path().join("sub/x.bin"), vec![7u8; 5000]).unwrap();
    std::fs::write(t.path().join(".hidden"), "secret").unwrap();
    t
}

#[tokio::test]
async fn serves_listing_and_files() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false, false).await;
    let html = reqwest::get(format!("{base}/")).await.unwrap().text().await.unwrap();
    assert!(html.contains("hello.txt") && html.contains("sub"));
    assert!(!html.contains(".hidden"));

    let body = reqwest::get(format!("{base}/hello.txt")).await.unwrap();
    assert_eq!(body.status(), 200);
    assert_eq!(body.text().await.unwrap(), "hello world");
}

#[tokio::test]
async fn json_listing() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false, false).await;
    let v: serde_json::Value = reqwest::get(format!("{base}/?format=json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> =
        v.as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"hello.txt") && names.contains(&"sub"));
}

#[tokio::test]
async fn range_requests_work() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false, false).await;
    let c = reqwest::Client::new();
    let r = c
        .get(format!("{base}/sub/x.bin"))
        .header("Range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.bytes().await.unwrap().len(), 100);
}

#[tokio::test]
async fn blocks_traversal_and_dotfiles() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false, false).await;
    for p in ["/.hidden", "/%2e%2e/%2e%2e/etc/passwd", "/..%2f..%2fetc%2fpasswd"] {
        let r = reqwest::get(format!("{base}{p}")).await.unwrap();
        assert_eq!(r.status(), 404, "path {p}");
    }
}

#[tokio::test]
async fn zip_download_streams_valid_zip() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false, false).await;
    let r = reqwest::get(format!("{base}/?zip")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().contains("zip"));
    let bytes = r.bytes().await.unwrap();
    let mut ar = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let names: Vec<String> =
        (0..ar.len()).map(|i| ar.by_index(i).unwrap().name().to_string()).collect();
    assert!(names.contains(&"hello.txt".to_string()));
    assert!(names.contains(&"sub/x.bin".to_string()));
    assert!(!names.iter().any(|n| n.contains(".hidden")));
    let mut f = ar.by_name("hello.txt").unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut f, &mut s).unwrap();
    assert_eq!(s, "hello world");
}

#[tokio::test]
async fn counts_completed_downloads() {
    let t = fixture();
    let root = t.path().canonicalize().unwrap();
    let opts = fshare::server::ShareOpts {
        show_hidden: false,
        dir_sizes: false,
        follow_links: false,
        zip: true,
        upload: false,
        max_upload: None,
    };
    let state = Arc::new(fshare::server::AppState::new(
        root,
        false,
        opts,
        false,
        fshare::log::Logger::spawn(false),
        None,
        None,
    ));
    let app = fshare::server::router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    reqwest::get(format!("http://{addr}/hello.txt")).await.unwrap().bytes().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(state.downloads_done.load(std::sync::atomic::Ordering::Relaxed), 1);
    // listing does not count
    reqwest::get(format!("http://{addr}/")).await.unwrap().text().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(state.downloads_done.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test]
async fn upload_roundtrip_and_collision() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false, true).await;
    let c = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(b"fresh content".to_vec()).file_name("up.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c
        .post(format!("{base}/sub/"))
        .header("Accept", "application/json")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["saved"][0], "up.txt");
    assert_eq!(std::fs::read_to_string(t.path().join("sub/up.txt")).unwrap(), "fresh content");

    // collision → (1)
    let part = reqwest::multipart::Part::bytes(b"second".to_vec()).file_name("up.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let v: serde_json::Value = c
        .post(format!("{base}/sub/"))
        .header("Accept", "application/json")
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["saved"][0], "up (1).txt");
    assert_eq!(std::fs::read_to_string(t.path().join("sub/up (1).txt")).unwrap(), "second");

    // traversal filename neutralized
    let part = reqwest::multipart::Part::bytes(b"evil".to_vec()).file_name("../../escape.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let v: serde_json::Value = c
        .post(format!("{base}/"))
        .header("Accept", "application/json")
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["saved"][0], "escape.txt");
    assert!(t.path().join("escape.txt").exists());

    // no temp files left anywhere
    let leftovers: Vec<_> = walkdir_all(t.path())
        .into_iter()
        .filter(|n| n.contains(".fshare-upload-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn upload_disabled_and_cap() {
    let t = fixture();
    // disabled → 405
    let (base, _h) = spawn(t.path().into(), false, false).await;
    let c = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(b"x".to_vec()).file_name("a.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c.post(format!("{base}/")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 405);

    // cap → 413, no temp left
    let (base, _h) = spawn_capped(t.path().into(), 10).await;
    let part = reqwest::multipart::Part::bytes(vec![0u8; 1000]).file_name("big.bin");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c.post(format!("{base}/")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 413);
    assert!(!t.path().join("big.bin").exists());
    assert!(walkdir_all(t.path()).iter().all(|n| !n.contains(".fshare-upload-")));
}

async fn spawn_auth(root: PathBuf, creds: &str) -> (String, tokio::task::JoinHandle<()>) {
    spawn_opts(
        root,
        false,
        fshare::server::ShareOpts {
            show_hidden: false,
            dir_sizes: false,
            follow_links: false,
            zip: true,
            upload: true,
            max_upload: None,
        },
        Some(creds.to_string()),
    )
    .await
}

#[tokio::test]
async fn basic_auth_gates_all_routes() {
    let t = fixture();
    let (base, _h) = spawn_auth(t.path().into(), "ben:secret").await;
    let c = reqwest::Client::new();

    // no credentials → 401 with prompt header
    let r = c.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert!(r.headers()["www-authenticate"].to_str().unwrap().contains("Basic"));

    // wrong password → 401
    let r = c
        .get(format!("{base}/hello.txt"))
        .basic_auth("ben", Some("wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // garbage header → 401 not 500
    let r = c
        .get(format!("{base}/"))
        .header("Authorization", "Basic !!!not-base64!!!")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // correct → 200 listing and file
    let r = c
        .get(format!("{base}/hello.txt"))
        .basic_auth("ben", Some("secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "hello world");

    // upload also guarded
    let part = reqwest::multipart::Part::bytes(b"x".to_vec()).file_name("a.txt");
    let form = reqwest::multipart::Form::new().part("file", part);
    let r = c.post(format!("{base}/")).multipart(form).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn bandwidth_limit_slows_downloads() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("blob.bin"), vec![42u8; 64 * 1024]).unwrap();
    let root = t.path().canonicalize().unwrap();
    let opts = fshare::server::ShareOpts {
        show_hidden: false,
        dir_sizes: false,
        follow_links: false,
        zip: true,
        upload: false,
        max_upload: None,
    };
    // 256 KiB/s over 64 KiB; assert ≥120ms (unthrottled is <20ms)
    let state = Arc::new(fshare::server::AppState::new(
        root,
        false,
        opts,
        false,
        fshare::log::Logger::spawn(false),
        None,
        Some(256 * 1024),
    ));
    let app = fshare::server::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    let start = std::time::Instant::now();
    let body = reqwest::get(format!("http://{addr}/blob.bin"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.len(), 64 * 1024);
    assert!(body.iter().all(|b| *b == 42));
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(120),
        "took {:?} — limiter not applied?",
        start.elapsed()
    );
}

#[tokio::test]
async fn tls_serves_https() {
    let t = fixture();
    let certdir = tempfile::tempdir().unwrap();
    let tls =
        fshare::tls::load_or_generate(certdir.path(), &["localhost".to_string()]).unwrap();
    let root = t.path().canonicalize().unwrap();
    let opts = fshare::server::ShareOpts {
        show_hidden: false,
        dir_sizes: false,
        follow_links: false,
        zip: true,
        upload: false,
        max_upload: None,
    };
    let state = Arc::new(fshare::server::AppState::new(
        root,
        false,
        opts,
        false,
        fshare::log::Logger::spawn(false),
        None,
        None,
    ));
    let app = fshare::server::router(state);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
        .await
        .unwrap();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, config)
            .unwrap()
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let c = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
    let r = c.get(format!("https://{addr}/hello.txt")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "hello world");

    // plain http against TLS port fails
    assert!(reqwest::get(format!("http://{addr}/")).await.is_err());
}

fn walkdir_all(root: &std::path::Path) -> Vec<String> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(root).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() {
            v.extend(walkdir_all(&e.path()));
        }
        v.push(n);
    }
    v
}

#[tokio::test]
async fn token_gates_everything() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), true, false).await;
    assert!(base.contains("/s/"));
    let root = base.split("/s/").next().unwrap().to_string();
    assert_eq!(reqwest::get(format!("{root}/hello.txt")).await.unwrap().status(), 404);
    assert_eq!(reqwest::get(format!("{base}/hello.txt")).await.unwrap().status(), 200);
}
