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
    out="dist/fshare-v$version-$target.tar.gz"
    tar czf "$out" -C "target/$target/release" fshare -C "$PWD" README.md LICENSE
    built+=("$(basename "$out")")
done

if [ ${#built[@]} -eq 0 ]; then
    echo "no targets built" >&2
    exit 1
fi

(cd dist && sha256sum "${built[@]}" > sha256sums.txt)
echo "== done:"
ls -l dist/
