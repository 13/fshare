# fshare Polish + Release Prep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** File-type icons, generated logo (README + favicon), local multi-target build script, dormant tag-release GitHub workflow, AUR PKGBUILDs (source + bin), LICENSE.

**Architecture:** Icons: pure `icon_for` fn in `listing.rs`. Logo: hand-written SVG committed to `assets/logo.svg`, base64-embedded favicon in `listing.html`. Release: standalone `build.sh`, `.github/workflows/release.yml` (matrix gnu/musl/aarch64-musl), `packaging/aur/{fshare,fshare-bin}/PKGBUILD` with `_ghowner=CHANGEME`.

**Tech Stack:** bash, GitHub Actions, makepkg conventions. No new Rust deps.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-10-fshare-polish-release-design.md`.
- Logo = my geometric folder+arcs design; `assets/fshare.{svg,png}` remain but unreferenced.
- Workflow repo-agnostic, inert until GitHub remote + `v*` tag pushed.
- build.sh skips unavailable targets gracefully, never fails on missing cross toolchain.
- Existing 34 tests stay green.

---

### Task 1: File-type icons

**Files:**
- Modify: `src/listing.rs`

**Interfaces:**
- Produces: `listing::icon_for(name: &str, is_dir: bool) -> &'static str`.

- [ ] **Step 1: Failing test** (append in `listing.rs` tests):

```rust
    #[test]
    fn icons_by_extension() {
        assert_eq!(icon_for("x", true), "📁");
        assert_eq!(icon_for("a.PNG", false), "🖼️");
        assert_eq!(icon_for("m.mkv", false), "🎬");
        assert_eq!(icon_for("s.flac", false), "🎵");
        assert_eq!(icon_for("z.tar", false), "📦");
        assert_eq!(icon_for("d.pdf", false), "📕");
        assert_eq!(icon_for("c.rs", false), "💻");
        assert_eq!(icon_for("n.md", false), "📝");
        assert_eq!(icon_for("b.iso", false), "⚙️");
        assert_eq!(icon_for("k.pem", false), "🔑");
        assert_eq!(icon_for("unknown.qqq", false), "📄");
        assert_eq!(icon_for("noext", false), "📄");
    }
```

- [ ] **Step 2: Implement**

```rust
pub fn icon_for(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "📁";
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "avif" | "heic" => "🖼️",
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" => "🎬",
        "mp3" | "flac" | "ogg" | "wav" | "m4a" | "opus" | "aac" => "🎵",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" => "📦",
        "pdf" | "epub" | "mobi" => "📕",
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "c" | "h" | "cpp" | "hpp" | "go"
        | "java" | "kt" | "rb" | "php" | "sh" | "zsh" | "lua" | "sql" | "html" | "css"
        | "json" | "yaml" | "yml" | "toml" | "xml" => "💻",
        "md" | "txt" | "rst" | "org" | "log" => "📝",
        "bin" | "so" | "exe" | "dll" | "deb" | "rpm" | "appimage" | "iso" | "img" => "⚙️",
        "pem" | "key" | "crt" | "pub" | "asc" | "gpg" => "🔑",
        _ => "📄",
    }
}
```

In `render_html`, replace the hardcoded pair:

```rust
        let icon = icon_for(&e.name, e.is_dir);
        let (href, size, sort_size) = if e.is_dir {
            (format!("{dir_url}/{name_enc}/"), String::new(), 0)
        } else {
            (format!("{dir_url}/{name_enc}"), human_size(e.size), e.size)
        };
```

(row format string unchanged, still interpolates `{icon}`.)

- [ ] **Step 3:** `cargo test listing::` PASS.
- [ ] **Step 4:** `git commit -am "feat: file-type icons in listing"`

---

### Task 2: Logo + favicon

**Files:**
- Create: `assets/logo.svg`; Modify: `README.md`, `src/listing.html`

- [ ] **Step 1: Write `assets/logo.svg`** (complete file):

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96" width="96" height="96">
  <!-- folder -->
  <path fill="#0b6cff" d="M12 30c0-3.3 2.7-6 6-6h16l8 8h26c3.3 0 6 2.7 6 6v6H12v-14z"/>
  <path fill="#0b6cff" d="M12 40h62c3.3 0 6 2.7 6 6l-4 26c-.4 3-3 5-6 5H18c-3 0-5.6-2-6-5l-4-26c-.5-3.2 2.7-6 6-6z" opacity=".85"/>
  <!-- share arcs from top-right -->
  <g fill="none" stroke="#0b6cff" stroke-width="5" stroke-linecap="round">
    <path d="M70 22a14 14 0 0 1 14 -14" opacity=".45"/>
    <path d="M70 14a22 22 0 0 1 22 -22" transform="translate(0,8)" opacity=".7"/>
  </g>
  <circle cx="70" cy="22" r="4" fill="#0b6cff"/>
</svg>
```

(Adjust arc coordinates if renderer clips — verify by opening in browser/eog; arcs must stay inside viewBox.)

- [ ] **Step 2: README** — replace `# fshare` heading with:

```markdown
<img src="assets/logo.svg" width="96" align="left">

# fshare

Modern LAN file sharing over HTTP — a better `python3 -m http.server`.
<br clear="left">
```

- [ ] **Step 3: favicon** — base64 the logo, add to `listing.html` head:

```bash
b64=$(base64 -w0 assets/logo.svg)
```

insert line after `<title>` in `src/listing.html`:

```html
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml;base64,<B64>">
```

(replace `<B64>` with actual output; do via script to avoid manual paste errors.)

- [ ] **Step 4:** `cargo test` green (template change compiles via include_str). Serve manually, check favicon in browser tab optional.
- [ ] **Step 5:** `git commit -am "feat: logo, README header, listing favicon"`

---

### Task 3: build.sh

**Files:**
- Create: `build.sh` (chmod +x), `LICENSE`

- [ ] **Step 1: LICENSE** — standard MIT text, `Copyright (c) 2026 Ben Egger`.

- [ ] **Step 2: build.sh** (complete file):

```bash
#!/usr/bin/env bash
# Build release tarballs for all locally available targets into dist/.
set -euo pipefail
cd "$(dirname "$0")"

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
host_target=$(rustc -vV | awk '/^host:/ {print $2}')
targets=("$host_target" x86_64-unknown-linux-musl aarch64-unknown-linux-musl)

echo "== fshare v$version — running tests"
cargo test --quiet

mkdir -p dist
rm -f dist/sha256sums.txt

have_target() { rustup target list --installed | grep -qx "$1"; }
have_linker() {
    case "$1" in
        x86_64-unknown-linux-musl) command -v musl-gcc >/dev/null ;;
        aarch64-unknown-linux-musl) command -v aarch64-linux-gnu-gcc >/dev/null \
            || command -v aarch64-linux-musl-gcc >/dev/null ;;
        *) true ;;
    esac
}

built=()
for target in "${targets[@]}"; do
    if ! have_target "$target"; then
        echo "-- skip $target (rustup target not installed: rustup target add $target)"
        continue
    fi
    if ! have_linker "$target"; then
        echo "-- skip $target (linker missing)"
        continue
    fi
    echo "== building $target"
    cargo build --release --locked --target "$target"
    bin="target/$target/release/fshare"
    out="dist/fshare-v$version-$target.tar.gz"
    tar czf "$out" -C "$(dirname "$bin")" fshare -C "$OLDPWD" README.md LICENSE
    built+=("$out")
done

if [ ${#built[@]} -eq 0 ]; then
    echo "no targets built" >&2
    exit 1
fi

(cd dist && sha256sum $(basename -a "${built[@]}") > sha256sums.txt)
echo "== done:"
ls -l dist/
```

- [ ] **Step 3: Run** — `./build.sh` must produce host tarball + sha256sums.txt, exit 0 even when musl targets skipped.
- [ ] **Step 4:** `git add build.sh LICENSE && git commit -m "build: local multi-target release script, MIT license"`

---

### Task 4: GitHub Actions release workflow

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Write workflow** (complete file):

```yaml
name: release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --all-targets

  build:
    needs: test
    runs-on: ubuntu-latest
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            cross: false
          - target: x86_64-unknown-linux-musl
            cross: false
          - target: aarch64-unknown-linux-musl
            cross: true
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: install musl tools
        if: matrix.target == 'x86_64-unknown-linux-musl'
        run: sudo apt-get update && sudo apt-get install -y musl-tools
      - name: install cross
        if: matrix.cross
        run: cargo install cross --locked
      - name: build
        run: |
          if [ "${{ matrix.cross }}" = "true" ]; then
            cross build --release --locked --target ${{ matrix.target }}
          else
            cargo build --release --locked --target ${{ matrix.target }}
          fi
      - name: package
        run: |
          tar czf fshare-${{ github.ref_name }}-${{ matrix.target }}.tar.gz \
            -C target/${{ matrix.target }}/release fshare -C "$GITHUB_WORKSPACE" README.md LICENSE
      - uses: actions/upload-artifact@v4
        with:
          name: tarball-${{ matrix.target }}
          path: fshare-*.tar.gz

  release:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          merge-multiple: true
      - run: sha256sum fshare-*.tar.gz > sha256sums.txt
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            fshare-*.tar.gz
            sha256sums.txt
          generate_release_notes: true
```

- [ ] **Step 2: Validate** — `actionlint` if installed, else `python3 -c "import yaml;yaml.safe_load(open('.github/workflows/release.yml'))"`.
- [ ] **Step 3:** `git add .github && git commit -m "ci: tag-triggered release workflow (linux gnu/musl/aarch64)"`

---

### Task 5: AUR packaging

**Files:**
- Create: `packaging/aur/fshare/PKGBUILD`, `packaging/aur/fshare-bin/PKGBUILD`, `packaging/aur/README.md`
- Modify: `README.md` (install section mentions AUR once published)

- [ ] **Step 1: source PKGBUILD** (`packaging/aur/fshare/PKGBUILD`):

```bash
# Maintainer: Ben Egger <eggerben@gmail.com>
_ghowner=CHANGEME   # set to the GitHub owner before pushing to AUR
pkgname=fshare
pkgver=0.1.0
pkgrel=1
pkgdesc="Modern LAN file sharing over HTTP — a better python3 -m http.server"
arch=(x86_64 aarch64)
url="https://github.com/${_ghowner}/fshare"
license=(MIT)
depends=(gcc-libs)
makedepends=(cargo)
source=("$pkgname-$pkgver.tar.gz::$url/archive/v$pkgver.tar.gz")
sha256sums=('SKIP')  # run updpkgsums after the release exists

prepare() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  cargo build --frozen --release
}

check() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  cargo test --frozen --release
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 target/release/fshare "$pkgdir/usr/bin/fshare"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
```

- [ ] **Step 2: bin PKGBUILD** (`packaging/aur/fshare-bin/PKGBUILD`):

```bash
# Maintainer: Ben Egger <eggerben@gmail.com>
_ghowner=CHANGEME   # set to the GitHub owner before pushing to AUR
pkgname=fshare-bin
_pkgname=fshare
pkgver=0.1.0
pkgrel=1
pkgdesc="Modern LAN file sharing over HTTP — a better python3 -m http.server (binary release)"
arch=(x86_64)
url="https://github.com/${_ghowner}/fshare"
license=(MIT)
provides=(fshare)
conflicts=(fshare)
source=("$url/releases/download/v$pkgver/fshare-v$pkgver-x86_64-unknown-linux-musl.tar.gz")
sha256sums=('SKIP')  # run updpkgsums after the release exists

package() {
  install -Dm755 fshare "$pkgdir/usr/bin/fshare"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
```

- [ ] **Step 3: packaging/aur/README.md** — steps: set `_ghowner`, bump `pkgver`, `updpkgsums`, `makepkg --printsrcinfo > .SRCINFO`, test `makepkg -si`, push to `ssh://aur@aur.archlinux.org/<pkg>.git`.

- [ ] **Step 4: Validate** — `bash -n` both PKGBUILDs; `namcap` optional.
- [ ] **Step 5:** `git add packaging && git commit -m "packaging: AUR PKGBUILDs (source + bin)"`

---

## Self-Review Notes

- Spec coverage: icons table→match arms 1:1 (T1), logo+README+favicon (T2), build.sh skip logic + dist naming (T3), workflow matrix/jobs (T4), PKGBUILDs + _ghowner + AUR readme + LICENSE (T3/T5).
- Naming consistency: tarball `fshare-v<ver>-<target>.tar.gz` used by build.sh, workflow (`github.ref_name` = `v0.1.0` → same shape), and fshare-bin source URL. Consistent.
- `tar -C ... -C "$OLDPWD"` trick: second -C returns to repo root for README/LICENSE. In workflow uses $GITHUB_WORKSPACE.
```
