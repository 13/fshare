use crate::listing::human_size;
use owo_colors::OwoColorize;
use serde_json::json;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum Event {
    Request { ip: IpAddr, method: String, path: String, status: u16 },
    Done { ip: IpAddr, path: String, bytes: u64, completed: bool, secs: f64 },
}

pub fn format_pretty(e: &Event) -> String {
    let ts = chrono::Local::now().format("%H:%M:%S");
    match e {
        Event::Request { ip, method, path, status } => {
            format!("{ts}  {ip:15}  {method} {path}  {status}")
        }
        Event::Done { ip, path, bytes, completed: true, secs } => {
            let speed = if *secs > 0.0 { *bytes as f64 / secs } else { 0.0 };
            format!(
                "{ts}  {ip:15}  {} {path} complete  {} in {secs:.0}s  {}/s",
                "✓".green(),
                human_size(*bytes),
                human_size(speed as u64),
            )
        }
        Event::Done { ip, path, bytes, completed: false, .. } => {
            format!("{ts}  {ip:15}  {} {path} canceled at {}", "✗".red(), human_size(*bytes))
        }
    }
}

fn format_json(e: &Event) -> String {
    match e {
        Event::Request { ip, method, path, status } => json!({
            "event": "request", "ip": ip, "method": method, "path": path, "status": status
        }),
        Event::Done { ip, path, bytes, completed, secs } => json!({
            "event": if *completed { "download_complete" } else { "download_canceled" },
            "ip": ip, "path": path, "bytes": bytes, "seconds": secs
        }),
    }
    .to_string()
}

pub struct Logger;

impl Logger {
    pub fn spawn(json: bool) -> mpsc::UnboundedSender<Event> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        let cache: Arc<Mutex<HashMap<IpAddr, Option<String>>>> = Arc::default();
        tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if json {
                    println!("{}", format_json(&e));
                    continue;
                }
                let ip = match &e {
                    Event::Request { ip, .. } | Event::Done { ip, .. } => *ip,
                };
                let host = lookup_cached(&cache, ip).await;
                let mut line = format_pretty(&e);
                if let Some(h) = host {
                    line = line.replacen(&ip.to_string(), &format!("{ip} ({h})"), 1);
                }
                println!("{line}");
            }
        });
        tx
    }
}

async fn lookup_cached(
    cache: &Arc<Mutex<HashMap<IpAddr, Option<String>>>>,
    ip: IpAddr,
) -> Option<String> {
    if let Some(v) = cache.lock().unwrap().get(&ip) {
        return v.clone();
    }
    let res = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        tokio::task::spawn_blocking(move || dns_lookup::lookup_addr(&ip).ok()),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .flatten();
    cache.lock().unwrap().insert(ip, res.clone());
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_events() {
        let ip = "192.168.1.23".parse().unwrap();
        let r = format_pretty(&Event::Request {
            ip,
            method: "GET".into(),
            path: "/vid.mp4".into(),
            status: 206,
        });
        assert!(r.contains("192.168.1.23") && r.contains("GET") && r.contains("206"));
        let d = format_pretty(&Event::Done {
            ip,
            path: "/vid.mp4".into(),
            bytes: 312 * 1024 * 1024,
            completed: true,
            secs: 14.0,
        });
        assert!(d.contains("✓") && d.contains("312 MB") && d.contains("22.3 MB/s"));
        let c = format_pretty(&Event::Done {
            ip,
            path: "/vid.mp4".into(),
            bytes: 1024,
            completed: false,
            secs: 1.0,
        });
        assert!(c.contains("✗") && c.contains("canceled"));
    }
}
