use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

/// Settings mutable at runtime (from the TUI) and read per-request.
/// Booleans use Relaxed ordering: toggles are independent, no ordering
/// relationship between settings is relied upon.
pub struct LiveSettings {
    pub mdns: AtomicBool,
    pub upload: AtomicBool,
    pub hidden: AtomicBool,
    pub dir_sizes: AtomicBool,
    pub zip: AtomicBool,
    /// True once the listener actually serves HTTPS. Set by the server
    /// supervisor (startup --tls, or live enable via the TUI's secure key).
    pub tls: AtomicBool,
    pub auth: RwLock<Option<String>>, // "user:pass", None = off
    pub base: RwLock<String>,         // "" or "/s/<token>"
}

impl LiveSettings {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mdns: bool,
        upload: bool,
        hidden: bool,
        dir_sizes: bool,
        zip: bool,
        auth: Option<String>,
        base: String,
    ) -> Self {
        Self {
            mdns: AtomicBool::new(mdns),
            upload: AtomicBool::new(upload),
            hidden: AtomicBool::new(hidden),
            dir_sizes: AtomicBool::new(dir_sizes),
            zip: AtomicBool::new(zip),
            tls: AtomicBool::new(false),
            auth: RwLock::new(auth),
            base: RwLock::new(base),
        }
    }

    pub fn base(&self) -> String {
        self.base.read().unwrap().clone()
    }

    pub fn auth(&self) -> Option<String> {
        self.auth.read().unwrap().clone()
    }

    /// on = install a NEW random token (old links die); off = plain base.
    /// Returns the new base.
    pub fn set_token(&self, on: bool) -> String {
        let b = if on { format!("/s/{}", crate::server::gen_token()) } else { String::new() };
        *self.base.write().unwrap() = b.clone();
        b
    }
}

/// Flip an AtomicBool, returning the NEW value.
pub fn toggle(flag: &AtomicBool) -> bool {
    !flag.fetch_xor(true, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> LiveSettings {
        LiveSettings::new(true, false, false, false, true, None, String::new())
    }

    #[test]
    fn toggle_flips() {
        let l = fresh();
        assert!(toggle(&l.upload)); // false -> true, returns true
        assert!(l.upload.load(Ordering::Relaxed));
        assert!(!toggle(&l.upload)); // true -> false, returns false
        assert!(!l.upload.load(Ordering::Relaxed));
    }

    #[test]
    fn set_token_regenerates_and_clears() {
        let l = fresh();
        let a = l.set_token(true);
        assert!(a.starts_with("/s/") && a.len() == 3 + 12);
        assert_eq!(l.base(), a);
        let b = l.set_token(true);
        assert_ne!(a, b, "regeneration must mint a new token");
        assert_eq!(l.set_token(false), "");
        assert_eq!(l.base(), "");
    }

    #[test]
    fn auth_clone_out() {
        let l = fresh();
        assert_eq!(l.auth(), None);
        *l.auth.write().unwrap() = Some("u:p".into());
        assert_eq!(l.auth(), Some("u:p".to_string()));
    }
}
