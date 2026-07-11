<img src="assets/logo.svg" width="96" align="left">

# fshare

Modern LAN file sharing over HTTP — a better `python3 -m http.server`.
<br clear="left">

Run it in any directory; get shareable URLs (LAN addresses first), a QR code
for phones, a live log of who connects and what they download, streamed zip
downloads of whole folders, and temporary-sharing controls.

```
  fshare v0.1.0 — serving /home/ben/photos (142 files, 1.3 GB)

  ➜ http://192.168.1.5:8000     (LAN, wlan0)
    http://localhost:8000       (lo)

  [QR code]

  Ctrl+C to stop

12:04:15  192.168.1.23 (phone)  GET /vid.mp4  200
12:04:29  192.168.1.23 (phone)  ✓ /vid.mp4 complete  312 MB in 14s  22 MB/s
```

## Install

One-liner (latest release binary into `~/.local/bin`, x86_64 or aarch64):

```bash
mkdir -p ~/.local/bin && curl -sL $(curl -s https://api.github.com/repos/13/fshare/releases/latest | grep -o "https://[^\"]*$(uname -m)-unknown-linux-musl\.tar\.gz") | tar xz -C ~/.local/bin fshare
```

From source:

```bash
cargo install --path .        # from source
./build.sh                    # release tarballs into dist/
```

Arch Linux: AUR packages `fshare` / `fshare-bin` (PKGBUILDs in
`packaging/aur/`, published once the GitHub repo is live).

## Configuration

Persistent defaults live in `~/.config/fshare/config.toml` (or
`$XDG_CONFIG_HOME/fshare/config.toml`; `FSHARE_CONFIG=<path>` overrides):

```toml
port = 9000
mdns = false          # don't announce on the network
upload = true
limit = "5MB"         # total download bandwidth
auth = "ben:secret"   # or `auth = true` for a generated password
tls = true
```

CLI flags always win — every boolean has an inverse (`--mdns/--no-mdns`,
`--tls/--no-tls`, …), and `--limit 0` lifts a configured limit.
Per-share options (`--token`, `--timeout`, `--max-downloads`) are CLI-only.

## Sharing on a public network

```sh
fshare --secure
```

One flag enables TLS, HTTP Basic auth with a generated password, a random
token URL, and turns mDNS announcement off. Anything you set explicitly
(e.g. `--auth bob:pw` or `mdns = true` in the config) wins over the bundle.

## Usage

```bash
fshare                      # share current directory on port 8000 (auto-bumps if busy)
fshare ~/photos             # share a directory
fshare big.iso              # share a single file (direct download link)

fshare --port 9000          # exact port, error if busy
fshare --bind 127.0.0.1     # bind address (default 0.0.0.0)
fshare --timeout 30m        # auto-shutdown after 30 minutes
fshare --max-downloads 3    # stop after 3 completed downloads
fshare --token              # random /s/<token>/ URL prefix (guessing protection)
fshare --hidden             # also serve dotfiles (hidden by default)
fshare --dir-sizes          # show recursive folder sizes in listings
fshare --no-zip             # disable folder zip downloads
fshare --no-qr              # skip the QR code
fshare --no-mdns            # skip fshare-<hostname>.local announcement
fshare --tls                # HTTPS with persisted self-signed cert
fshare --limit 5M           # cap total download speed (all clients combined)
fshare --json-log           # JSON-lines event log for scripting
fshare --upload             # allow uploads into the browsed folder (drag & drop)
fshare --upload --max-upload-size 2G
fshare --auth               # basic auth, generated password shown in banner
fshare --auth=ben:secret    # explicit credentials
fshare --follow-links       # allow symlinks leaving the shared root (off by default)
```

Extras:

- `GET /path/?format=json` — machine-readable directory listing
- `GET /path/?zip` — streamed zip of that folder (no temp files)
- Range requests supported: browser video seeking and download resume work
- Announces `http://fshare-<hostname>.local:8000` via mDNS (zero-config, `--no-mdns` to disable)
- Global download speed cap (`--limit 5M`)
- Detects other running fshare instances and shows them at startup
- Shutdown prints a summary: requests served, unique clients, bytes sent

## Security notes

- Read-only by construction — no write endpoints exist unless `--upload`.
- Uploads are opt-in (`--upload`); filenames are sanitized to their final
  component, collisions never overwrite (auto-rename), size capped with
  `--max-upload-size`.
- Path traversal blocked: every request is resolved and must stay inside the
  shared root; symlinks pointing outside are refused unless `--follow-links`.
- Dotfiles are hidden from listings *and* direct fetch unless `--hidden`.
- `--token` protects against casual URL guessing on shared networks; it is
  not authentication.
- `--auth` adds HTTP Basic authentication (constant-time verified). Over
  plain HTTP credentials travel base64-encoded — fine for a trusted LAN;
  add `--tls` to encrypt them.
- `--tls` serves HTTPS with a self-signed certificate persisted in
  `~/.local/share/fshare/` (delete the directory to regenerate). The
  SHA-256 fingerprint is printed at startup so you can match the browser
  warning.

## Roadmap

- Airdrop-style push between fshare instances
