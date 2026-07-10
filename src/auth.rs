use rand::Rng;

const PW_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn gen_password() -> String {
    let mut rng = rand::thread_rng();
    (0..10)
        .map(|_| PW_ALPHABET[rng.gen_range(0..PW_ALPHABET.len())] as char)
        .collect()
}

/// `arg` is the value of `--auth=...`; `None` means bare `--auth`.
pub fn parse_auth(arg: &Option<String>) -> Result<String, String> {
    let (user, pass) = match arg {
        None => ("fshare".to_string(), None),
        Some(v) => match v.split_once(':') {
            Some((u, p)) => (u.to_string(), Some(p.to_string())),
            None => (v.clone(), None),
        },
    };
    if user.is_empty() {
        return Err("--auth: user must not be empty (use --auth=user[:pass])".into());
    }
    let pass = pass.unwrap_or_else(gen_password);
    Ok(format!("{user}:{pass}"))
}

/// Constant-time, length-independent comparison: iteration count depends
/// only on the longer input, never on where the first mismatch occurs.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credentials() {
        let full = parse_auth(&Some("ben:secret".into())).unwrap();
        assert_eq!(full, "ben:secret");
        let colons = parse_auth(&Some("ben:se:cret".into())).unwrap();
        assert_eq!(colons, "ben:se:cret");
        let user_only = parse_auth(&Some("ben".into())).unwrap();
        assert!(user_only.starts_with("ben:") && user_only.len() == 4 + 10);
        let bare = parse_auth(&None).unwrap();
        assert!(bare.starts_with("fshare:") && bare.len() == 7 + 10);
        assert!(parse_auth(&Some("".into())).is_err());
        assert!(parse_auth(&Some(":x".into())).is_err());
    }

    #[test]
    fn password_alphabet_safe() {
        for _ in 0..50 {
            let p = gen_password();
            assert_eq!(p.len(), 10);
            assert!(!p.chars().any(|c| "0O1lI".contains(c)), "{p}");
        }
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"x"));
        assert!(ct_eq(b"", b""));
    }
}
