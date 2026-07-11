# fshare: Config File, Secure Mode, Per-Machine mDNS — Design

Date: 2026-07-11
Status: approved

## Problem

1. Every PC running fshare announces the same mDNS host `fshare.local` — with
   multiple machines on one network, resolution races and the URL points at an
   arbitrary instance.
2. On a public network (cafe wifi), a safe share requires remembering three
   flags (`--tls --auth --token`) and disabling mDNS by hand.
3. No way to persist preferences (e.g. "mDNS always off") — every default must
   be re-typed per run.

## Feature 1: Config file `~/.config/fshare/config.toml`

- Location: `$XDG_CONFIG_HOME/fshare/config.toml`, falling back to
  `~/.config/fshare/config.toml`. Missing file = all defaults, no error.
- `FSHARE_CONFIG=<path>` env var overrides the location (`/dev/null` or a
  nonexistent path = no config); used by tests for isolation.
- Format: TOML, parsed with `serde` + `toml`.
- Allowed keys — persistent preferences only:

  ```toml
  port = 9000            # u16
  bind = "0.0.0.0"       # IpAddr
  hidden = false         # bool — show hidden files
  follow_links = false   # bool
  dir_sizes = false      # bool
  qr = true              # bool — show QR code
  zip = true             # bool — offer zip download
  upload = false         # bool
  max_upload_size = "2GB"  # string, same parser as CLI
  auth = "user:pass"     # string, or true for generated password
  tls = false            # bool
  limit = "5MB"          # string, same parser as --limit; absent = unlimited
  mdns = true            # bool
  json_log = false       # bool
  secure = false         # bool — see Feature 2
  ```

- Explicitly excluded (per-share, must stay CLI-only): `path`, `token`,
  `timeout`, `max_downloads`.
- Booleans are positive-named in the file (`mdns = false`, not
  `no_mdns = true`).
- Unknown key: hard error naming the file and key (`serde` deny_unknown_fields).
  Malformed TOML: hard error with parse message. No silent ignoring.
- Precedence: **CLI flag > config value > built-in default.**

### CLI negation pairs

So the CLI can flip a config boolean either way, each boolean gains a
positive/negative pair wired with clap `overrides_with`:

`--mdns/--no-mdns`, `--tls/--no-tls`, `--upload/--no-upload`,
`--qr/--no-qr`, `--zip/--no-zip`, `--hidden/--no-hidden`,
`--dir-sizes/--no-dir-sizes`, `--follow-links/--no-follow-links`,
`--auth[=..]/--no-auth`, `--secure/--no-secure`, `--json-log/--no-json-log`.

Existing flag spellings (`--no-mdns`, `--no-qr`, `--no-zip`, `--hidden`,
`--dir-sizes`, `--follow-links`, `--upload`, `--tls`, `--auth`) keep working —
only their inverses are new. Value flags override by giving a new value;
`--limit 0` now means "unlimited" (overrides a config limit).

### Architecture

- New `src/config.rs`:
  - `Config` — serde struct mirroring the key table, all fields `Option<T>`.
  - `Config::load(path: Option<&Path>) -> Result<Config, String>` — reads the
    file, default path from XDG; `None` config when file absent.
  - `resolve(cli: &cli::Args, cfg: &Config) -> Settings` — pure merge
    producing the effective settings struct consumed by `main.rs`.
- `cli::Args` booleans become tri-state (each pair maps to `Option<bool>`:
  set / unset / absent) so `resolve` can tell "flag absent" from "flag off".
- Banner prints `note: loaded <path>` when a config file was read.

## Feature 2: `--secure` bundle

- `--secure` (or `secure = true` in config) expands to:
  `tls = on`, `auth = on` (generated password unless credentials given),
  `token = on`, `mdns = off`.
- Expansion runs **after** the CLI/config merge; any setting the user set
  explicitly (either source) wins over the bundle. Examples:
  - `--secure --auth bob:pw` — TLS + token + mDNS off, auth is bob:pw.
  - `--secure --mdns` — everything secure except mDNS stays on.
- Banner note spells out what secure mode enabled, e.g.
  `note: secure mode — TLS on, auth on, token URL, mDNS off`.
- README gains a "Sharing on a public network" section.

## Feature 3: Per-machine mDNS hostname

- Announce `fshare-<hostname>.local` instead of the shared `fshare.local`.
- `<hostname>` = machine hostname sanitized: lowercase; every char outside
  `[a-z0-9-]` becomes `-`; runs of `-` collapsed; leading/trailing `-`
  trimmed; empty result falls back to `host`. Example: `Ben's PC` →
  `fshare-ben-s-pc.local`.
- Two PCs never collide. Two instances on the same PC share the host record
  (same IPs) and differ by port — no conflict.
- Banner shows the new hostname URL. The DNS-SD instance name keeps the
  human-readable raw hostname ("fshare on ben-pc"); the QR code keeps
  encoding the best interface-IP URL (phones resolve IPs more reliably
  than `.local` names). The mDNS TXT `path` property never carries the
  token prefix — it always announces `/`.
- TLS: the SAN list is persisted next to the cert (`sans.txt`). On startup,
  if any requested SAN is missing from the stored list, the cert is
  regenerated (existing "newly generated" fingerprint note covers UX).

## Testing

- `config.rs` unit tests: load (missing file, unknown key error, malformed
  TOML error), precedence matrix (CLI beats config beats default, negation
  flags both directions), value-key override incl. `--limit 0`.
- Secure expansion unit tests: plain `--secure`, explicit overrides from CLI
  and from config.
- mDNS: `sanitize_hostname` unit tests; existing `instance_name` tests
  updated; ignored browse-back test updated to new hostname.
- TLS: unit test — SAN change triggers regeneration, unchanged SANs reuse.
- Integration tests unaffected (config path injectable so tests never read
  the real user config; tests pass explicit `--config /dev/null`-style
  isolation via `FSHARE_CONFIG` env var override).

## Implementation order

One branch, three tasks: (1) config file + tri-state flags, (2) `--secure`
bundle, (3) mDNS hostname rename + TLS SAN regeneration.
