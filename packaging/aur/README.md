# AUR packaging

Two packages:

- `fshare/` — source package, builds from the GitHub tag tarball with cargo
- `fshare-bin/` — installs the prebuilt x86_64 musl binary from GitHub releases

## Publishing a release to AUR

Prerequisite: the GitHub repo exists and a `v<version>` tag has been pushed
(the `release` workflow then attaches the binary tarballs).

1. Set `_ghowner=<github-owner>` in both PKGBUILDs (once).
2. Bump `pkgver` to the new version, reset `pkgrel=1`.
3. Fill checksums: `updpkgsums` (from pacman-contrib) inside each package dir.
4. Test build: `makepkg -si` (source) / `makepkg -si` (bin).
5. Regenerate metadata: `makepkg --printsrcinfo > .SRCINFO`.
6. Push to AUR:

```bash
git clone ssh://aur@aur.archlinux.org/fshare.git aur-fshare
cp fshare/PKGBUILD fshare/.SRCINFO aur-fshare/
cd aur-fshare && git add -A && git commit -m "v<version>" && git push
```

(same flow with `fshare-bin`.)
