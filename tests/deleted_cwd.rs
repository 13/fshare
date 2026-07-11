//! Own integration-test binary: these tests mutate the process cwd and $PWD,
//! so they must not run concurrently with anything else — serialized via LOCK.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

#[test]
fn resolve_root_falls_back_to_pwd_when_cwd_deleted() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let base = std::env::temp_dir().join(format!("fshare-delcwd-{}", std::process::id()));
    let dir = base.join("newfolder");
    fs::create_dir_all(&dir).unwrap();
    let canonical = dir.canonicalize().unwrap();

    // emulate: shell sits in the dir, dir is deleted and recreated elsewhere
    std::env::set_current_dir(&dir).unwrap();
    fs::remove_dir(&dir).unwrap();
    fs::create_dir(&dir).unwrap();
    std::env::set_var("PWD", &dir);

    // canonicalize(".") resolves via the dead inode and must fail...
    assert!(Path::new(".").canonicalize().is_err());
    // ...but resolve_root recovers through the logical $PWD
    let root = fshare::fsutil::resolve_root(Path::new(".")).unwrap();
    assert_eq!(root, canonical);

    std::env::set_current_dir(&base).unwrap();
    fs::remove_dir_all(&base).ok();
}

#[test]
fn resolve_root_error_hints_at_deleted_cwd() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let base = std::env::temp_dir().join(format!("fshare-delcwd-hint-{}", std::process::id()));
    let dir = base.join("gone");
    fs::create_dir_all(&dir).unwrap();

    std::env::set_current_dir(&dir).unwrap();
    fs::remove_dir(&dir).unwrap();
    // no recreated dir this time: $PWD points nowhere
    std::env::set_var("PWD", &dir);

    let err = fshare::fsutil::resolve_root(Path::new(".")).unwrap_err();
    assert!(err.contains("cannot share"), "got: {err}");
    assert!(err.contains("cd \"$PWD\""), "hint missing: {err}");

    std::env::set_current_dir(&base).unwrap();
    fs::remove_dir_all(&base).ok();
}

#[test]
fn resolve_root_plain_missing_path_has_no_cwd_hint() {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let err =
        fshare::fsutil::resolve_root(Path::new("/definitely/not/here-fshare-test")).unwrap_err();
    assert!(err.contains("cannot share"), "got: {err}");
    assert!(!err.contains("cd \"$PWD\""), "hint wrongly added: {err}");
}
