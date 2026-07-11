use std::io;
use std::path::{Path, PathBuf};

/// Canonicalize the share root.
///
/// A relative path resolves through the process cwd, which fails with
/// `NotFound` when the shell's working directory was deleted (even if a
/// directory has since been recreated at the same logical path — that is a
/// new inode). The shell's `$PWD` still holds the logical path, so retry
/// through it before giving up, and hint at `cd "$PWD"` in the error.
pub fn resolve_root(path: &Path) -> Result<PathBuf, String> {
    let err = match path.canonicalize() {
        Ok(p) => return Ok(p),
        Err(e) => e,
    };
    if err.kind() == io::ErrorKind::NotFound && path.is_relative() {
        if let Ok(pwd) = std::env::var("PWD") {
            if let Ok(p) = Path::new(&pwd).join(path).canonicalize() {
                return Ok(p);
            }
        }
        return Err(format!(
            "cannot share '{}': {err} (deleted working directory? run: cd \"$PWD\")",
            path.display()
        ));
    }
    Err(format!("cannot share '{}': {err}", path.display()))
}
