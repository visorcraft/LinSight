<!--
SPDX-FileCopyrightText: 2026 VisorCraft LLC
SPDX-License-Identifier: GPL-3.0-only
-->

# GitHub Actions for LinSight — CI + Release Design

**Date:** 2026-06-01
**Status:** Approved design, pending implementation plan
**Author:** VisorCraft

## Goal

LinSight currently has **no** GitHub Actions. Add a minimal-but-complete
set, modelled on the sibling project LinSync
(`/work/repos/visorcraft/linsync/.github/workflows/`), that achieves two
things:

1. **CI:** every push / PR to `master` confirms the core test suite,
   formatting, and lints pass.
2. **Release:** pushing a version tag automatically builds *all* the
   distributable artifacts LinSight supports and attaches them to a
   GitHub Release.

## Constraints & guiding facts

- **The repo is public.** Standard Linux runners are therefore free and
  unlimited — the monthly-minutes budget is not a real constraint.
  Design goal shifts from "stingy" to "lean CI for fast feedback,
  comprehensive release."
- **Cost asymmetry.** CI runs on every push/PR (recurring); the release
  pipeline runs only on a version tag (a handful of times a month).
  Keep CI to a single lean job; the release pipeline can afford to build
  every format.
- **No mold needed for the default build.** LinSight's committed
  `.cargo/config.toml` is a bare `[build]` table — unlike LinSync it does
  *not* wire `-fuse-ld=mold`, so the default `ld` works for CI and most
  release jobs. **Exception:** the Fedora RPM build *does* require mold
  (see §4.5).
- **No Kirigami needed at compile time.** LinSync's CI compiles its
  Kirigami GUI on plain `ubuntu-latest` apt Qt with no Kirigami package
  installed; cxx-qt-build's `qmlcachegen` tolerates the missing import.
  LinSight mirrors this.
- **Rust 1.95** is pinned by `rust-toolchain.toml`. CI/release install
  the matching toolchain.
- **`linsight-cli` has no `man` / `completions` subcommands** (only
  `list`, `read`, `watch`, `alert`, `plugin`, `history`). The portable
  tarball therefore ships binaries + desktop/metainfo + systemd unit +
  docs — *not* manpages/completions (this is where it differs from
  LinSync's tarball).
- **No git dependencies** in `Cargo.lock` (`grep -c 'source = "git+'` →
  0). The Flatpak vendor-config fix can hardcode the crates.io source
  replacement.
- **Packaging version fields are stale and inconsistent.** This is the
  single most important release-correctness fact:

  | File | Hardcoded version |
  |---|---|
  | `packaging/arch/PKGBUILD` | `1.7.0` |
  | `packaging/arch-v3/PKGBUILD` | `1.7.0` |
  | `packaging/fedora/linsight.spec` | `1.6.0` |
  | `packaging/opensuse/linsight.spec` | `1.6.0` |
  | `packaging/debian/changelog` | `1.6.0-1` |

  The workspace `Cargo.toml` version is `1.7.0`. The release workflow
  **must treat the tag as the source of version truth** and patch every
  packaging version field from it at build time, so a release never
  depends on a human having bumped five separate files.

## Security posture

All `run:` blocks avoid interpolating `${{ github.event.* }}` /
`${{ github.ref_name }}` directly into shell — values are passed via
`env:` and referenced as `"$VAR"`. This follows LinSync's documented
hardening against workflow injection from PR titles / branch names /
tag names.

---

## 1. Overview — three workflow files

```
.github/workflows/
├── ci.yml         ← push/PR to master: one lean fmt+clippy+test job
├── security.yml   ← weekly schedule + manual: cargo-deny + cargo-audit
└── release.yml    ← tag push: build every artifact, publish GitHub Release
```

Every file carries the standard SPDX header:

```yaml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
```

---

## 2. `ci.yml` — the recurring gate

The recurring-cost workflow, kept to a **single job** that mirrors
`just ci` (fmt-check → clippy → test).

- **Triggers:**
  - `push` to `master`
  - `pull_request` to `master`
  - Both with `paths-ignore: ['**.md', 'docs/**', 'LICENSE',
    'packaging/screenshots/**']` so doc-only commits don't spin a
    runner.
- **Concurrency:** `group: ci-${{ github.ref }}`,
  `cancel-in-progress: true` — a new push cancels the stale run on the
  same ref.
- **Env:** `CARGO_TERM_COLOR=always`, `RUST_BACKTRACE=1`,
  `QMAKE=/usr/bin/qmake6`, `QT_VERSION_MAJOR=6`.
- **Job `test` (`ubuntu-latest`):**
  1. `actions/checkout@v4`
  2. Install Qt 6 build deps:
     `qt6-base-dev qt6-declarative-dev qt6-tools-dev libgl1-mesa-dev clang`
     (no mold, no Kirigami).
  3. `dtolnay/rust-toolchain@1.95.0` with `components: rustfmt, clippy`
     (matches `rust-toolchain.toml`).
  4. `Swatinem/rust-cache@v2`.
  5. `cargo fmt --all -- --check`
  6. `cargo clippy --workspace --all-targets -- -D warnings`
  7. `cargo test --workspace --locked`

  Sequential within one job → fail-fast, one checkout/toolchain/cache,
  fastest signal.

**Rationale for one job vs. LinSync's split fmt/lint/test:** LinSync
splits for parallel pinpointing; LinSight's chosen "lean test gate"
favours a single job. Since the repo is public (free minutes) this is a
readability/feedback choice, not a cost one — a single sequential job is
simplest and adequate.

---

## 3. `security.yml` — off the per-push path

Supply-chain checks that don't need to run on every push.

- **Triggers:** `schedule` (weekly, e.g. `cron: '0 6 * * 1'` — Monday
  06:00 UTC) + `workflow_dispatch` (manual button).
- **Job `deny` (`ubuntu-latest`):** `EmbarkStudios/cargo-deny-action@v2`
  (advisories + licenses + bans; reads `deny.toml`).
- **Job `audit` (`ubuntu-latest`):** install `cargo-audit --locked`, run
  `cargo audit`.
- Neither needs Qt — both read `Cargo.lock` / the advisory DB without
  compiling.

This is the "+ weekly security" choice: advisory coverage without
per-push cost.

---

## 4. `release.yml` — tag-triggered build-everything + publish

- **Trigger:** `push` tags matching `v*` (e.g. `v1.7.0`,
  `v1.8.0-rc1`).
- **Permissions:** `contents: write` (to create the Release).
- **Concurrency:** `group: release-${{ github.ref }}`,
  `cancel-in-progress: false` (never cancel a release mid-flight).

### 4.0 Version resolution (shared first step pattern)

Each build job resolves the version once, up front:

1. Read the workspace version from `Cargo.toml` `[workspace.package]`
   (awk, mirroring LinSync's release.yml).
2. Verify the pushed tag matches: strip a leading `v` and any
   `-rc1`/`-alpha` suffix; fail if the release portion ≠ workspace
   version.
3. Export `version` as a job output / step output.
4. **Patch the relevant packaging version field(s) from this version**
   before invoking the packager (see each job below).

This makes the tag the single source of version truth and neutralises
the stale hardcoded versions catalogued above.

### 4.1 `linux-tarball` — portable `.tar.gz`

- Runner: `archlinux:base-devel` container (fresh Qt 6 + Kirigami).
- Install: `git curl ca-certificates pkgconf qt6-base qt6-declarative
  qt6-tools kirigami clang ninja mesa file desktop-file-utils appstream
  github-cli`.
- Toolchain: `dtolnay/rust-toolchain@1.95.0` + `Swatinem/rust-cache@v2`.
- Resolve + verify version.
- Sanity gate: `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`. (Full
  `cargo test` is **not** repeated here — `ci.yml` already runs it on
  every push, and the `.deb` build re-tests in release mode. This job's
  purpose is to produce artifacts.)
- Validate `packaging/com.visorcraft.LinSight.desktop` +
  `com.visorcraft.LinSight.metainfo.xml`.
- `cargo build --workspace --release --locked`.
- Offscreen GUI smoke: `QT_QPA_PLATFORM=offscreen timeout 5
  target/release/linsight`; accept exit 0 / 124 / 143.
- Stage `linsight-<version>-linux-x86_64.tar.gz` containing:
  - `bin/linsight`, `bin/linsightd`, `bin/linsight-cli`
  - `share/applications/com.visorcraft.LinSight.desktop`
  - `share/metainfo/com.visorcraft.LinSight.metainfo.xml`
  - `lib/systemd/user/linsight.service`
  - `README.md`, `LICENSE`
  - (reproducible tar: `--sort=name --owner=0 --group=0
    --numeric-owner`)
- `sha256sum` the tarball; `actions/upload-artifact@v4` (name
  `linux-tarball`, `if-no-files-found: error`).

### 4.2 `arch-pkg` — Arch `.pkg.tar.zst`

- Runner: `archlinux:base-devel`, `needs: [linux-tarball]`.
- makepkg refuses root → create an unprivileged `builder` user with
  NOPASSWD sudo, `chown` the workspace.
- **Patch `packaging/arch/PKGBUILD` `pkgver` to the resolved version**
  (sed) so the GitHub archive source URL (`…/archive/v$pkgver.tar.gz`)
  resolves to the just-pushed tag.
- `runuser -u builder -- makepkg -sf --noconfirm --syncdeps` in
  `packaging/arch`.
- Stage `linsight-*.pkg.tar.zst` + sha256, upload (name `arch-pkg`).

### 4.3 `arch-v3-pkg` — Arch `.pkg.tar.zst` (x86-64-v3)

- Identical to 4.2 but in `packaging/arch-v3` (PKGBUILD adds
  `RUSTFLAGS=-C target-cpu=x86-64-v3`). Patch its `pkgver`. Upload name
  `arch-v3-pkg`.

### 4.4 `deb-pkg` — Debian/Ubuntu `.deb`

- Runner: `debian:trixie-slim`, `needs: [linux-tarball]` (trixie is the
  only Debian-family release whose apt ships
  `qml6-module-org-kde-kirigami`).
- Install build deps incl. `debhelper devscripts dpkg-dev qt6-base-dev
  qt6-declarative-dev qt6-tools-dev kirigami2-dev` (per
  `debian/control`).
- trixie's apt rustc is too old → install Rust 1.95 via rustup.
- **Patch `packaging/debian/changelog`** top entry to
  `linsight (<version>-1) …` (e.g. `dch` or sed) — `dpkg-buildpackage`
  names the `.deb` from the changelog, so without this it would ship
  `1.6.0` regardless of tag.
- Symlink `packaging/debian` → `debian/` at repo root, then
  `dpkg-buildpackage -d -us -uc -b`.
  - Note: `debian/rules`'s `override_dh_auto_test` runs
    `cargo test --workspace --release --locked` — the `.deb` build
    re-validates the suite in release mode (acceptable extra coverage).
- Stage `../linsight_*.deb` + sha256, upload (name `deb-pkg`).

### 4.5 `rpm-pkg` — Fedora `.rpm`

- Runner: **`fedora:44`** container (not `fedora:latest`) — the spec's
  AOT-compiled QML links Qt *private* symbols pinned to the exact Qt
  minor version, so the build distro's Qt must match what Fedora 44
  ships. Mirrors `packaging/fedora/Containerfile.fedora44`.
- Install (per the Containerfile): `rpm-build rpmdevtools git tar gzip
  make cargo rust clang mold pkgconf-pkg-config openssl-devel
  qt6-qtbase-devel qt6-qtbase-private-devel qt6-qtdeclarative-devel
  qt6-qtdeclarative-private-devel qt6-qttools-devel kf6-kirigami-devel
  sqlite-devel desktop-file-utils`.
- **mold is required here:** set `RUSTFLAGS=-Clink-arg=-fuse-ld=mold`
  (the Containerfile documents that ld.bfd under Fedora's `--as-needed`
  drops `-lQt6Quick` → `undefined reference QQuickWindow::grabWindow`).
- **Patch `packaging/fedora/linsight.spec` `Version:`** to the resolved
  version.
- Produce the source tarball via `git archive
  --prefix=linsight-<version>/` and `rpmbuild -bb` with `_topdir` /
  `_sourcedir` set, as a non-root `builder` user.
- Stage `linsight-*.rpm` + sha256, upload (name `rpm-pkg`).

### 4.6 `opensuse-pkg` — openSUSE `.rpm`

- Runner: `opensuse/tumbleweed` container, `needs: [linux-tarball]`.
- `zypper install` the spec's BuildRequires: `rust cargo
  cmake(Qt6Core) cmake(Qt6Quick) cmake(KF6Kirigami) pkgconfig(sqlite3)
  clang` (+ `rpm-build git tar gzip`).
- **Patch `packaging/opensuse/linsight.spec` `Version:`** to the
  resolved version; `git archive` the matching
  `linsight-<version>.tar.gz` into the rpmbuild sourcedir; `rpmbuild
  -bb`.
- Same Qt-private-symbol caveat as Fedora: tumbleweed's Qt is what the
  resulting RPM targets (document this; it installs on tumbleweed, not
  arbitrary openSUSE Leap releases).
- Stage + sha256, upload (name `opensuse-pkg`).

### 4.7 `appimage` — `.AppImage` (**requires packaging fix**)

The current `packaging/appimage/AppImageBuilder.yml` is non-functional:
`script: []`, hardcoded stale `version: 0.3.0`, and there is no
`just appimage` recipe wiring it. Fix as part of this work:

- **Recipe fix (`packaging/appimage/AppImageBuilder.yml`):**
  - Replace the hardcoded `version: 0.3.0` with a value injected at
    build time (the build script sed-substitutes the resolved version,
    or the recipe reads `$LINSIGHT_VERSION`).
  - Populate `script:` to build the workspace release and install
    `linsight` + desktop + metainfo + icon into `AppDir/usr/...`.
  - The recipe bundles Qt + Kirigami from Debian trixie apt
    (`apt.sources` already points at trixie) — so this job runs in a
    `debian:trixie-slim` (or appimage-builder) container, not Arch.
  - Disable the recipe's `test:` block in CI (it needs docker-in-docker
    + host X) by invoking `appimage-builder --skip-test`.
- **New `just appimage` recipe** wrapping the above for local parity.
- **Job (`debian:trixie-slim`):** because the AppImage bundles Qt +
  Kirigami from Debian trixie apt, the `linsight` binary it wraps must
  be compiled in that *same* environment (an Arch-built binary would
  mismatch the bundled libs). So the job installs the full build
  toolchain — rustup 1.95 + `qt6-base-dev qt6-declarative-dev
  qt6-tools-dev kirigami2-dev clang` — **and** `appimage-builder` (pip)
  with its runtime deps (`patchelf`, `desktop-file-utils`, `fakeroot`,
  `zsync`, etc.). It builds the release binary, then runs
  `appimage-builder --recipe … --skip-test`, and uploads the produced
  `*.AppImage` (+ `.zsync` if generated) as artifact `appimage`.
- **Resilience:** because appimage-builder is comparatively fragile,
  the `publish` job does **not** hard-depend on this job (see §4.9) —
  a failure here does not block publishing the native packages.

### 4.8 `flatpak` — `.flatpak` bundle (**requires packaging fix**)

The Flatpak module runs `cargo --offline build --locked` against a
`vendor/` dir, but nothing tells cargo to use it: the repo's
`.cargo/config.toml` is bare `[build]`, and `just flatpak-vendor` never
writes a vendor source-replacement config. Fix:

- **Manifest fix (`packaging/flatpak/com.visorcraft.LinSight.yml`):** add
  a build-command, before the `cargo --offline build`, that writes a
  cargo config into `$CARGO_HOME` (already set to
  `/run/build/linsight/cargo`) redirecting crates.io to the vendored
  sources:

  ```yaml
  build-commands:
    - mkdir -p "${CARGO_HOME}"
    - |
      cat > "${CARGO_HOME}/config.toml" <<'EOF'
      [source.crates-io]
      replace-with = "vendored-sources"
      [source.vendored-sources]
      directory = "vendor"
      EOF
    - cargo --offline build --workspace --release --locked
    # …existing install lines…
  ```

  This is safe to hardcode because `Cargo.lock` has **no git sources**
  (only crates.io). `directory = "vendor"` matches the manifest's
  `dest: vendor` extraction of `vendor.tar.gz`.
- **Job (`ubuntu-latest`):**
  1. `actions/checkout@v4`.
  2. Install `flatpak flatpak-builder`; add the flathub remote.
  3. `flatpak install -y flathub org.kde.Platform//6.10
     org.kde.Sdk//6.10 org.freedesktop.Sdk.Extension.rust-stable`
     (the KDE 6.10 runtime *is* on flathub — no special base image
     needed, which is why LinSync's "no KDE 6 image" blocker does not
     apply here).
  4. `just flatpak-vendor` (produces `packaging/flatpak/vendor.tar.gz`).
  5. `flatpak-builder --repo=…/repo --force-clean build
     packaging/flatpak/com.visorcraft.LinSight.yml`.
  6. `flatpak build-bundle …/repo linsight-<version>.flatpak
     com.visorcraft.LinSight master`.
  7. Upload artifact `flatpak`.
- **Resilience:** like AppImage, `publish` does not hard-depend on this
  job.

### 4.9 `publish` — aggregate + GitHub Release

- Runner: `ubuntu-latest`.
- `needs:` the **reliable** jobs only — `[linux-tarball, arch-pkg,
  arch-v3-pkg, deb-pkg, rpm-pkg, opensuse-pkg]`. The two
  fix-required/fragile jobs (`appimage`, `flatpak`) are *not* in
  `needs`, so a flaky AppImage/Flatpak build never blocks shipping the
  native packages — their artifacts are still picked up if they
  succeeded.
- `actions/download-artifact@v4` (all artifacts) → flatten into `dist/`,
  skipping per-job `sha256sums.txt`.
- Produce one aggregate `dist/sha256sums.txt`.
- `gh release create "$TAG" dist/* --title "LinSight $TAG"
  --generate-notes --verify-tag` (`GH_TOKEN=${{ github.token }}`, `TAG`
  via env).

---

## 5. Packaging changes required (summary)

Beyond the three workflow files, this work touches packaging so the
"build everything" release is real, not aspirational:

1. **`packaging/appimage/AppImageBuilder.yml`** — dynamic version +
   populated `script:` (see §4.7).
2. **`Justfile`** — new `appimage` recipe (local parity with §4.7).
3. **`packaging/flatpak/com.visorcraft.LinSight.yml`** — vendor
   source-replacement config write before the offline build (see §4.8).
4. **Sync all in-tree packaging version fields to the workspace
   version (`1.7.0`).** The workflow patches them from the tag at build
   time regardless, but the committed files currently disagree
   (Arch/arch-v3 = `1.7.0`; Fedora/openSUSE = `1.6.0`; Debian changelog
   = `1.6.0-1`; AppImage = `0.3.0`), which breaks *local* package builds
   and is just confusing. Bring them all to `1.7.0`:
   - `packaging/fedora/linsight.spec` — `Version: 1.6.0` → `1.7.0`.
   - `packaging/opensuse/linsight.spec` — `Version: 1.6.0` → `1.7.0`.
   - `packaging/debian/changelog` — add a new top entry
     `linsight (1.7.0-1) unstable; urgency=medium` (don't rewrite
     history; prepend per Debian convention).
   - `packaging/appimage/AppImageBuilder.yml` — the stale `0.3.0` is
     replaced by the dynamic-version fix in §4.7 / item 1 above, so it
     lands on `1.7.0` too.
   - `packaging/arch/PKGBUILD`, `packaging/arch-v3/PKGBUILD` — already
     `1.7.0`; verify, no change expected.
   - Sanity check: every field should equal the `Cargo.toml`
     `[workspace.package]` `version` (`1.7.0`).

---

## 6. Risks & validation

- **CI compiles the GUI without Kirigami.** Proven by LinSync; the one
  thing to confirm on the first CI run. If `qmlcachegen` *does* hard-fail
  without Kirigami, fall back to running the `test` job in an
  `archlinux:base-devel` container with `kirigami` installed (slower,
  still fine).
- **appimage-builder reliability** is the least certain piece;
  `publish` is decoupled from it so a failure is non-blocking, and the
  recipe runs with `--skip-test`.
- **Flatpak runtime download** (~1–2 GB KDE 6.10) makes it the slowest
  job; acceptable (release-only, free minutes) and non-blocking.
- **Qt-private-symbol pinning** for Fedora/openSUSE RPMs means those
  packages install on the *build* distro's Qt minor version
  (Fedora 44 / Tumbleweed), documented in the spec headers — not a CI
  failure, a distribution caveat.
- **Tag/version mismatch** fails the release fast in the version step
  (intended).

### Validation plan

- `just ci` green locally (current baseline: 381 tests pass) before
  wiring.
- After landing: open a throwaway PR to confirm `ci.yml` runs and is
  green; manually `workflow_dispatch` `security.yml`.
- Cut a pre-release tag (e.g. `v1.7.0-rc-ci`) to exercise `release.yml`
  end-to-end and inspect every uploaded artifact before trusting a real
  tag.

---

## 7. Out of scope

- GUI screenshot / a11y-grep jobs (LinSync has these; not requested —
  "no frivolous tests").
- Splitting CI into parallel fmt/lint/test jobs (chosen single lean
  job).
- macOS / Windows / non-x86_64 targets (LinSight is Linux x86_64).
- Caching across the per-distro release containers (releases are
  infrequent; not worth the complexity).
