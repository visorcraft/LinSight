<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# AGENTS.md

## Agent Working Style

- Be concise. No long explanations. Don't restate the plan unless it changed.
- Never scan `node_modules`, `.venv`, `dist`, `build`, log/archive dirs, or generated files.
- Cap output when searching or reading files. Default to limits, e.g.:
  - `head -n 100` / `tail -n 100`
  - `grep -n "pattern" file | head`
  - `find . -type f | head -n 200`
  - `python script.py --limit 50`

Guidelines for AI assistants working on this repository.

LinSight is a Linux system-monitoring dashboard with multi-GPU
support, a runtime `.so` plugin system, and an mTLS remote tunnel.
The latest release is **v1.16.0**; the workspace `version` in
`Cargo.toml` is the single source of truth — check it rather than
trusting this line. `CHANGELOG.md` has the full per-release history
(0.3.0 → 1.15.0): all 10 spec phases, an audit-driven hardening
sprint, themes + custom dashboards, the Hardware page + per-device
nicknames, the Alerts UI, i915/zram/systemd sensors, per-plugin
config (ABI v5), transport hardening, the v1.2–1.4 theming work,
Storage-page disk nesting (v1.6), a second security-hardening
sprint with plugin panic isolation (ABI v6, v1.7), GitHub Actions
CI + multi-format release automation (v1.7.1), the third-party
notices rename + regeneration (v1.7.2), the
`com.visorcraft.LinSight` app-id rename (v1.7.3), the new
application icon + corrected third-party credits (v1.8.0), the
v1.9.0 hardening and performance release, the v1.10.0 feature release
(process explorer, SMART disk health, GUI history charts, history
retention, alert event log/cooldown, and sensor snapshot caches),
the v1.13.0 multi-host view with saved SSH hosts and sidebar
switching, the v1.14.0 Network page and persisted process-table
state, and the v1.15.0 benchmarks / proptest / GUI-CI / sandbox
design release.

- User-facing changelog: `CHANGELOG.md`

## Build commands

Everything goes through `just`. Direct `cargo` invocations are
equivalent if `just` isn't installed.

```bash
just ci             # fmt-check + clippy + tests — same gate CI runs
just preflight      # ci + cargo deny + cargo audit
just build          # cargo build --workspace (debug)
just build-release  # release: lto=fat, codegen-units=1, strip
just build-release-v3   # x86_64-v3 tuned release (CachyOS-friendly)
just test           # cargo test --workspace
just lint           # cargo clippy --workspace --all-targets -- -D warnings
just fmt            # cargo fmt --all
just deny           # cargo deny --all-features check
just audit          # cargo audit
just credits        # cargo about generate → docs/third-party-notices.md
just gui-smoke      # xvfb headless GUI handshake smoke (shell, not cargo)
just run-daemon ARGS    # cargo run -p linsightd -- ARGS
just run-cli ARGS       # cargo run -p linsight-cli -- ARGS
```

**CI parity:** `just ci` runs `fmt --check`, `clippy -D warnings`, and
`cargo test --workspace` in that order. A red local `just ci` is a red
CI run.

**Toolchain pinning:** `rust-toolchain.toml` pins stable Rust 1.95.
Do not bump without updating this file's prerequisites table.

**Build acceleration (optional, opt-in).** The committed
`.cargo/config.toml` is intentionally minimal (`[build]` only) — it
does **not** wire `mold` or `sccache`, so a clean checkout builds
with the default toolchain and needs no extra tools. If you want the
`mold` linker + `sccache` rustc/C++ wrapper locally, install them and
opt in via your own config (e.g. `~/.cargo/config.toml` or a
`.cargo/config.local.toml`); don't bake machine-specific wiring into
the committed config.

```bash
pacman -S mold sccache   # Arch / CachyOS, if you want them
```

**Watch the test baseline.** `cargo test --workspace` passes **524**
(0 failed, 3 ignored) at the time of writing. Run it to get the
current number rather than trusting this one; a drop without a
corresponding test deletion in the diff means a regression.

The workspace now includes Criterion benchmarks (`just bench`) and
proptest round-trip tests. Release-mode tests (`just test-release`)
are also available and are the only way the v0.3.0 stabby
`match_owned` opt-level bug reproduced.

### Building & launching the GUI

The desktop app's **cargo package is `linsight`** — it lives in
`apps/linsight-gui/`, but the package name is *not* `linsight-gui`
(`cargo build -p linsight-gui` fails). Rebuild it and find the binary:

```bash
cargo build -p linsight --release   # → target/release/linsight
cargo build -p linsight             # → target/debug/linsight (faster iteration)
```

Touching a shared crate the GUI depends on (e.g. `linsight-core`) and
rebuilding `-p linsight` is enough — the dependency is recompiled.
`just build-release` does the whole workspace.

**Launch on your desktop** from a shell that carries the graphical
session env (`WAYLAND_DISPLAY`/`DISPLAY` + `XDG_RUNTIME_DIR`):

```bash
LINSIGHT_LOG=info ./target/release/linsight [page]
```

- `[page]` is an optional positional that opens straight to one of
  `overview gpus storage network hardware processes editor settings
  about licenses credits` — land directly on the screen you're verifying
  (e.g. `linsight gpus`). Anything else is ignored.
- **Don't start the daemon yourself.** The GUI auto-spawns a
  `linsightd` child when nothing is listening on
  `$XDG_RUNTIME_DIR/linsight.sock`, and drops it on exit. A healthy
  cold start logs `NVML initialized` → `hardware registry built` →
  `sensor catalogue cached count=N`; that last line is the
  daemon-handshake-complete marker.
- Attach to a remote daemon instead: `--connect ssh://[user@]host[:port]`.

**To verify what it rendered, use the in-app screenshot path, never
`spectacle`/`grim`** (an external grab of an unfocused LinSight window
returns a frozen frame — see *GUI conventions* below for the full
rationale):

```bash
./scripts/dev_screenshot.sh <page> /tmp/shot.png
```

A headless boot smoke is `just gui-smoke` (runs under `xvfb-run` with
`LIBGL_ALWAYS_SOFTWARE=1`).

## Local install

This dev machine runs LinSight in one of two ways: directly from the workspace
release binaries, or from a locally built AppImage. The AppImage builder
requires Ubuntu/`apt-get`, so local AppImage builds on this Arch/CachyOS host
run inside an Ubuntu container using
`target/appimage/build_appimage_in_container.sh` (podman/docker with the
`ubuntu:24.04` image). Bind-mount a fresh `target-appimage` directory over
`/workspace/target` to avoid the cxx-qt-build symlink error; the recipe will
rebuild the workspace inside Ubuntu and produce
`LinSight-<version>-x86_64.AppImage`.

### Local binary install

Current symlinks for the binary install:

```
~/.local/bin/linsight  -> /work/repos/visorcraft/linsight/target/release/linsight
~/.local/bin/linsightd -> /work/repos/visorcraft/linsight/target/release/linsightd
~/.local/bin/linsight-cli -> /work/repos/visorcraft/linsight/target/release/linsight-cli
```

After pulling/rebuilding, recreate the symlinks:

```bash
cargo build --workspace --release
ln -sf /work/repos/visorcraft/linsight/target/release/linsight  ~/.local/bin/linsight
ln -sf /work/repos/visorcraft/linsight/target/release/linsightd ~/.local/bin/linsightd
ln -sf /work/repos/visorcraft/linsight/target/release/linsight-cli ~/.local/bin/linsight-cli
```

### AppImage install

The AppImage lives in `~/Applications/` with a generic symlink:

```
~/Applications/LinSight-<version>-x86_64.AppImage
~/Applications/LinSight.AppImage -> LinSight-<version>-x86_64.AppImage
```

The user-level desktop entry at
`~/.local/share/applications/com.visorcraft.LinSight.desktop` uses
`Exec=/home/thomasw/.local/bin/linsight`, so launcher/taskbar pins keep
working as long as that path resolves to the AppImage:

```bash
ln -sf ~/Applications/LinSight.AppImage ~/.local/bin/linsight
```

To update the AppImage after a rebuild, replace the versioned file, refresh the
generic symlink, and keep the `~/.local/bin/linsight` symlink pointed at it:

```bash
mv LinSight-<version>-x86_64.AppImage ~/Applications/
ln -sf LinSight-<version>-x86_64.AppImage ~/Applications/LinSight.AppImage
ln -sf ~/Applications/LinSight.AppImage ~/.local/bin/linsight
```

`linsightd` and `linsight-cli` are not shipped in the AppImage, so their
`~/.local/bin` symlinks should continue to point at `target/release/*`.

### Taskbar pin maintenance

The desktop entry and the `~/.local/bin/linsight` symlink are the only things
KDE's taskbar pin cares about. As long as the desktop entry's `Exec=` path is
valid, the pin will launch whichever binary or AppImage that path points to.
No KDE service-cache refresh is needed when only the symlink target changes.

If a taskbar pin stops responding after changing the desktop entry itself,
rebuild the KDE service cache and restart Plasma:

```bash
update-desktop-database ~/.local/share/applications
kbuildsycoca6 --noincremental
killall plasmashell
kstart6 plasmashell
```

### Removing the old pacman package

If this host previously had `linsight` or `linsight-v3` installed via pacman,
remove it before switching to the local-binary install:

```bash
sudo pacman -R linsight-v3   # or `sudo pacman -R linsight`
```

Also clear any stale system binaries or user-level desktop entry that shadows
the local one:

```bash
sudo rm -f /usr/bin/linsight /usr/bin/linsightd /usr/bin/linsight-cli
rm -f ~/.local/share/applications/com.visorcraft.LinSight.desktop
rm -f ~/.local/share/icons/hicolor/scalable/apps/com.visorcraft.LinSight.svg
```

## Continuous integration & releases

Three lean GitHub Actions workflows — kept minimal on purpose so routine
pushes don't burn the Actions budget. Full runbook: `docs/releasing.md`.

- **`ci.yml`** — push/PR to `master` (path-ignored for docs/markdown).
  Single `ubuntu-latest` job: `fmt --check` → `clippy -D warnings` →
  `cargo test --workspace`. Concurrency-cancels superseded runs.
- **`security.yml`** — weekly cron (Mon 06:00 UTC) + manual dispatch:
  `cargo deny` + `cargo audit`.
- **`release.yml`** — fires on a `v*` tag. Builds all 8 formats (tarball,
  Arch, Arch v3, deb, Fedora rpm, openSUSE rpm, AppImage, Flatpak) in
  parallel, then publishes a GitHub release with every artifact + an
  aggregated `sha256sums.txt`.

To cut a release, bump the version everywhere (the per-file list is in
`docs/releasing.md` — `Cargo.toml`'s workspace `version` is the source of
truth), then `git tag -a vX.Y.Z && git push origin master vX.Y.Z`.

**Known transient: the openSUSE job sometimes fails at dep-install** with a
`Curl error (28)` timeout fetching `appdata.xml.gz` from `cdn.opensuse.org`.
This is an upstream CDN flake, not a config bug. **Fix: `gh run rerun
<run-id> --failed`** — a fresh runner draws a fresh CDN edge and rebuilds
only openSUSE + publish (not the seven formats that already passed). Do
*not* pin a single mirror (snapshot skew → hard dependency conflicts) or
re-tag (reruns all eight builds). See `docs/releasing.md` for why.

**Known transient: a format job can fail at "Initialize containers"** with
`docker pull … registry-1.docker.io … context deadline exceeded` — Docker Hub
intermittently times out pulling a job's base image (`archlinux:base-devel`,
`fedora:44`, …) through all 3 retries. Same upstream-flake class as the
openSUSE CDN one, same fix: **`gh run rerun <run-id> --failed`** reruns only the
affected job(s) + publish, reusing the formats that already passed. (The v1.8.0
release hit this twice — tarball, then Fedora — and cleared on rerun each time.)

## Workspace layout

```
linsight/
├── apps/
│   ├── linsightd/           ← daemon: plugin host, postcard Unix socket,
│   │                          optional SQLite history + evalexpr alerts
│   │                          + Prometheus exporter (all env-gated)
│   ├── linsight-gui/        ← Qt 6 / Kirigami GUI via cxx-qt 0.8
│   └── linsight-tunnel/     ← mTLS bridge (rustls + ring) for non-SSH remote
├── crates/
│   ├── linsight-core/       ← shared types + dashboard model (no I/O)
│   ├── linsight-protocol/   ← postcard wire types + framing
│   ├── linsight-plugin-sdk/ ← public LinsightPlugin trait + export_plugin!
│   │                          macro. ABI v6 via R-mirror types in
│   │                          `mirror` module (kind+payload structs —
│   │                          see ADR-0001 for the stabby release-mode
│   │                          bug that drove the v2→v3 refactor).
│   │                          v5 added `config_json` on RPluginCtx for
│   │                          per-plugin TOML config; v6 switched the
│   │                          trait methods to `extern "C-unwind"` so a
│   │                          plugin panic is caught by the daemon
│   │                          (needs the release `panic = "unwind"`).
│   │                          The factory symbol is renamed each ABI
│   │                          bump (…v4 → _v5 → _v6); mismatched plugins
│   │                          fail at symbol lookup, not first sample.
│   │                          LINSIGHT_PLUGIN_ABI_VERSION = 6.
│   ├── linsight-sensors/    ← built-in sensors (in-tree plugins):
│   │   ├── cpu/ mem/ net/ nvme/ nvml/ smart/ xe/ amdgpu/ i915/ disk/
│   │   ├── fs/ hwmon/ proc/ system/ systemd/ zram/ containers/ sock/
│   └── linsight-cli/        ← list / read / plugin {new,install,ls,remove}
├── examples/
│   └── echo-plugin/         ← minimal third-party plugin (`cdylib`).
│                              Exercised by the SDK's dynamic-load test
│                              and serves as a worked example.
```

The daemon's optional surfaces are off by default. Opt in with
`LINSIGHT_HISTORY=1`, `LINSIGHT_ALERTS=1`, `LINSIGHT_PROM_BIND=...`;
the systemd user unit at `packaging/systemd/linsight.service`
enables them for always-on mode. The Settings page now reflects
each subsystem's actual on/off state via the GUI's
`OverviewModel.envIsSet(name)` invokable (caveat: it reads the GUI
process's env, not the daemon's — fine for the systemd-unit
deployment).

## Code conventions

- **Resolver 3, edition 2024, latest stable Rust 1.95.** Do not require nightly.
- **No async runtime in the daemon hot path.** Sync + `polling` only.
  (`linsight-tunnel` uses tokio — it's a separate binary, not the
  daemon: an mTLS bridge for the daemon socket, defaulting to
  `127.0.0.1:9443`, with a `--max-connections` cap and graceful
  shutdown on Ctrl+C/SIGTERM.) Async deps (`tokio`, `rustls`, `tokio-rustls`,
  `rustls-pki-types`) live in
  `[workspace.dependencies]` so a second async crate would inherit
  the same versions.
- **SPDX REUSE headers required.** Every new source file gets:
  ```
  // SPDX-FileCopyrightText: 2026 VisorCraft LLC
  // SPDX-License-Identifier: GPL-3.0-only
  ```
- **GPL-3.0-only.** New deps must use a license in `deny.toml`'s
  allowlist. Our own workspace crates are `publish = false`, so
  `about.toml`'s `private.ignore` drops them from the third-party
  credits supplement (`docs/third-party-notices.md`) automatically —
  keep new internal crates `publish = false` and regenerate with
  `just credits`.
- **The app icon is a generated asset.** Source of truth is
  `assets/LinSight.svg` (the GitHub social banner is
  `assets/social-card.svg`); never hand-edit the derived PNGs. Re-run
  `./scripts/dev_render_icons.sh` (needs `rsvg-convert` + `magick`) to
  regenerate every size from the master: the
  `packaging/icons/<size>x<size>/apps/com.visorcraft.LinSight.png`
  hicolor tree + scalable SVG, the GUI's
  `apps/linsight-gui/resources/linsight-*.png` (compiled into the binary
  via the Qt resource bundle in `build.rs`), and `assets/LinSight.{png,ico}`
  + `assets/social-1024x512.png`. `assets/README.md` documents the set.
- **Default to no comments.** Only add one when the WHY is non-obvious.
- **`tracing::*!` for structured logs.** Default filter is `info`;
  override via `LINSIGHT_LOG`.
- **Conventional Commits** for messages: `feat:`, `fix:`, `chore:`,
  `docs:`, `refactor:`, `test:`, `perf:`.
- **Never add an AI assistant as a commit contributor.** Do not add
  `Co-Authored-By: Claude`, `Co-Authored-By: Cursor`, `Co-Authored-By: cursoragent`, `Co-Authored-By: Codex`, or any other
  AI-attribution trailer (`Generated with`, `Assisted-by`, etc.) to
  commit messages. Commits are authored by the human committer only.

## Plugin SDK conventions

- **FFI validation at the boundary, not in the type system.**
  `From<RSensorId> for SensorId` calls the infallible
  `SensorId::new` (debug-asserted invariants); `host_init` walks the
  raw stabby strings and runs `SensorId::try_new` on each ID
  *before* the From-conversion, so a release-mode plugin that emits
  whitespace-bearing IDs is rejected instead of poisoning the
  registry. Apply the same pattern for any future R-type that wraps
  a validated host type.
- **`PluginCtx::sysroot` is UTF-8 only.** Build via
  `PluginCtx::new_with_sysroot(PathBuf)` — non-UTF-8 paths are
  rejected at construction so the FFI conversion is infallible. The
  field is private; reach it via `ctx.sysroot()` which returns
  `Option<&Path>`.
- **End-to-end example: `examples/echo-plugin/`.** Built as a real
  `cdylib`; the SDK's `tests/dynamic_load.rs` dlopens it and
  exercises the full reflection-checked load path. Use this as the
  reference for `linsight-cli plugin new`'s output shape.

## GUI conventions (`apps/linsight-gui/qml/`)

- **One `OverviewModel`, declared only in `Main.qml`.** Every page
  receives it as `property QtObject dashModel: null` (typed, not
  `var`) and reads `dashModel.tilesJson` / `dashModel.cpuText` /
  `dashModel.connected` / etc. Per-page instantiation looks
  innocent but silently breaks every page after the first:
  `Client::take_sample_rx()` is one-shot, so the second model gets
  `None` and stalls on "…". Root-caused in commit `1f4e4a8`.
- **For visual verification, use the in-app screenshot path**, not
  `spectacle` / `grim`:
  ```
  ./scripts/dev_screenshot.sh <page> /tmp/shot.png
  # wraps: linsight --screenshot <path> --reduce-motion
  ```
  It calls `QQuickWindow::grabWindow()` to render the QML scene
  independent of compositor focus. Wayland's stale-surface cache
  for unfocused windows will hand back a frozen frame to external
  tools and trick you into chasing imaginary binding bugs.
- **cxx-qt 0.8.** Setter calls through `qt_thread.queue` *do*
  re-trigger QML bindings; if a value looks stale, screenshot via
  the in-app path before suspecting cxx-qt.
- **Discriminated banner feedback.** `showSuccess(msg)` and
  `showError(msg)` in `CanvasEditorPage.qml` replace the previous
  string-prefix sniff. `isLayoutError(s)` centralizes the
  `error:`-prefix check for Rust-returned status strings. Don't
  re-introduce inferred error/success state from message contents.
- **CLI flags pass through clap.** `--reduce-motion`,
  `--no-animations`, `--screenshot`, `--screenshot-delay`,
  `--connect`, and the optional initial page are all declared in
  `apps/linsight-gui/src/main.rs`'s `Cli` struct. QML reads them
  from `Qt.application.arguments`; adding a new flag means updating
  both surfaces.
- **The GUI auto-spawns the daemon.** `Client::connect_or_spawn`
  starts `linsightd` as a child process if no socket is listening
  (held alive by the client, dropped on `Client::drop`); remote
  sessions attach over an `ssh -N -L` tunnel instead.
- **DesignTokens are the source of truth for spacing/color/radii.**
  `pageHeaderHeight`, `markPanelDeep/Top/Bar`, `accentMute`,
  spacing scale, etc. Bypassing them produces the "introduced the
  system and then immediately ignored it" pattern flagged in the
  c28089a peer review — don't.
- **`just i18n-extract` scans an explicit QML file list, not a glob.**
  A new `qsTr(...)` in a file missing from the `lupdate6` list in
  `Justfile` silently renders English in every locale. When you add a
  QML file with translatable strings, append it to *both* the
  `lupdate6` arg list and the `lrelease6` invocation in `i18n-compile`.

### Adding a new QML page or QObject (the five-trap saga)

The cxx-qt + cxx-qt-build pipeline has five traps that don't fail at
`cargo build` time. A whole-file release rebuild and a fresh AppImage
will succeed, the launcher will start a process, and *no window will
appear* — Qt logs the actual error to the systemd journal, not stderr,
so you have to know where to look.
Each of these bit us in a single commit and the next one
hid behind the previous one's failure:

1. **Register every new `*.qml` file in `apps/linsight-gui/build.rs`.**
   Add it to the `QmlModule::new(...).qml_files([...])` list. Without
   this the QML engine reports `Type FooPage unavailable` at module
   load and `Main.qml`'s `Component { id: fooPage; FooPage { ... } }`
   declaration aborts the whole `QQmlApplicationEngine`.
2. **Register every new `qobjects/*.rs` file in the same
   `build.rs`.** Add it to the `CxxQtBuilder` `.file("...")` chain.
   Without this the `#[cxx_qt::bridge]` codegen never runs for that
   QObject, the QML registration is empty, and any `FooModel { id: x }`
   declaration in QML fails at module load. The Justfile lupdate6 list
   has the same "explicit list, no globbing" property.
3. **`import QtQuick.Controls as Controls` requires `Controls.`
   on *every* type from that module.** `ScrollView`, `BusyIndicator`,
   `Switch`, `Button`, `ScrollBar` (including its attached-property
   form: `Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff`),
   etc. The cxx-qt-build QML AOT compiler validates this when each
   `Component { FooPage {} }` is first realized — the file parses
   syntactically but blows up with `ScrollView is not a type` at
   runtime. A workspace-wide scan that fits in one shell line:
   ```
   cd apps/linsight-gui/qml && for f in *.qml; do
       a=$(grep -oE "QtQuick.Controls as [A-Za-z]+" "$f" | awk '{print $NF}')
       [ -n "$a" ] && grep -nE "^\s*(ScrollView|BusyIndicator|Switch|Button|Label|TextField|TextArea|ComboBox|CheckBox|RadioButton|Slider|SpinBox|ProgressBar|ToolBar|MenuBar|Menu|MenuItem|Frame|Pane|GroupBox|StackView|SwipeView|TabBar|TabButton|DialogButtonBox|Dialog|Popup|ToolTip|ItemDelegate|RoundButton|ToolButton|ScrollBar) \{" "$f" \
         | grep -v "$a\." | sed "s|^|$f:|"
   done
   ```
4. **Leading-underscore QML property names break change handlers.**
   `property var _pts: ...` followed by `on_ptsChanged: { ... }` is
   rejected by the QML AOT compiler with `Cannot assign to non-existent
   property "on_ptsChanged"`. The property *declaration* itself is
   fine — using the auto-derived signal handler is what fails. Rename
   to `samples` / `onSamplesChanged` if you need the change hook.
5. **Rust QObject method names must not collide with C++ reserved
   words.** `#[qinvokable] fn delete(...)` emits `void delete(...)` in
   the generated C++ which fails to compile. `auto_cxx_name` will not
   rescue you. Rename to `delete_rule` / `deleteRule` (or use
   `#[cxx_name = ...]`).

**Validation that catches all five before a release rebuild:**

```bash
just build-release
./scripts/dev_screenshot.sh overview /tmp/shot.png   # writes PNG
journalctl --user --since "30 sec ago" | grep -iE "qml|linsight"
```

If the PNG is missing or zero bytes, or the journal contains
`QQmlApplicationEngine failed to load component` / `Type X
unavailable` / `Cannot assign to non-existent property` / `is not a
type`, the build is broken even though `cargo build --release`
returned 0. The dev_screenshot path goes through the full
`QQuickWindow::grabWindow()` pipeline — same code path the real
launch hits — so a successful PNG is strong evidence the engine
loaded every Component declaration in Main.qml without an AOT
compiler error.

## Tests

```bash
cargo test --workspace      # 489 pass at the time of writing
cargo test -p <crate> <name>   # single test, e.g. -p linsight-sensors-cpu sample_parses_proc_stat
just gui-smoke              # xvfb headless handshake smoke (shell script)
```

Notable test infrastructure:

- **`crates/linsight-plugin-sdk/tests/dynamic_load.rs`** — closes the
  "fabricated test" gap from commit `8c301d5`. Builds
  `examples/echo-plugin` via escargot, dlopens it, and exercises the
  full ABI v6 load path (version symbol, `get_stabbied` factory,
  `host_init`/`host_sample` round-trip).
- **`apps/linsight-tunnel/tests/mtls_smoke.rs`** — closes the open
  follow-up. Two tests: rcgen-generated cert chain handshake +
  byte round trip, and rogue-client cert rejection.
- **`apps/linsightd/src/alerts.rs::tests::shell_split_*`** — covers
  the `exec:<argv>` notify target that replaced the RCE-prone
  `shell:<cmd>` target.

Sensor crates test against synthetic `/sys` fixtures using
`tempfile::TempDir`. Hardware-dependent tests are `#[ignore]` by
default — including the cpu/mem live-`/proc` reads that previously
ran unmarked. The GUI smoke lives as a shell script rather than a
`cargo test` because `cxx-qt-build` doesn't link cleanly into a
separate test target; the script now distinguishes timeout (exit
124) from clean exit and uses GNU automake's skip convention (exit
77) when `xvfb-run` is absent.

## Reporting

- Bugs / feature requests: GitHub issues at
  <https://github.com/visorcraft/linsight/issues>.
- Security: see `docs/security.md`.

## Prompt-file hygiene for subagents & peer reviewers (MANDATORY)

NEVER write prompts for subagents or peer code reviewers (e.g. `opencode run "$(cat ...)"`)
to the shared generic path `/tmp/prompt.txt`. Multiple agent sessions run concurrently on
this machine and follow the same conventions; a shared path WILL be clobbered between
writing the prompt and the consumer reading it at launch, silently running the review or
task against another project's brief (this has actually happened).

Required: use a collision-proof path —
- per-repo: `/tmp/linsight-prompt.txt`, or
- better, a unique random path so multiple subagents can work on this repo concurrently
  with varying prompts: `PROMPT_FILE=$(mktemp /tmp/linsight-prompt-XXXXXXXX.txt)`

The same rule applies to any scratch file consumed via `$(cat ...)` or read by the
subagent at launch time (review diffs, briefs, fixture lists).
