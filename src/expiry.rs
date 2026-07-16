use crate::stats::Stats;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Resolves when either shutdown condition fires; pending forever when both None.
/// Race note: `notified()` is re-created after each counter check, but the
/// tracker increments the counter *before* calling `notify_waiters`, and the
/// loop rechecks the counter on every wake, so the final wake cannot be missed.
pub async fn wait(timeout: Option<Duration>, max: Option<u64>, stats: Arc<Stats>) -> &'static str {
    let timer = async {
        match timeout {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending().await,
        }
    };
    let limit = async {
        match max {
            None => std::future::pending().await,
            Some(m) => loop {
                if stats.downloads_done.load(Ordering::Relaxed) >= m {
                    break;
                }
                stats.download_signal.notified().await;
            },
        }
    };
    tokio::select! {
        _ = timer => "timeout reached",
        _ = limit => "download limit reached",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn timeout_fires() {
        let stats = Arc::new(Stats::default());
        let fut = wait(Some(Duration::from_secs(60)), None, stats);
        assert_eq!(fut.await, "timeout reached"); // paused clock auto-advances
    }

    #[tokio::test]
    async fn max_downloads_fires() {
        let stats = Arc::new(Stats::default());
        let fut = wait(None, Some(2), stats.clone());
        tokio::pin!(fut);
        stats.downloads_done.store(2, Ordering::Relaxed);
        stats.download_signal.notify_waiters();
        tokio::select! {
            r = &mut fut => assert_eq!(r, "download limit reached"),
            _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("did not fire"),
        }
    }
}
