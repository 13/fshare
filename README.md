# fshare

Modern LAN file sharing over HTTP — a better `python3 -m http.server`.

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

```bash
cargo install --path .
```

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
fshare --no-zip             # disable folder zip downloads
fshare --no-qr              # skip the QR code
fshare --json-log           # JSON-lines event log for scripting
fshare --upload             # allow uploads into the browsed folder (drag & drop)
fshare --upload --max-upload-size 2G
fshare --follow-links       # allow symlinks leaving the shared root (off by default)
```

Extras:

- `GET /path/?format=json` — machine-readable directory listing
- `GET /path/?zip` — streamed zip of that folder (no temp files)
- Range requests supported: browser video seeking and download resume work
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
- `--token` protects against casual URL guessing on shared networks. It is
  not authentication; treat anything shared on a hostile LAN as public.

## Roadmap

- Basic auth (`--user` / `--pass`)
- mDNS announcement (`fshare.local`)
- Optional self-signed TLS
- Bandwidth limiting
- Airdrop-style push between fshare instances
