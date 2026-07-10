# fshare — HTTP Basic Auth Design

Date: 2026-07-10
Status: Approved

## Purpose

Optional HTTP Basic authentication guarding every route.

## CLI

`--auth[=USER[:PASS]]` — clap `Option<Option<String>>` with
`require_equals = true` (bare `--auth` must not consume the positional DIR).

- `--auth` → user `fshare`, generated 10-char password
- `--auth=ben` → user `ben`, generated password
- `--auth=ben:secret` → explicit credentials (PASS may contain further
  colons; split on the first)
- `--auth=` (empty), empty user (`:x`) → startup error

Generated passwords: 10 chars from an unambiguous alphabet (no `0 O 1 l I`),
via `rand`.

## Mechanics

- `AppState.auth: Option<String>` holds precomputed `"user:pass"`; `None` =
  auth disabled, middleware not registered.
- Middleware (axum `from_fn_with_state`) layered so `track` remains outermost:
  401 responses appear in the connection log.
- Request must carry `Authorization: Basic <base64(user:pass)>`. Decode with
  the `base64` crate; any parse failure → 401 (never 500).
- Comparison is constant-time and length-independent (XOR fold over both
  byte strings; unequal lengths still traverse fully).
- Failure response: `401` with `WWW-Authenticate: Basic realm="fshare"` so
  browsers prompt.
- Guards all methods and routes: listing, files, zip, JSON, uploads.
  Independent of and combinable with `--token`.

## Banner

```
  note: auth enabled — user: fshare  password: kR7mXw2Pq4
```

Password always shown in the banner when generated; with explicit
credentials, show only `auth enabled (user ben)`. QR stays a plain URL —
the phone's browser prompts for credentials.

## Errors

- Bad `--auth` value: exit before binding with a clear message.
- Malformed/garbage Authorization header: 401.

## Testing

Unit: credential parsing (`ben`, `ben:secret`, `ben:se:cret`, empty, `:x`),
constant-time equality (equal, differing, different lengths).
Integration: request without header → 401 + `WWW-Authenticate`; wrong
password → 401; correct → 200 for listing, file, and upload POST; auth and
token combined both enforced.

## Out of scope

TLS (basic auth over plain HTTP is LAN-trust-level security — README note),
rate limiting, multiple users.
