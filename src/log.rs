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
    Upload { ip: IpAddr, name: String, bytes: u64, secs: f64 },
    Setting { text: String },
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
        Event::Upload { ip, name, bytes, secs } => {
            format!(
                "{ts}  {ip:15}  {} {name} received  {} in {secs:.0}s",
                "⬆".cyan(),
                human_size(*bytes)
            )
        }
        Event::Setting { text } => format!("{ts}  ⚙ {text}"),
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
        Event::Upload { ip, name, bytes, secs } => json!({
            "event": "upload", "ip": ip, "name": name, "bytes": bytes, "seconds": secs
        }),
        Event::Setting { text } => json!({ "event": "setting", "text": text }),
    }
    .to_string()
}

pub struct Logger;

impl Logger {
    pub fn spawn(json: bool) -> mpsc::UnboundedSender<Event> {
        let (tx, rx) = mpsc::unbounded_channel::<Event>();
        Self::spawn_printer(rx, json);
        tx
    }

    pub fn spawn_printer(mut rx: mpsc::UnboundedReceiver<Event>, json: bool) {
        let cache = HostCache::default();
        tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if json {
                    println!("{}", format_json(&e));
                    continue;
                }
                println!("{}", cache.annotate(&e).await);
            }
        });
    }
}

/// Reverse-DNS cache shared by the plain printer and the TUI so both
/// annotate client IPs the same way ("ip (hostname)").
#[derive(Default, Clone)]
pub struct HostCache(Arc<Mutex<HashMap<IpAddr, Option<String>>>>);

impl HostCache {
    /// `format_pretty` plus hostname annotation; lookups cached per IP.
    pub async fn annotate(&self, e: &Event) -> String {
        let ip = match e {
            Event::Request { ip, .. } | Event::Done { ip, .. } | Event::Upload { ip, .. } => {
                Some(*ip)
            }
            Event::Setting { .. } => None,
        };
        let mut line = format_pretty(e);
        if let Some(ip) = ip {
            if let Some(h) = self.lookup(ip).await {
                line = line.replacen(&ip.to_string(), &format!("{ip} ({h})"), 1);
            }
        }
        line
    }

    async fn lookup(&self, ip: IpAddr) -> Option<String> {
        if let Some(v) = self.0.lock().unwrap().get(&ip) {
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
        self.0.lock().unwrap().insert(ip, res.clone());
        res
    }
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

    #[test]
    fn formats_upload() {
        let ip = "192.168.1.23".parse().unwrap();
        let u = format_pretty(&Event::Upload {
            ip,
            name: "photo.jpg".into(),
            bytes: 4 * 1024 * 1024,
            secs: 3.0,
        });
        assert!(u.contains("⬆") && u.contains("photo.jpg") && u.contains("4.0 MB"));
    }

    #[test]
    fn formats_setting() {
        let s = format_pretty(&Event::Setting { text: "upload enabled".into() });
        assert!(s.contains("⚙") && s.contains("upload enabled"));
    }
}
