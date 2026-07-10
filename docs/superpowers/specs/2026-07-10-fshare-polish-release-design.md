# fshare — Polish + Release Prep Design

Date: 2026-07-10
Status: Approved

## Purpose

Five deliverables: file-type icons in the listing UI, project logo, local
multi-target build script, tag-triggered GitHub release workflow (dormant
until a GitHub remote exists), and AUR packaging (source + -bin).

## 1. File-type icons

`listing.rs`: `pub fn icon_for(name: &str, is_dir: bool) -> &'static str`.
Case-insensitive extension match:

| icon | extensions |
|------|-----------|
| 📁 | directories |
| 🖼️ | png jpg jpeg gif webp svg bmp ico avif heic |
| 🎬 | mp4 mkv webm avi mov m4v |
| 🎵 | mp3 flac ogg wav m4a opus aac |
| 📦 | zip tar gz tgz bz2 xz zst 7z rar |
| 📕 | pdf epub mobi |
| 💻 | rs py js ts jsx tsx c h cpp hpp go java kt rb php sh zsh lua sql html css json yaml yml toml xml |
| 📝 | md txt rst org log |
| ⚙️ | bin so exe dll deb rpm appimage iso img |
| 🔑 | pem key crt pub asc gpg |
| 📄 | everything else |

Row rendering in `render_html` switches from hardcoded 📁/📄 to `icon_for`.
Unit tests cover one of each class plus fallback + case-insensitivity.

## 2. Logo

- Generated: `assets/logo.svg` — hand-written geometric SVG, flat folder
  silhouette with three quarter-arcs radiating from its top-right corner
  (share/broadcast motif), primary color `#0b6cff`, no text, square
  viewBox, legible at 16px. (User-provided `assets/fshare.{svg,png}` stay
  in the repo but are not referenced.)
- README top: `<img src="assets/logo.svg" width="96">` beside the title.
- Listing template gains a favicon: base64 data-URI of `assets/logo.svg`
  embedded in `listing.html` (`<link rel="icon" type="image/svg+xml" ...>`).

## 3. build.sh (repo root, executable)

- `#!/usr/bin/env bash`, `set -euo pipefail`.
- Reads version from `Cargo.toml`.
- Runs `cargo test` first; abort on failure.
- Target list: host default + `x86_64-unknown-linux-musl` +
  `aarch64-unknown-linux-musl`. For each: skip with a printed note when
  `rustup target list --installed` lacks it or the needed linker is absent
  (`musl-gcc` for x86_64-musl, `aarch64-linux-gnu-gcc` or configured cargo
  linker for aarch64).
- Output `dist/fshare-<version>-<target>.tar.gz` (binary + README + LICENSE)
  and `dist/sha256sums.txt`.

## 4. GitHub Actions release workflow

- `.github/workflows/release.yml`, trigger `on: push: tags: ['v*']`.
- Job 1 `test`: ubuntu-latest, `cargo test --all-targets`.
- Job 2 `build` (needs test), matrix:
  - `x86_64-unknown-linux-gnu` (native)
  - `x86_64-unknown-linux-musl` (apt musl-tools)
  - `aarch64-unknown-linux-musl` (`cross`)
- Steps: checkout, rust toolchain + target, build `--release --locked`,
  tar.gz as `fshare-${tag}-${target}.tar.gz`, sha256.
- Job 3 `release` (needs build): download artifacts,
  `softprops/action-gh-release` attaches tarballs + `sha256sums.txt`.
- No hardcoded repo owner — workflow is inert until the repo is pushed to
  GitHub and a `v*` tag is pushed.
- Commit `Cargo.lock` (currently untracked? verify) so `--locked` works.

## 5. AUR packaging (`packaging/aur/`)

- `fshare/PKGBUILD`: source package. `pkgname=fshare`,
  `_ghowner=CHANGEME` (documented), source =
  `https://github.com/${_ghowner}/fshare/archive/v${pkgver}.tar.gz`,
  `makedepends=(cargo)`, standard cargo build/check/package per Arch Rust
  guidelines (`--frozen --release`, install binary + LICENSE + README).
- `fshare-bin/PKGBUILD`: binary package from the release asset
  `fshare-v${pkgver}-x86_64-unknown-linux-musl.tar.gz`,
  `provides=(fshare)`, `conflicts=(fshare)`.
- `packaging/aur/README.md`: how to set `_ghowner`, update `pkgver`,
  regenerate checksums (`updpkgsums`) and `.SRCINFO`
  (`makepkg --printsrcinfo > .SRCINFO`), and push to AUR.
- Add `LICENSE` (MIT, current year, Ben Egger) at repo root — Cargo.toml
  already declares MIT.

## Testing

- Icons: unit tests as above; existing HTML test updated if icon chars
  asserted.
- build.sh: run it — host target must produce dist tarball; script exits 0
  with musl/aarch64 skipped or built.
- Workflow: `actionlint` if available, else YAML parse check
  (`python -c "import yaml,sys;yaml.safe_load(open(...))"`).
- PKGBUILDs: `bash -n` syntax check; `namcap` if installed (optional).

## Out of scope

Creating the GitHub repo/pushing, actual AUR submission, macOS/Windows
builds, cross-compiling in build.sh beyond target-if-available.
