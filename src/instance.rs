use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug)]
pub struct Instance {
    pub pid: u32,
    pub dir: PathBuf,
    pub port: u16,
}

pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d).join("fshare"),
        None => {
            let uid = std::fs::metadata("/proc/self")
                .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
                .unwrap_or(0);
            PathBuf::from(format!("/tmp/fshare-{uid}"))
        }
    }
}

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

pub struct InstanceGuard {
    file: PathBuf,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

pub fn register(port: u16, dir: &Path) -> io::Result<InstanceGuard> {
    register_in(&runtime_dir(), port, dir)
}

pub fn register_in(base: &Path, port: u16, dir: &Path) -> io::Result<InstanceGuard> {
    std::fs::create_dir_all(base)?;
    let file = base.join(format!("{port}.json"));
    let inst = Instance { pid: std::process::id(), dir: dir.to_path_buf(), port };
    std::fs::write(&file, serde_json::to_vec(&inst)?)?;
    Ok(InstanceGuard { file })
}

pub fn others() -> Vec<Instance> {
    others_in(&runtime_dir())
}

pub fn others_in(base: &Path) -> Vec<Instance> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(base) else { return out };
    for entry in rd.flatten() {
        let path = entry.path();
        let inst: Option<Instance> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        match inst {
            Some(i) if i.pid == std::process::id() => {}
            Some(i) if pid_alive(i.pid) => out.push(i),
            _ => {
                let _ = std::fs::remove_file(&path); // stale or corrupt
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_scan_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let g = register_in(tmp.path(), 8123, std::path::Path::new("/srv")).unwrap();
        // own PID excluded from others
        assert!(others_in(tmp.path()).is_empty());
        // fake dead instance gets cleaned
        std::fs::write(
            tmp.path().join("9999.json"),
            r#"{"pid":4000000000,"dir":"/x","port":9999}"#,
        )
        .unwrap();
        assert!(others_in(tmp.path()).is_empty());
        assert!(!tmp.path().join("9999.json").exists());
        // live foreign instance reported (PID 1 always alive on linux)
        std::fs::write(
            tmp.path().join("8500.json"),
            r#"{"pid":1,"dir":"/y","port":8500}"#,
        )
        .unwrap();
        let o = others_in(tmp.path());
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].port, 8500);
        drop(g);
        assert!(!tmp.path().join("8123.json").exists());
    }
}
