use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

async fn spawn(root: PathBuf, token: bool) -> (String, tokio::task::JoinHandle<()>) {
    let root = root.canonicalize().unwrap();
    let opts = fshare::server::ShareOpts { show_hidden: false, follow_links: false, zip: true };
    let state = fshare::server::AppState::new(root, false, opts, token);
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
    let (base, _h) = spawn(t.path().into(), false).await;
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
    let (base, _h) = spawn(t.path().into(), false).await;
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
    let (base, _h) = spawn(t.path().into(), false).await;
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
    let (base, _h) = spawn(t.path().into(), false).await;
    for p in ["/.hidden", "/%2e%2e/%2e%2e/etc/passwd", "/..%2f..%2fetc%2fpasswd"] {
        let r = reqwest::get(format!("{base}{p}")).await.unwrap();
        assert_eq!(r.status(), 404, "path {p}");
    }
}

#[tokio::test]
async fn zip_download_streams_valid_zip() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), false).await;
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
async fn token_gates_everything() {
    let t = fixture();
    let (base, _h) = spawn(t.path().into(), true).await;
    assert!(base.contains("/s/"));
    let root = base.split("/s/").next().unwrap().to_string();
    assert_eq!(reqwest::get(format!("{root}/hello.txt")).await.unwrap().status(), 404);
    assert_eq!(reqwest::get(format!("{base}/hello.txt")).await.unwrap().status(), 200);
}
