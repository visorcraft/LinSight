# GitHub Actions CI + Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three GitHub Actions workflows (lean per-push CI, weekly security, tag-triggered build-everything release) plus the packaging repairs that make the release real, and sync all in-tree packaging versions to the workspace version.

**Architecture:** A single `ci.yml` job mirrors `just ci` on every push/PR. A `security.yml` runs `cargo-deny`/`cargo-audit` weekly. A `release.yml` fans out on a `v*` tag to build a portable tarball, Arch + arch-v3 pacman packages, a `.deb`, Fedora + openSUSE `.rpm`s, an AppImage, and a Flatpak bundle, then aggregates them into one GitHub Release. The tag is the source of version truth: each job patches the relevant packaging version field from it.

**Tech Stack:** GitHub Actions, Rust 1.95 (cxx-qt 0.8 / Qt 6 / Kirigami workspace), `makepkg`, `dpkg-buildpackage`, `rpmbuild`, `appimage-builder`, `flatpak-builder`, `actionlint` for local workflow validation.

**Branch:** Work happens on `ci/github-actions` (already created, spec already committed there).

**Workspace version at time of writing:** `1.7.0` (verify with `awk -F'"' '/^\[workspace\.package\]/{s=1;next} s&&/^\[/{exit} s&&$1~/^version/{print $2;exit}' Cargo.toml`).

---

## Pre-flight: install the workflow linter

`actionlint` is the validation tool used throughout (it checks workflow
syntax, expressions, and shellchecks every `run:` block). Install it once:

```bash
# Arch / CachyOS:
pacman -S actionlint            # if packaged, else:
go install github.com/rhysd/actionlint/cmd/actionlint@latest
# or download a release binary from github.com/rhysd/actionlint/releases
```

Fallback if `actionlint` cannot be installed — basic YAML syntax only:

```bash
python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1])); print('yaml ok')" <file>
```

---

## File Structure

**Created:**
- `.github/workflows/ci.yml` — per-push fmt+clippy+test gate
- `.github/workflows/security.yml` — weekly cargo-deny + cargo-audit
- `.github/workflows/release.yml` — tag-triggered build-everything + publish
- `scripts/build_appimage.sh` — shared AppImage build (used by `just appimage` and CI)

**Modified:**
- `packaging/fedora/linsight.spec` — `Version: 1.6.0` → `1.7.0`
- `packaging/opensuse/linsight.spec` — `Version: 1.6.0` → `1.7.0`
- `packaging/debian/changelog` — prepend `1.7.0-1` entry
- `packaging/appimage/AppImageBuilder.yml` — version `0.3.0` → `1.7.0`, populate `script:`
- `packaging/flatpak/io.visorcraft.LinSight.yml` — vendor-config write + `strip-components: 1`
- `Justfile` — new `appimage` recipe

---

## Task 1: Sync in-tree packaging versions to 1.7.0

**Files:**
- Modify: `packaging/fedora/linsight.spec`
- Modify: `packaging/opensuse/linsight.spec`
- Modify: `packaging/debian/changelog`

- [ ] **Step 1: Write the failing check**

Run this assertion that all RPM-style specs are on the workspace version:

```bash
ver="$(awk -F'"' '/^\[workspace\.package\]/{s=1;next} s&&/^\[/{exit} s&&$1~/^version/{print $2;exit}' Cargo.toml)"
echo "workspace=$ver"
grep -H '^Version:' packaging/fedora/linsight.spec packaging/opensuse/linsight.spec
head -1 packaging/debian/changelog
```

Expected NOW: fedora + opensuse show `1.6.0`, changelog shows
`linsight (1.6.0-1) ...` — i.e. they do **not** match `1.7.0`.

- [ ] **Step 2: Bump the Fedora spec**

Edit `packaging/fedora/linsight.spec`, change the `Version:` line:

```
Version:        1.7.0
```

- [ ] **Step 3: Bump the openSUSE spec**

Edit `packaging/opensuse/linsight.spec`, change the `Version:` line:

```
Version:        1.7.0
```

- [ ] **Step 4: Prepend a Debian changelog entry**

Prepend this block to the very top of `packaging/debian/changelog`
(keep all existing entries below it — do not rewrite history):

```
linsight (1.7.0-1) unstable; urgency=medium

  * Sync packaging version to 1.7.0.

 -- VisorCraft <29009015+visorcraft@users.noreply.github.com>  Mon, 01 Jun 2026 12:00:00 +0000

```

(2026-06-01 is a Monday, so the `Mon` day-name is correct and
`dpkg-parsechangelog` will not warn.)

- [ ] **Step 5: Run the check to verify it passes**

```bash
ver="$(awk -F'"' '/^\[workspace\.package\]/{s=1;next} s&&/^\[/{exit} s&&$1~/^version/{print $2;exit}' Cargo.toml)"
grep -H '^Version:' packaging/fedora/linsight.spec packaging/opensuse/linsight.spec
head -1 packaging/debian/changelog
grep -H '^pkgver=' packaging/arch/PKGBUILD packaging/arch-v3/PKGBUILD
```

Expected: fedora + opensuse `Version: 1.7.0`; changelog top
`linsight (1.7.0-1) ...`; both PKGBUILDs already `pkgver=1.7.0`. All five
agree with `$ver` (`1.7.0`). (AppImage's version is fixed in Task 5.)

- [ ] **Step 6: Commit**

```bash
git add packaging/fedora/linsight.spec packaging/opensuse/linsight.spec packaging/debian/changelog
git commit -m "chore: sync packaging versions to 1.7.0"
```

---

## Task 2: `ci.yml` — per-push fmt + clippy + test

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Confirm the underlying gate is green locally**

```bash
just ci
```

Expected: PASS — `fmt-check`, `clippy -D warnings`, and
`cargo test --workspace` all succeed (baseline 381 tests pass). If this
is red, stop and fix the tree first; CI will be red too.

- [ ] **Step 2: Verify actionlint fails on the missing file**

```bash
actionlint .github/workflows/ci.yml
```

Expected: FAIL (file does not exist yet).

- [ ] **Step 3: Create the workflow**

Create `.github/workflows/ci.yml`:

```yaml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Lean per-push gate: a single job mirroring `just ci`
# (fmt-check -> clippy -> test). Doc-only changes are path-ignored.
# No `${{ github.event.* }}` interpolation in run blocks (injection-safe).

name: ci

on:
  push:
    branches: [master]
    paths-ignore:
      - '**.md'
      - 'docs/**'
      - 'LICENSE'
      - 'packaging/screenshots/**'
  pull_request:
    branches: [master]
    paths-ignore:
      - '**.md'
      - 'docs/**'
      - 'LICENSE'
      - 'packaging/screenshots/**'

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  QMAKE: /usr/bin/qmake6
  QT_VERSION_MAJOR: "6"

jobs:
  test:
    name: fmt + clippy + test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Qt 6 + C++ toolchain
        run: |
          sudo apt-get update
          sudo apt-get install -y --no-install-recommends \
            qt6-base-dev \
            qt6-declarative-dev \
            qt6-tools-dev \
            libgl1-mesa-dev \
            clang
      - uses: dtolnay/rust-toolchain@1.95.0
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: Test
        run: cargo test --workspace --locked
```

- [ ] **Step 4: Validate**

```bash
actionlint .github/workflows/ci.yml
```

Expected: PASS (no output, exit 0).

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add per-push fmt + clippy + test workflow"
```

---

## Task 3: `security.yml` — weekly cargo-deny + cargo-audit

**Files:**
- Create: `.github/workflows/security.yml`

- [ ] **Step 1: Confirm the checks pass locally (if tools installed)**

```bash
cargo deny --all-features check || echo "(install cargo-deny: cargo install cargo-deny --locked)"
cargo audit || echo "(install cargo-audit: cargo install cargo-audit --locked)"
```

Expected: both pass, or are skipped because the tool isn't installed.
A genuine advisory/license failure here means the weekly job would also
fail — note it but it does not block creating the workflow.

- [ ] **Step 2: Create the workflow**

Create `.github/workflows/security.yml`:

```yaml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Supply-chain checks off the per-push path: weekly schedule + manual
# dispatch. Neither job compiles the workspace.

name: security

on:
  schedule:
    - cron: '0 6 * * 1'   # Mondays 06:00 UTC
  workflow_dispatch:

jobs:
  deny:
    name: cargo-deny
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2

  audit:
    name: cargo-audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install cargo-audit
        run: cargo install cargo-audit --locked
      - run: cargo audit
```

- [ ] **Step 3: Validate**

```bash
actionlint .github/workflows/security.yml
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/security.yml
git commit -m "ci: add weekly cargo-deny + cargo-audit workflow"
```

---

## Task 4: Fix the Flatpak manifest (vendor wiring)

The Flatpak module runs `cargo --offline build --locked` against vendored
sources, but two things are broken: (a) nothing points cargo at the
vendor dir, and (b) `vendor.tar.gz` (which has a top-level `vendor/`
prefix) is extracted with `strip-components: 0` into `dest: vendor`,
double-nesting it to `vendor/vendor/`. Fix both.

**Files:**
- Modify: `packaging/flatpak/io.visorcraft.LinSight.yml`

- [ ] **Step 1: Confirm vendoring produces a `vendor/`-prefixed archive**

```bash
grep -n 'flatpak-vendor:' -A4 Justfile
```

Expected: the recipe runs `cargo vendor --locked packaging/flatpak/vendor`
then `tar -C packaging/flatpak -cf packaging/flatpak/vendor.tar.gz vendor/`
— i.e. archive entries are prefixed with `vendor/`. (We keep this recipe
as-is and adjust the manifest's `strip-components` instead.)

- [ ] **Step 2: Add the vendor-config write to the build-commands**

In `packaging/flatpak/io.visorcraft.LinSight.yml`, replace the module's
`build-commands:` opening so the first thing it does is write a cargo
config redirecting crates.io to the vendored sources. Change:

```yaml
    build-commands:
      - cargo --offline build --workspace --release --locked
```

to:

```yaml
    build-commands:
      - mkdir -p "${CARGO_HOME}"
      - |
        cat > "${CARGO_HOME}/config.toml" <<EOF
        [source.crates-io]
        replace-with = "vendored-sources"
        [source.vendored-sources]
        directory = "$(pwd)/vendor"
        EOF
      - cargo --offline build --workspace --release --locked
```

(`CARGO_HOME` is already set to `/run/build/linsight/cargo` in
`build-options.env`. The heredoc uses unquoted `EOF` so only `$(pwd)`
expands — to the absolute build dir — making `directory` an absolute
path immune to cargo's relative-path resolution rules. Safe to hardcode
the crates.io-only replacement because `Cargo.lock` has no git sources.)

- [ ] **Step 3: Fix the double-nesting of the vendor archive**

In the same file, the `vendor.tar.gz` source currently reads:

```yaml
      - type: archive
        path: vendor.tar.gz
        dest: vendor
        strip-components: 0
```

Change `strip-components: 0` to `strip-components: 1` so the archive's
`vendor/` prefix is stripped and crates land at `<builddir>/vendor/<crate>`
(matching `directory = "$(pwd)/vendor"`):

```yaml
      - type: archive
        path: vendor.tar.gz
        dest: vendor
        strip-components: 1
```

- [ ] **Step 4: Validate YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('packaging/flatpak/io.visorcraft.LinSight.yml')); print('yaml ok')"
```

Expected: `yaml ok`.

- [ ] **Step 5: (Best-effort) local vendor sanity check**

```bash
cargo vendor --locked packaging/flatpak/vendor >/dev/null && \
  ls packaging/flatpak/vendor | head && echo "vendor populated ok"
rm -rf packaging/flatpak/vendor
```

Expected: a populated vendor dir (proves `cargo vendor --locked`
resolves). The full `flatpak-builder` build is validated end-to-end at
the rc-tag stage (Task 11) since it needs the KDE 6.10 runtime.

- [ ] **Step 6: Commit**

```bash
git add packaging/flatpak/io.visorcraft.LinSight.yml
git commit -m "fix(flatpak): wire vendored cargo sources + un-nest vendor archive"
```

---

## Task 5: Fix the AppImage recipe + add `just appimage`

The recipe is non-functional: empty `script:`, stale hardcoded
`version: 0.3.0`. Populate the build script, sync the committed version
to `1.7.0`, and add a shared build script that injects the version
dynamically so it never goes stale again.

**Files:**
- Modify: `packaging/appimage/AppImageBuilder.yml`
- Create: `scripts/build_appimage.sh`
- Modify: `Justfile`

- [ ] **Step 1: Populate the recipe script + sync the version**

In `packaging/appimage/AppImageBuilder.yml`:

(a) change the `app_info.version` from `0.3.0` to `1.7.0`:

```yaml
    version: 1.7.0
```

(b) replace `script: []` with the AppDir population steps (these run
from the repo root before appimage-builder bundles dependencies):

```yaml
script:
  - cargo build --workspace --release --locked
  - install -Dm755 target/release/linsight     AppDir/usr/bin/linsight
  - install -Dm755 target/release/linsightd    AppDir/usr/bin/linsightd
  - install -Dm755 target/release/linsight-cli AppDir/usr/bin/linsight-cli
  - install -Dm644 packaging/io.visorcraft.LinSight.desktop
      AppDir/usr/share/applications/io.visorcraft.LinSight.desktop
  - install -Dm644 packaging/io.visorcraft.LinSight.metainfo.xml
      AppDir/usr/share/metainfo/io.visorcraft.LinSight.metainfo.xml
  - install -Dm644 packaging/icons/scalable/apps/io.visorcraft.LinSight.svg
      AppDir/usr/share/icons/hicolor/scalable/apps/io.visorcraft.LinSight.svg
  - install -Dm644 packaging/icons/256x256/apps/io.visorcraft.LinSight.png
      AppDir/usr/share/icons/hicolor/256x256/apps/io.visorcraft.LinSight.png
```

(`linsightd` is bundled because the GUI auto-spawns it as a child; it
must sit next to `linsight` inside the AppImage.)

- [ ] **Step 2: Create the shared build script**

Create `scripts/build_appimage.sh`:

```bash
#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the LinSight AppImage via appimage-builder. The recipe's
# app_info.version is overwritten from the workspace Cargo.toml at build
# time so the produced AppImage is always versioned correctly, even if
# the committed recipe value lags. Requires appimage-builder on PATH.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="$(awk -F'"' '
    /^\[workspace\.package\]/ { in_section = 1; next }
    in_section && /^\[/ { exit }
    in_section && $1 ~ /^version[[:space:]]*=/ { print $2; exit }
' Cargo.toml)"

mkdir -p target/appimage
# Rewrite the 4-space-indented app_info.version line (the top-level
# recipe `version: 1` is at column 0, so it is untouched).
sed "s/^    version: .*/    version: ${version}/" \
    packaging/appimage/AppImageBuilder.yml \
    > target/appimage/AppImageBuilder.yml

rm -rf AppDir
appimage-builder --recipe target/appimage/AppImageBuilder.yml --skip-test
echo "AppImage built for version ${version}"
```

Make it executable:

```bash
chmod +x scripts/build_appimage.sh
```

- [ ] **Step 3: Add the `just appimage` recipe**

In `Justfile`, add this recipe (place it near the other `package`/`arch-pkg`
recipes):

```just
# Build the AppImage via appimage-builder. Injects the workspace version
# dynamically. Requires `appimage-builder` on PATH.
appimage:
    bash scripts/build_appimage.sh
```

- [ ] **Step 4: Validate the recipe parses + version synced**

```bash
python3 -c "import yaml; yaml.safe_load(open('packaging/appimage/AppImageBuilder.yml')); print('yaml ok')"
grep -nE '^    version:' packaging/appimage/AppImageBuilder.yml
just --summary | tr ' ' '\n' | grep -x appimage
bash -n scripts/build_appimage.sh && echo "script syntax ok"
```

Expected: `yaml ok`; the indented `version:` line shows `1.7.0`;
`appimage` appears in the just recipe list; `script syntax ok`.

- [ ] **Step 5: (Best-effort) local AppImage build**

Only if `appimage-builder` is installed locally:

```bash
command -v appimage-builder >/dev/null && just appimage || \
  echo "(appimage-builder not installed — validated in CI / rc-tag instead)"
```

Expected: either a produced `*.AppImage` in the repo root, or the skip
message. (CI builds this in `debian:trixie-slim`; full validation is at
the rc-tag stage.)

- [ ] **Step 6: Commit**

```bash
git add packaging/appimage/AppImageBuilder.yml scripts/build_appimage.sh Justfile
git commit -m "fix(appimage): populate recipe script + dynamic version, add just appimage"
```

---

## Task 6: `release.yml` — version resolution + tarball + publish

This is the minimal end-to-end release: resolve/verify the version, build
the portable tarball, and publish a GitHub Release. Later tasks add the
package jobs.

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Verify actionlint fails on the missing file**

```bash
actionlint .github/workflows/release.yml
```

Expected: FAIL (file does not exist yet).

- [ ] **Step 2: Create the workflow with the tarball + publish jobs**

Create `.github/workflows/release.yml`:

```yaml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Tag-triggered release: build every artifact LinSight ships and attach
# them to a GitHub Release. The tag is the source of version truth — the
# version step verifies it against Cargo.toml and each package job patches
# its version field from it. No `${{ github.event.* }}` interpolation in
# run blocks (injection-safe): tag/version flow through env vars.

name: release

on:
  push:
    tags:
      - 'v*'

permissions:
  contents: write

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  QMAKE: /usr/bin/qmake6
  QT_VERSION_MAJOR: "6"

jobs:
  linux-tarball:
    name: Linux x86_64 tarball
    runs-on: ubuntu-24.04
    container:
      image: archlinux:base-devel
    outputs:
      version: ${{ steps.version.outputs.version }}
    steps:
      - name: Install Arch dependencies
        run: |
          pacman -Sy --noconfirm --needed archlinux-keyring
          pacman -Syu --noconfirm --needed \
            git curl ca-certificates pkgconf \
            qt6-base qt6-declarative qt6-tools kirigami \
            clang ninja mesa file desktop-file-utils appstream

      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@1.95.0
        with:
          components: rustfmt, clippy

      - uses: Swatinem/rust-cache@v2

      - name: Resolve + verify version
        id: version
        env:
          TAG: ${{ github.ref_name }}
        run: |
          workspace_version="$(awk -F'"' '
              /^\[workspace\.package\]/ { in_section = 1; next }
              in_section && /^\[/ { exit }
              in_section && $1 ~ /^version[[:space:]]*=/ { print $2; exit }
          ' Cargo.toml)"
          tag_version="${TAG#v}"
          tag_release_version="${tag_version%%-*}"
          if [[ "$TAG" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+([.-].*)?$ ]] \
              && [[ "$tag_release_version" != "$workspace_version" ]]; then
            echo "tag ${TAG} (release ${tag_release_version}) != workspace ${workspace_version}" >&2
            exit 1
          fi
          printf 'version=%s\n' "$workspace_version" >> "$GITHUB_OUTPUT"
          echo "Building LinSight ${workspace_version} from tag ${TAG}"

      - name: Format + clippy sanity gate
        run: |
          cargo fmt --all -- --check
          cargo clippy --workspace --all-targets -- -D warnings

      - name: Validate desktop file + metainfo
        run: |
          desktop-file-validate packaging/io.visorcraft.LinSight.desktop
          appstreamcli validate --no-net packaging/io.visorcraft.LinSight.metainfo.xml

      - name: Build release binaries
        run: cargo build --workspace --release --locked

      - name: Offscreen GUI smoke
        env:
          QT_QPA_PLATFORM: offscreen
        run: |
          set +e
          timeout --preserve-status 5 target/release/linsight
          code=$?
          set -e
          if [[ "$code" -ne 124 && "$code" -ne 143 && "$code" -ne 0 ]]; then
            echo "GUI exited unexpectedly with ${code}" >&2
            exit "$code"
          fi

      - name: Stage portable tarball
        env:
          VERSION: ${{ steps.version.outputs.version }}
        run: |
          root="linsight-${VERSION}-linux-x86_64"
          staging="target/release-dist/${root}"
          dist="target/dist"
          rm -rf "$staging" "$dist"
          mkdir -p \
            "$staging/bin" \
            "$staging/share/applications" \
            "$staging/share/metainfo" \
            "$staging/lib/systemd/user" \
            "$dist"
          install -m755 target/release/linsight     "$staging/bin/linsight"
          install -m755 target/release/linsightd    "$staging/bin/linsightd"
          install -m755 target/release/linsight-cli "$staging/bin/linsight-cli"
          install -m644 packaging/io.visorcraft.LinSight.desktop \
            "$staging/share/applications/io.visorcraft.LinSight.desktop"
          install -m644 packaging/io.visorcraft.LinSight.metainfo.xml \
            "$staging/share/metainfo/io.visorcraft.LinSight.metainfo.xml"
          install -m644 packaging/systemd/linsight.service \
            "$staging/lib/systemd/user/linsight.service"
          install -m644 README.md "$staging/README.md"
          install -m644 LICENSE   "$staging/LICENSE"
          tar -C target/release-dist \
            --sort=name --mtime="@${SOURCE_DATE_EPOCH:-0}" \
            --owner=0 --group=0 --numeric-owner \
            -czf "${dist}/${root}.tar.gz" "$root"
          (cd "$dist" && sha256sum "${root}.tar.gz" >> sha256sums.txt)

      - name: Upload tarball artefact
        uses: actions/upload-artifact@v4
        with:
          name: linux-tarball
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7

  publish:
    name: Create GitHub release
    runs-on: ubuntu-latest
    needs: [linux-tarball]
    steps:
      - uses: actions/checkout@v4

      - name: Download all artefacts
        uses: actions/download-artifact@v4
        with:
          path: dist-staging

      - name: Aggregate artefacts + checksums
        run: |
          mkdir -p dist
          find dist-staging -type f ! -name "sha256sums.txt" -exec mv {} dist/ \;
          (cd dist && sha256sum * > sha256sums.txt)
          echo "Release payload:" && ls -la dist/

      - name: Create GitHub release
        env:
          GH_TOKEN: ${{ github.token }}
          TAG: ${{ github.ref_name }}
        run: |
          gh release create "$TAG" dist/* \
            --title "LinSight $TAG" \
            --generate-notes \
            --verify-tag
```

- [ ] **Step 3: Validate**

```bash
actionlint .github/workflows/release.yml
```

Expected: PASS. (Later tasks add jobs to `publish`'s `needs:` list.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add tag-triggered release (tarball + publish)"
```

---

## Task 7: `release.yml` — Arch + arch-v3 pacman packages

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add the `arch-pkg` and `arch-v3-pkg` jobs**

In `.github/workflows/release.yml`, add these two jobs under the `jobs:`
key (after `linux-tarball`, before `publish`):

```yaml
  arch-pkg:
    name: Arch pkg.tar.zst
    runs-on: ubuntu-24.04
    container:
      image: archlinux:base-devel
    needs: [linux-tarball]
    env:
      VERSION: ${{ needs.linux-tarball.outputs.version }}
    steps:
      - name: Install Arch build dependencies
        run: |
          pacman -Sy --noconfirm --needed archlinux-keyring
          pacman -Syu --noconfirm --needed \
            git sudo pkgconf \
            qt6-base qt6-declarative qt6-tools kirigami \
            clang ninja mesa rust cargo
      - uses: actions/checkout@v4
      - name: Create unprivileged build user
        run: |
          useradd -m -G wheel builder
          echo 'builder ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/builder
          chown -R builder:builder "$GITHUB_WORKSPACE"
      - name: Patch pkgver to the release version
        run: sed -i "s/^pkgver=.*/pkgver=${VERSION}/" packaging/arch/PKGBUILD
      - name: Build pacman package
        run: |
          runuser -u builder -- bash -lc '
            set -euo pipefail
            cd "$GITHUB_WORKSPACE/packaging/arch"
            makepkg -sf --noconfirm --syncdeps
          '
      - name: Stage artefact + sha256
        run: |
          mkdir -p target/dist
          mv packaging/arch/linsight-*.pkg.tar.zst target/dist/
          (cd target/dist && sha256sum linsight-*.pkg.tar.zst > sha256sums.txt)
      - uses: actions/upload-artifact@v4
        with:
          name: arch-pkg
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7

  arch-v3-pkg:
    name: Arch pkg.tar.zst (x86-64-v3)
    runs-on: ubuntu-24.04
    container:
      image: archlinux:base-devel
    needs: [linux-tarball]
    env:
      VERSION: ${{ needs.linux-tarball.outputs.version }}
    steps:
      - name: Install Arch build dependencies
        run: |
          pacman -Sy --noconfirm --needed archlinux-keyring
          pacman -Syu --noconfirm --needed \
            git sudo pkgconf \
            qt6-base qt6-declarative qt6-tools kirigami \
            clang ninja mesa rust cargo
      - uses: actions/checkout@v4
      - name: Create unprivileged build user
        run: |
          useradd -m -G wheel builder
          echo 'builder ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/builder
          chown -R builder:builder "$GITHUB_WORKSPACE"
      - name: Patch pkgver to the release version
        run: sed -i "s/^pkgver=.*/pkgver=${VERSION}/" packaging/arch-v3/PKGBUILD
      - name: Build pacman package
        run: |
          runuser -u builder -- bash -lc '
            set -euo pipefail
            cd "$GITHUB_WORKSPACE/packaging/arch-v3"
            makepkg -sf --noconfirm --syncdeps
          '
      - name: Stage artefact + sha256
        run: |
          mkdir -p target/dist
          mv packaging/arch-v3/linsight-*.pkg.tar.zst target/dist/
          (cd target/dist && sha256sum linsight-*.pkg.tar.zst > sha256sums.txt)
      - uses: actions/upload-artifact@v4
        with:
          name: arch-v3-pkg
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7
```

- [ ] **Step 2: Add both jobs to `publish`'s `needs:`**

Change the `publish` job's `needs:` line from:

```yaml
    needs: [linux-tarball]
```

to:

```yaml
    needs: [linux-tarball, arch-pkg, arch-v3-pkg]
```

- [ ] **Step 3: Validate**

```bash
actionlint .github/workflows/release.yml
```

Expected: PASS.

- [ ] **Step 4: (Best-effort) local Arch build sanity**

On an Arch/CachyOS host only — confirm the patched PKGBUILD parses and
the source URL resolves for the current tag pattern (does NOT need to
finish the build):

```bash
( cd packaging/arch && makepkg --printsrcinfo >/dev/null && echo "arch PKGBUILD parses ok" )
( cd packaging/arch-v3 && makepkg --printsrcinfo >/dev/null && echo "arch-v3 PKGBUILD parses ok" )
```

Expected: both "parses ok". (Full `makepkg` is exercised at the rc-tag
stage, since the source tarball is fetched from the pushed tag.)

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): add Arch + arch-v3 pacman package jobs"
```

---

## Task 8: `release.yml` — Debian `.deb`

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add the `deb-pkg` job**

Add this job under `jobs:` (after `arch-v3-pkg`, before `publish`):

```yaml
  deb-pkg:
    name: Debian .deb
    runs-on: ubuntu-24.04
    container:
      image: debian:trixie-slim
    needs: [linux-tarball]
    env:
      VERSION: ${{ needs.linux-tarball.outputs.version }}
    steps:
      - name: Install Debian build dependencies
        env:
          DEBIAN_FRONTEND: noninteractive
        run: |
          apt-get update
          apt-get install -y --no-install-recommends \
            ca-certificates git curl \
            build-essential debhelper devscripts dpkg-dev \
            qt6-base-dev qt6-declarative-dev qt6-tools-dev \
            kirigami2-dev libgl1-mesa-dev pkg-config clang ninja-build
      - name: Install Rust 1.95 via rustup
        env:
          RUSTUP_HOME: /usr/local/rustup
          CARGO_HOME: /usr/local/cargo
          HOME: /root
        run: |
          curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain 1.95 --profile minimal --no-modify-path
          {
            echo "RUSTUP_HOME=/usr/local/rustup"
            echo "CARGO_HOME=/usr/local/cargo"
          } >> "$GITHUB_ENV"
          echo "/usr/local/cargo/bin" >> "$GITHUB_PATH"
      - uses: actions/checkout@v4
      - name: Mark workspace safe for git
        run: git config --global --add safe.directory "$GITHUB_WORKSPACE"
      - name: Patch changelog version
        env:
          DEBEMAIL: "29009015+visorcraft@users.noreply.github.com"
          DEBFULLNAME: "VisorCraft"
        run: |
          ln -sfn packaging/debian debian
          if [ "$(dpkg-parsechangelog -S Version)" != "${VERSION}-1" ]; then
            dch -b -v "${VERSION}-1" --distribution unstable \
              "Release ${VERSION}."
          fi
      - name: Build .deb
        run: dpkg-buildpackage -d -us -uc -b
      - name: Stage Debian artefact + sha256
        run: |
          mkdir -p target/dist
          mv ../linsight_*.deb target/dist/
          (cd target/dist && sha256sum linsight_*.deb > sha256sums.txt)
      - uses: actions/upload-artifact@v4
        with:
          name: deb-pkg
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7
```

(`debian/rules`'s `override_dh_auto_test` re-runs `cargo test --workspace
--release --locked` during the build, so the `.deb` job also re-validates
the suite in release mode. `dch -b` patches the changelog version
in-container only — the committed file already has the `1.7.0-1` entry
from Task 1, so the guard skips when versions already match.)

- [ ] **Step 2: Add `deb-pkg` to `publish`'s `needs:`**

```yaml
    needs: [linux-tarball, arch-pkg, arch-v3-pkg, deb-pkg]
```

- [ ] **Step 3: Validate**

```bash
actionlint .github/workflows/release.yml
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): add Debian .deb job"
```

---

## Task 9: `release.yml` — Fedora + openSUSE `.rpm`

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add the `rpm-pkg` (Fedora) and `opensuse-pkg` jobs**

Add both jobs under `jobs:` (after `deb-pkg`, before `publish`):

```yaml
  rpm-pkg:
    name: Fedora .rpm
    runs-on: ubuntu-24.04
    container:
      image: fedora:44
    needs: [linux-tarball]
    env:
      VERSION: ${{ needs.linux-tarball.outputs.version }}
      # mold is mandatory here: under Fedora's --as-needed default,
      # ld.bfd drops -lQt6Quick and the link fails with an undefined
      # reference to QQuickWindow::grabWindow (see Containerfile.fedora44).
      RUSTFLAGS: "-Clink-arg=-fuse-ld=mold"
    steps:
      - name: Install Fedora build dependencies
        run: |
          dnf install -y --setopt=install_weak_deps=False \
            rpm-build rpmdevtools git tar gzip make \
            cargo rust clang mold pkgconf-pkg-config openssl-devel \
            qt6-qtbase-devel qt6-qtbase-private-devel \
            qt6-qtdeclarative-devel qt6-qtdeclarative-private-devel \
            qt6-qttools-devel kf6-kirigami-devel sqlite-devel \
            desktop-file-utils sudo
      - uses: actions/checkout@v4
      - name: Mark workspace safe for git
        run: git config --global --add safe.directory "$GITHUB_WORKSPACE"
      - name: Create unprivileged build user
        run: |
          useradd -m builder
          chown -R builder:builder "$GITHUB_WORKSPACE"
      - name: Patch spec version + build RPM
        run: |
          sed -i "s/^Version:.*/Version:        ${VERSION}/" packaging/fedora/linsight.spec
          runuser -u builder -- bash -lc '
            set -euo pipefail
            export RUSTFLAGS="-Clink-arg=-fuse-ld=mold"
            cd "$GITHUB_WORKSPACE/packaging/fedora"
            git archive --format=tar.gz \
              --prefix="linsight-'"${VERSION}"'/" \
              --output="linsight-'"${VERSION}"'.tar.gz" \
              -C "$GITHUB_WORKSPACE" HEAD
            rpmbuild \
              --define "_topdir $(pwd)/_rpmbuild" \
              --define "_sourcedir $(pwd)" \
              -bb linsight.spec
          '
      - name: Stage RPM artefact + sha256
        run: |
          mkdir -p target/dist
          find packaging/fedora/_rpmbuild/RPMS -name "linsight-*.rpm" -exec mv {} target/dist/ \;
          (cd target/dist && sha256sum linsight-*.rpm > sha256sums.txt)
      - uses: actions/upload-artifact@v4
        with:
          name: rpm-pkg
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7

  opensuse-pkg:
    name: openSUSE .rpm
    runs-on: ubuntu-24.04
    container:
      image: opensuse/tumbleweed
    needs: [linux-tarball]
    env:
      VERSION: ${{ needs.linux-tarball.outputs.version }}
      RUSTFLAGS: "-Clink-arg=-fuse-ld=mold"
    steps:
      - name: Install openSUSE build dependencies
        run: |
          zypper --non-interactive refresh
          zypper --non-interactive install --no-recommends \
            rpm-build git tar gzip make \
            rust cargo clang mold pkgconf-pkgconfig \
            'cmake(Qt6Core)' 'cmake(Qt6Quick)' 'cmake(KF6Kirigami)' \
            'pkgconfig(sqlite3)' sudo
      - uses: actions/checkout@v4
      - name: Mark workspace safe for git
        run: git config --global --add safe.directory "$GITHUB_WORKSPACE"
      - name: Create unprivileged build user
        run: |
          useradd -m builder
          chown -R builder:builder "$GITHUB_WORKSPACE"
      - name: Patch spec version + build RPM
        run: |
          sed -i "s/^Version:.*/Version:        ${VERSION}/" packaging/opensuse/linsight.spec
          runuser -u builder -- bash -lc '
            set -euo pipefail
            export RUSTFLAGS="-Clink-arg=-fuse-ld=mold"
            cd "$GITHUB_WORKSPACE/packaging/opensuse"
            git archive --format=tar.gz \
              --prefix="linsight-'"${VERSION}"'/" \
              --output="linsight-'"${VERSION}"'.tar.gz" \
              -C "$GITHUB_WORKSPACE" HEAD
            rpmbuild \
              --define "_topdir $(pwd)/_rpmbuild" \
              --define "_sourcedir $(pwd)" \
              -bb linsight.spec
          '
      - name: Stage RPM artefact + sha256
        run: |
          mkdir -p target/dist
          find packaging/opensuse/_rpmbuild/RPMS -name "linsight-*.rpm" -exec mv {} target/dist/ \;
          (cd target/dist && sha256sum linsight-*.rpm > sha256sums.txt)
      - uses: actions/upload-artifact@v4
        with:
          name: opensuse-pkg
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7
```

(Both RPMs link Qt private API, so each installs on the *build* distro's
Qt minor version — Fedora 44 / Tumbleweed respectively. The
`opensuse-pkg` artefact filenames collide with Fedora's `linsight-*.rpm`;
the `publish` aggregation keeps both because their dist names differ by
`%{dist}` tag (`.fc44` vs `.suse`/none). If they ever collide, rename in
the stage step.)

- [ ] **Step 2: Add both jobs to `publish`'s `needs:`**

```yaml
    needs: [linux-tarball, arch-pkg, arch-v3-pkg, deb-pkg, rpm-pkg, opensuse-pkg]
```

- [ ] **Step 3: Validate**

```bash
actionlint .github/workflows/release.yml
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): add Fedora + openSUSE .rpm jobs"
```

---

## Task 10: `release.yml` — AppImage + Flatpak (decoupled from publish)

These two are the fix-required/fragile formats. They are **not** added to
`publish`'s `needs:`, so a failure can't block the native packages — but
their artefacts are still picked up by `download-artifact` when they
succeed.

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add the `appimage` and `flatpak` jobs**

Add both jobs under `jobs:` (after `opensuse-pkg`, before `publish`):

```yaml
  appimage:
    name: AppImage
    runs-on: ubuntu-24.04
    container:
      image: debian:trixie-slim
    needs: [linux-tarball]
    steps:
      - name: Install build + appimage-builder dependencies
        env:
          DEBIAN_FRONTEND: noninteractive
        run: |
          apt-get update
          apt-get install -y --no-install-recommends \
            ca-certificates git curl file \
            qt6-base-dev qt6-declarative-dev qt6-tools-dev \
            kirigami2-dev libgl1-mesa-dev pkg-config clang ninja-build \
            python3-pip patchelf desktop-file-utils fakeroot zsync \
            squashfs-tools gnupg2 libglib2.0-bin
          pip3 install --break-system-packages appimage-builder
      - name: Install Rust 1.95 via rustup
        env:
          RUSTUP_HOME: /usr/local/rustup
          CARGO_HOME: /usr/local/cargo
          HOME: /root
        run: |
          curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain 1.95 --profile minimal --no-modify-path
          {
            echo "RUSTUP_HOME=/usr/local/rustup"
            echo "CARGO_HOME=/usr/local/cargo"
          } >> "$GITHUB_ENV"
          echo "/usr/local/cargo/bin" >> "$GITHUB_PATH"
      - uses: actions/checkout@v4
      - name: Mark workspace safe for git
        run: git config --global --add safe.directory "$GITHUB_WORKSPACE"
      - name: Build AppImage
        env:
          APPIMAGE_EXTRACT_AND_RUN: "1"
        run: bash scripts/build_appimage.sh
      - name: Stage AppImage + sha256
        run: |
          mkdir -p target/dist
          mv LinSight-*.AppImage target/dist/ 2>/dev/null || mv ./*.AppImage target/dist/
          (cd target/dist && sha256sum ./*.AppImage > sha256sums.txt)
      - uses: actions/upload-artifact@v4
        with:
          name: appimage
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7

  flatpak:
    name: Flatpak bundle
    runs-on: ubuntu-24.04
    needs: [linux-tarball]
    env:
      VERSION: ${{ needs.linux-tarball.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - name: Install flatpak + builder
        run: |
          sudo apt-get update
          sudo apt-get install -y --no-install-recommends flatpak flatpak-builder
      - name: Add flathub + install KDE 6.10 runtime/SDK
        run: |
          flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
          flatpak install -y --noninteractive flathub \
            org.kde.Platform//6.10 \
            org.kde.Sdk//6.10 \
            org.freedesktop.Sdk.Extension.rust-stable//24.08
      - uses: dtolnay/rust-toolchain@1.95.0
      - name: Vendor crates
        run: |
          cargo vendor --locked packaging/flatpak/vendor >/dev/null
          tar -C packaging/flatpak -cf packaging/flatpak/vendor.tar.gz vendor/
      - name: Build + bundle Flatpak
        run: |
          flatpak-builder --user --disable-rofiles-fuse --force-clean \
            --repo=flatpak-repo flatpak-build \
            packaging/flatpak/io.visorcraft.LinSight.yml
          flatpak build-bundle flatpak-repo \
            "linsight-${VERSION}.flatpak" io.visorcraft.LinSight master
      - name: Stage Flatpak + sha256
        env:
          VERSION: ${{ needs.linux-tarball.outputs.version }}
        run: |
          mkdir -p target/dist
          mv "linsight-${VERSION}.flatpak" target/dist/
          (cd target/dist && sha256sum linsight-*.flatpak > sha256sums.txt)
      - uses: actions/upload-artifact@v4
        with:
          name: flatpak
          path: target/dist/*
          if-no-files-found: error
          retention-days: 7
```

(The `rust-stable//24.08` extension version may need bumping to match the
runtime branch available on flathub at release time — confirm at the
rc-tag stage and adjust the pinned branch if `flatpak install` reports it
unavailable.)

- [ ] **Step 2: Confirm publish does NOT depend on these**

Verify the `publish` job's `needs:` still reads exactly:

```yaml
    needs: [linux-tarball, arch-pkg, arch-v3-pkg, deb-pkg, rpm-pkg, opensuse-pkg]
```

(`appimage` and `flatpak` are intentionally absent — `download-artifact`
with no `name:` pulls every artefact that uploaded, including these two
when they succeed.)

- [ ] **Step 3: Validate**

```bash
actionlint .github/workflows/release.yml
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): add AppImage + Flatpak jobs (non-blocking)"
```

---

## Task 11: End-to-end validation + open PR

GitHub Actions can only be fully exercised on GitHub. Validate CI via a
PR, security via manual dispatch, and the release via a throwaway
pre-release tag — then clean the throwaway tag/release up.

**Files:** none (operational).

- [ ] **Step 1: Final local sanity**

```bash
just ci
for f in .github/workflows/*.yml; do actionlint "$f" && echo "ok: $f"; done
```

Expected: `just ci` green; every workflow passes actionlint.

- [ ] **Step 2: Push the branch and open a PR**

```bash
git push -u origin ci/github-actions
gh pr create --fill --base master \
  --title "Add GitHub Actions CI + release pipeline" \
  --body "Adds lean per-push CI, weekly security checks, and a tag-triggered build-everything release. Syncs in-tree packaging versions to 1.7.0 and repairs the AppImage + Flatpak recipes. See docs/superpowers/specs/2026-06-01-github-actions-ci-release-design.md."
```

- [ ] **Step 3: Confirm `ci.yml` runs green on the PR**

```bash
gh pr checks --watch
```

Expected: the `fmt + clippy + test` check passes. **If it fails on a
Kirigami-related QML AOT error**, apply the spec's fallback: switch the
`ci.yml` `test` job to `container: archlinux:base-devel` with `kirigami`
installed (per §6 of the design doc), and push again.

- [ ] **Step 4: Manually exercise `security.yml`**

```bash
gh workflow run security.yml --ref ci/github-actions
gh run watch "$(gh run list --workflow=security.yml -L1 --json databaseId -q '.[0].databaseId')"
```

Expected: both `cargo-deny` and `cargo-audit` jobs complete (green, or a
genuine advisory finding to triage).

- [ ] **Step 5: Exercise the release with a throwaway pre-release tag**

```bash
git tag v1.7.0-rc-ci
git push origin v1.7.0-rc-ci
gh run watch "$(gh run list --workflow=release.yml -L1 --json databaseId -q '.[0].databaseId')"
```

Expected: `linux-tarball`, both Arch jobs, `deb-pkg`, `rpm-pkg`,
`opensuse-pkg`, and `publish` succeed; a draft/prerelease
`LinSight v1.7.0-rc-ci` appears with all native-package artefacts +
`sha256sums.txt`. Inspect AppImage/Flatpak job logs separately — if
either failed, note it for a follow-up; it must not have blocked
`publish`.

- [ ] **Step 6: Inspect, then tear down the throwaway release/tag**

```bash
gh release view v1.7.0-rc-ci    # eyeball the attached files
gh release delete v1.7.0-rc-ci --yes --cleanup-tag
```

Expected: every expected artefact was present; the throwaway release and
tag are removed. (The real release happens later by pushing a real
`v<version>` tag on `master` after merge.)

- [ ] **Step 7: Record outcomes on the PR**

If AppImage or Flatpak needed another iteration, add a PR comment (or a
tracked follow-up note) describing exactly what failed and the fix, so it
isn't silently dropped.

---

## Self-Review Notes

- **Spec coverage:** ci.yml (§2 → Task 2), security.yml (§3 → Task 3),
  release version-truth + tarball + publish (§4.0/4.1/4.9 → Task 6),
  Arch/arch-v3 (§4.2/4.3 → Task 7), deb (§4.4 → Task 8), Fedora/openSUSE
  rpm (§4.5/4.6 → Task 9), AppImage/Flatpak fixes + jobs (§4.7/4.8 →
  Tasks 5/4/10), in-tree version sync (§5 item 4 → Task 1 + AppImage in
  Task 5), validation plan (§6 → Task 11). All spec sections map to a
  task.
- **Decoupling invariant:** `publish.needs` must never include `appimage`
  or `flatpak` (Tasks 6→9 build it up to the six reliable jobs; Task 10
  Step 2 asserts it).
- **Version-name consistency:** the awk version-resolver is identical in
  `scripts/build_appimage.sh` and the release `version` step; every
  package job patches its field from `${VERSION}` =
  `needs.linux-tarball.outputs.version`.
