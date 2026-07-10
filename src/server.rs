use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ShareOpts {
    pub show_hidden: bool,
    pub follow_links: bool,
    pub zip: bool,
}

/// root MUST be canonicalized by the caller (done once at startup).
pub fn resolve(root: &Path, uri_path: &str, opts: &ShareOpts) -> Option<PathBuf> {
    let decoded = percent_decode_str(uri_path).decode_utf8().ok()?;
    let mut p = root.to_path_buf();
    for comp in decoded.split('/').filter(|c| !c.is_empty()) {
        if comp == "." || comp == ".." || comp.contains('\\') || comp.contains('\0') {
            return None;
        }
        if !opts.show_hidden && comp.starts_with('.') {
            return None;
        }
        p.push(comp);
    }
    if opts.follow_links {
        return p.symlink_metadata().is_ok().then_some(p);
    }
    let canon = p.canonicalize().ok()?; // also fails for missing files
    canon.starts_with(root).then_some(canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), "hello").unwrap();
        fs::write(root.join("sub/b.txt"), "world").unwrap();
        fs::write(root.join(".secret"), "shh").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", root.join("esc")).unwrap();
        (t, root)
    }

    fn opts() -> ShareOpts {
        ShareOpts { show_hidden: false, follow_links: false, zip: true }
    }

    #[test]
    fn resolves_normal_paths() {
        let (_t, root) = setup();
        assert_eq!(resolve(&root, "/a.txt", &opts()).unwrap(), root.join("a.txt"));
        assert_eq!(resolve(&root, "/sub/b.txt", &opts()).unwrap(), root.join("sub/b.txt"));
        assert_eq!(resolve(&root, "/", &opts()).unwrap(), root);
        assert_eq!(resolve(&root, "/sub%2Fb.txt", &opts()).unwrap(), root.join("sub/b.txt"));
    }

    #[test]
    fn rejects_bad_paths() {
        let (_t, root) = setup();
        assert!(resolve(&root, "/../x", &opts()).is_none());
        assert!(resolve(&root, "/%2e%2e/x", &opts()).is_none());
        assert!(resolve(&root, "/.secret", &opts()).is_none()); // dotfile
        assert!(resolve(&root, "/esc", &opts()).is_none()); // symlink escape
        assert!(resolve(&root, "/missing.txt", &opts()).is_none());
        // hidden opt-in
        let show = ShareOpts { show_hidden: true, ..opts() };
        assert!(resolve(&root, "/.secret", &show).is_some());
        // follow-links opt-in
        let follow = ShareOpts { follow_links: true, ..opts() };
        assert!(resolve(&root, "/esc", &follow).is_some());
    }
}
