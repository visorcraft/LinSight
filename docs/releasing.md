<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Releasing

LinSight ships through three GitHub Actions workflows. They are deliberately
lean — one per-push CI job, one weekly security scan, and a release builder
that only runs on a tag — so routine pushes don't burn the Actions budget.

## Workflows

| Workflow | File | Trigger | What it does |
|---|---|---|---|
| CI | `.github/workflows/ci.yml` | push / PR to `master` (skips `**.md`, `docs/**`, `LICENSE`, screenshots) | `fmt --check` → `clippy -D warnings` → `cargo test --workspace`, a single `ubuntu-latest` job. Concurrency cancels superseded runs. |
| Security | `.github/workflows/security.yml` | weekly cron (Mon 06:00 UTC) + manual `workflow_dispatch` | `cargo deny` + `cargo audit`, read-only token. |
| Release | `.github/workflows/release.yml` | push of a `v*` tag | Builds all 8 distributable formats in parallel, then publishes a GitHub release with every artifact plus an aggregated `sha256sums.txt`. |

CI finds Qt via the `QMAKE=/usr/bin/qmake6` + `QT_VERSION_MAJOR=6` env declared
in the workflow — cxx-qt-build needs one of these to locate the Qt install, and
fails `%build`/`build.rs` with `Could not find Qt installation` without it.

## Release formats

The release job produces, checksums, and uploads:

| Format | Asset name | Built on |
|---|---|---|
| Portable tarball | `linsight-<ver>-linux-x86_64.tar.gz` | ubuntu-latest |
| Arch | `linsight-<ver>-1-x86_64.pkg.tar.zst` | archlinux container |
| Arch x86-64-v3 | `linsight-v3-<ver>-1-x86_64.pkg.tar.zst` | archlinux container |
| Debian | `linsight_<ver>-1_amd64.deb` | debian container |
| Fedora | `linsight-<ver>-1.fc44.x86_64.rpm` | fedora:44 container |
| openSUSE | `linsight-<ver>-0.x86_64.rpm` | opensuse/tumbleweed container |
| AppImage | `LinSight-<ver>-x86_64.AppImage` | ubuntu-latest |
| Flatpak | `linsight-<ver>.flatpak` | ubuntu-latest (flatpak-builder) |

## Cutting a release

1. Bump the version **everywhere**. `Cargo.toml`'s `[workspace] version` is the
   source of truth; keep these in lockstep with it:
   - `Cargo.toml` (workspace `version`) — then build once so `Cargo.lock` updates
   - `crates/linsight-plugin-sdk/Cargo.toml` (`linsight-core` dependency version)
   - `crates/linsight-plugin-sdk/README.md` (dependency example)
   - `packaging/fedora/linsight.spec` and `packaging/opensuse/linsight.spec`
     (`Version:` plus a new `%changelog` entry)
   - `packaging/com.visorcraft.LinSight.metainfo.xml` (new `<release>` entry)
   - `packaging/appimage/AppImageBuilder.yml` (`version:`)
   - `packaging/arch/PKGBUILD` and `packaging/arch-v3/PKGBUILD` (`pkgver`)
   - `packaging/arch/PKGBUILD.local` and `packaging/arch-v3/PKGBUILD.local`
     (`pkgver`)
   - `packaging/debian/changelog` (new top entry)
   - the latest-release line in `AGENTS.md`
2. Run `just ci` locally, then commit on `master`.
3. Tag and push:
   ```bash
   git tag -a v<ver> -m "v<ver>"
   git push origin master v<ver>
   ```
4. Watch the release run. When it is green it publishes
   `https://github.com/visorcraft/linsight/releases/tag/v<ver>` with all 8
   assets.

## openSUSE CDN flake (known, transient)

The openSUSE job occasionally fails at *Install openSUSE build dependencies*:

```
Timeout exceeded when accessing '…/repodata/…-appdata.xml.gz'.
Timeout reached Curl error (28)
```

This is an **upstream openSUSE CDN flake, not a config error** — the same
workflow builds openSUSE fine on most runs. The geoip edge `cdn.opensuse.org`
intermittently cannot serve the AppStream `appdata.xml.gz` for the Tumbleweed
OSS repo; the runner's curl times out and the in-job retry loop keeps hitting
the same sticky edge.

**Fix: re-run only the failed jobs on a fresh runner.**

```bash
gh run rerun <run-id> --failed
```

A fresh runner draws a fresh CDN edge (usually healthy) and rebuilds **only**
openSUSE + the publish step — the seven formats that already passed are not
rebuilt, so this costs one openSUSE build, not eight.

**Do not** try to "fix" this by:

- **Skipping appdata** — current libzypp (17.x) downloads `appdata.xml.gz`
  unconditionally; the empty `/usr/lib/zypp/plugins/appdata/` dir does not gate
  it, and there is no per-metadata-type exclusion flag.
- **Pinning a single mirror** — a direct mirror is usually a different
  Tumbleweed snapshot than the container image, which forces package downgrades
  and trips the `glib2-stage1-devel` / `this-is-only-for-build-envs` build
  guard. That trades a transient flake for a hard, non-transient conflict, even
  with `--force-resolution --allow-downgrade`.
- **Re-tagging** — re-pushing the tag reruns all eight builds and wastes the
  minutes the seven good ones already spent.
