# fshare — mDNS Announcement Design

Date: 2026-07-10
Status: Approved

## Purpose

Zero-config discovery: `http://fshare.local:<port>/` resolvable on the LAN,
plus DNS-SD service visibility in network-discovery apps.

## Behavior

- Default ON; `--no-mdns` disables.
- Announces hostname `fshare.local` with A/AAAA records for all ranked
  non-loopback interface IPs (`net::ranked_ifaces()`, Lan + Other kinds).
- Registers DNS-SD service `_http._tcp.local.`:
  - instance name: `fshare on <machine-hostname>` for port 8000, else
    `fshare on <machine-hostname> (<port>)` — DNS-SD instance names must be
    unique per network; port suffix keeps concurrent instances distinct.
  - TXT record: `path=/<base>` (token prefix included when `--token`).
- Banner gains `➜ http://fshare.local:<port><base>/ (mDNS)` line above the
  IP list. QR keeps the numeric-IP URL (Android camera apps often cannot
  resolve `.local`).
- Machine hostname read via `hostname::get()` equivalent — use
  `std::fs::read_to_string("/etc/hostname")` trimmed, fallback `"host"`.

## Architecture

New module `src/mdns.rs` using crate `mdns-sd`:

- `pub fn announce(port: u16, base: &str) -> Result<MdnsGuard, String>` —
  creates `ServiceDaemon`, builds `ServiceInfo` (service type
  `_http._tcp.local.`, hostname `fshare.local.`, explicit IP list), calls
  `register`. Errors stringified.
- `pub struct MdnsGuard { daemon: ServiceDaemon, fullname: String }` —
  `Drop` calls `unregister(fullname)` then `shutdown()` (both best-effort).
- `pub fn instance_name(host: &str, port: u16) -> String` — pure helper per
  the naming rule above.
- `main.rs`: after banner-relevant data assembled, call `announce` unless
  `--no-mdns`; on `Err(e)` print
  `note: mDNS unavailable: <e>` (yellow) and continue. Guard held until
  shutdown.

## Error handling

mDNS failure is never fatal: any socket/registration error degrades to a
banner note. Guard drop errors ignored.

## Testing

- Unit: `instance_name("ben-pc", 8000)` → `"fshare on ben-pc"`;
  `instance_name("ben-pc", 8001)` → `"fshare on ben-pc (8001)"`.
- Integration (`#[ignore]`, manual — multicast unreliable in CI/sandbox):
  `announce` on ephemeral port, browse `_http._tcp.local.` with a second
  `mdns-sd` daemon, assert our instance appears within 3 s.
- Manual smoke: run fshare, `dig +short @224.0.0.251 -p 5353 fshare.local A`
  returns a LAN IP; banner shows the .local URL.

## Out of scope

Custom hostname flag, IPv6-only refinements, service browsing UI.
