# fshare — `--tls` Design

Date: 2026-07-10
Status: Approved

## Purpose

Optional HTTPS with a persisted self-signed certificate, so basic-auth
credentials and file contents are encrypted on the LAN.

## Behavior

- `--tls` switches serving to HTTPS; banner, QR and mDNS line print
  `https://` URLs. Combines freely with `--auth`, `--token`, `--upload`.
- Certificate persisted in `$XDG_DATA_HOME/fshare/` (fallback
  `~/.local/share/fshare/`): `cert.pem`, `key.pem`.
  - Missing → generate with `rcgen`: self-signed, 3650-day validity, SANs =
    `fshare.local`, machine hostname, `localhost`, plus all current
    LAN/Other interface IPs (IP SANs).
  - Present → reuse without rewriting (browser certificate exception
    persists across runs). Regeneration = delete the directory (README).
- `key.pem` written with mode 0600.
- Banner prints the certificate SHA-256 fingerprint (hex, colon-separated)
  so the user can match the browser warning:
  `note: TLS cert fingerprint SHA256: AB:CD:…`
- TLS setup failure (unwritable dir, corrupt PEM) is FATAL with a clear
  message — never silently fall back to plain HTTP.

## Architecture

- New `src/tls.rs`:
  - `pub struct TlsPaths { pub cert: PathBuf, pub key: PathBuf, pub fingerprint: String, pub generated: bool }`
  - `pub fn load_or_generate(dir: &Path, sans: &[String]) -> Result<TlsPaths, String>` —
    dir injected for testability; production caller passes
    `data_dir() = $XDG_DATA_HOME/fshare | ~/.local/share/fshare`.
  - `pub fn data_dir() -> PathBuf`
  - Fingerprint = SHA-256 over the certificate DER (parse PEM body).
- `src/main.rs`: when `args.tls`, build SAN list (`fshare.local`, hostname,
  `localhost`, `ranked_ifaces()` non-loopback IPs), call `load_or_generate`,
  serve via `axum_server::from_tcp_rustls` with
  `RustlsConfig::from_pem_file`; plain path keeps `axum::serve`. Both sit in
  the existing `tokio::select!` with expiry and Ctrl-C.
- `src/cli.rs`: `--tls` bool.
- URL scheme threaded to banner (`http`/`https` chosen once).

## Dependencies

`axum-server` (features `tls-rustls`), `rcgen`, `sha2`.

## Testing

- Unit (`tls.rs`, tempdir): generate creates cert.pem + key.pem, key mode
  0600, cert PEM parseable and SAN list contains `fshare.local`; second
  call returns same fingerprint with `generated == false` and does not
  rewrite files (compare mtimes or content).
- Integration: spawn HTTPS server on ephemeral port with generated cert,
  `reqwest` client with `danger_accept_invalid_certs(true)` fetches listing
  and file (200, correct body); plain `http://` request to the same port
  errors.

## Out of scope

User-provided `--cert`/`--key`, ACME/Let's Encrypt, cert rotation, HSTS.
