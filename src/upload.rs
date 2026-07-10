use std::path::{Path, PathBuf};

pub fn sanitize_name(raw: &str, allow_hidden: bool) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next()?;
    let name: String = last.chars().filter(|c| *c != '\0').collect();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if !allow_hidden && name.starts_with('.') {
        return None;
    }
    Some(name.to_string())
}

pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1u32..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|c| !c.exists())
        .expect("finite collisions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("photo.jpg", false).unwrap(), "photo.jpg");
        assert_eq!(sanitize_name("../../etc/passwd", false).unwrap(), "passwd");
        assert_eq!(sanitize_name(r"C:\evil\x.exe", false).unwrap(), "x.exe");
        assert_eq!(sanitize_name("a\0b.txt", false).unwrap(), "ab.txt");
        assert!(sanitize_name("", false).is_none());
        assert!(sanitize_name("..", false).is_none());
        assert!(sanitize_name(".bashrc", false).is_none());
        assert_eq!(sanitize_name(".bashrc", true).unwrap(), ".bashrc");
    }

    #[test]
    fn unique_paths() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(t.path(), "a.txt"), t.path().join("a.txt"));
        std::fs::write(t.path().join("a.txt"), "x").unwrap();
        assert_eq!(unique_path(t.path(), "a.txt"), t.path().join("a (1).txt"));
        std::fs::write(t.path().join("a (1).txt"), "x").unwrap();
        assert_eq!(unique_path(t.path(), "a.txt"), t.path().join("a (2).txt"));
        std::fs::write(t.path().join("noext"), "x").unwrap();
        assert_eq!(unique_path(t.path(), "noext"), t.path().join("noext (1)"));
    }
}
