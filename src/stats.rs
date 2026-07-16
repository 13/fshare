use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use tokio::sync::Notify;

/// All transfer counters shared between the request path, the expiry
/// watcher, the TUI header and the shutdown summary.
#[derive(Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub bytes: AtomicU64,
    pub clients: Mutex<std::collections::HashSet<std::net::IpAddr>>,
    pub downloads_done: AtomicU64,
    pub download_signal: Notify,
}
