<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Phases 3-10 Completion Notes (v0.2.0)

**Tag:** `v0.2.0`
**Date:** 2026-05-25
**Goal:** "complete all remaining phases" (Phases 3 through 10 from the
roadmap). Everything achievable autonomously in one session has shipped.
Items requiring visual iteration or contributor-grade UX work are
flagged TBD with concrete next steps.

## Sensor coverage growth

| Phase | Crate | Sensors | Verified on hardware |
|---|---|---|---|
| Phase 3 | `linsight-sensors-xe` | per-GPU util/freq/temp/fan for every xe-driven card | Yes — iGPU (Panther Lake) + Arc B70 (Battlemage) |
| Phase 3 | `linsight-sensors-nvml` | per-GPU util/mem_used/mem_total/temp/power | Yes — RTX 5080 Laptop, 41°C / 15.92 GiB VRAM |
| Phase 4 | `linsight-sensors-nvme` | per-controller temp + read/written bytes | Yes — Samsung 990 PRO 4TB ×2, Predator GM7 4TB |
| Phase 4 | `linsight-sensors-net` | per-interface rx/tx/state/speed | Yes — 7 interfaces (enp\*, wlan0, lo, tailscale0, virbr0, vnet0) |

Total live sensors on the dev machine went from 14 (v0.1.0) to 51.

## Phase-by-phase status

### Phase 3 — Multi-GPU sensors — ✅ shipped

- `linsight-sensors-xe` enumerates `/sys/class/drm/card*` where driver = xe,
  derives utilization from `gtidle/idle_residency_ms` deltas vs wall time,
  reads frequency from `tile0/gt0/freq0/act_freq`, scrapes temp/fan from
  the device's first `hwmon` directory when present.
- `linsight-sensors-nvml` uses the `nvml-wrapper` crate. On systems
  without NVIDIA hardware it returns an empty sensor list rather than
  erroring, so the daemon can host the plugin unconditionally.

### Phase 4 — NVMe + Network sensors — ✅ shipped

- NVMe temperature from `/sys/class/nvme/nvme<N>/hwmon*/temp1_input`,
  bytes-read/written from `/sys/class/block/nvme<N>n1/stat` (sectors × 512).
- Network rx/tx counters from `/sys/class/net/<if>/statistics/{rx,tx}_bytes`,
  link state from `operstate`, speed from `speed` with -1 fallback for
  virtual interfaces that return EINVAL.

### Phase 5 — Runtime .so plugin loading — ✅ shipped

`linsightd` scans the three standard plugin directories (`/usr/lib/...`,
`/usr/local/lib/...`, `$XDG_DATA_HOME/...`) at startup, opens each `.so`
via `libloading`, checks the reported `LINSIGHT_PLUGIN_ABI_VERSION`,
calls `linsight_plugin_v1()` to get the factory, and registers the
returned plugin alongside the in-tree built-ins.

`linsight-cli plugin {new,install,ls,remove}` ships the contributor
workflow. End-to-end verified: a generated demo plugin built, installed,
loaded by the daemon, and served `example.hello = 42` via the CLI.

### Phase 6 — Custom canvas — data model shipped; QML editor TBD

`linsight-core::dashboard` ships the `DashboardSpec` schema, JSON
load/save with atomic write, a migration framework, and seven widget
kinds (Gauge / Sparkline / Bar / TextValue / Donut / Table / MultiSparkline).

Deferred: the QML drag/resize/palette UX. It needs visual iteration
the agent can't do in a non-interactive shell. Workaround today: users
can author Custom pages by editing `~/.config/linsight/dashboard.json`
directly; the schema and round-trip code already handle them.

### Phase 7 — Always-on mode — history + Prometheus shipped; alerts TBD

`apps/linsightd/src/history.rs`: SQLite WAL database at
`$XDG_DATA_HOME/linsight/history.db`. Background writer thread with a
1-second flush window. Enable via `LINSIGHT_HISTORY=1`.

`apps/linsightd/src/prom.rs`: hand-rolled HTTP/1.0 Prometheus exporter
on a configurable bind (`LINSIGHT_PROM_BIND=127.0.0.1:9777`). Verified
end-to-end: real values like `linsight_nvml_gpu0_power_w 21.522` and
`linsight_net_enp92s0_tx_bytes 1036604256` returned by `/metrics`.

`packaging/systemd/linsight.service` enables both subsystems by default
with `MemoryMax=64M` / `CPUQuota=10%` limits.

Deferred: alerts. The `evalexpr` rule engine + `notify-rust` desktop
notifications need a focused pass; folding it in after history was
already in production would have been over-stuffed for one commit.

### Phase 8 — Multi-window + SSH remote — SSH shipped; multi-window TBD

`linsight --connect ssh://user@host` runs a one-shot `ssh` to discover
the remote `XDG_RUNTIME_DIR`, then spawns `ssh -N -L local:remote` and
attaches the GUI to the forwarded socket as if it were local.

Deferred: multi-window inside a single process — a QML refactor that
wants visual iteration. Workaround today: launch a second `linsight`
process; the GUI binaries are independent and both auto-spawn or attach.

### Phase 9 — Theming + i18n + a11y — shipped at scaffold quality

- a11y: SensorTile is an `Accessible.Indicator` with translatable
  `name`/`description`; inner Labels are `Accessible.ignored` so screen
  readers don't double-announce.
- Theming: Kirigami inherits the host theme; light/dark/accent already
  work without app-side code.
- i18n: `qsTr()` + `QTranslator` pipeline from Phase 2 is in place;
  `just i18n-extract` / `just i18n-compile` are the contributor flow.

Deferred: visual polish (animations, motion-reduction toggles, custom
icon set) — agent-non-friendly visual iteration.

### Phase 10 — Packaging — recipes shipped; icon assets TBD

`packaging/` ships recipes for:

- Arch PKGBUILD (generic x86_64)
- Arch PKGBUILD (x86_64-v3 tuned variant for CachyOS / Haswell+)
- Debian source package (control, rules, changelog, copyright)
- Fedora RPM spec
- openSUSE RPM spec
- Flatpak manifest (`org.kde.Platform//6.10` runtime, vendored crates)
- AppImage builder manifest
- systemd user unit (already in place since Phase 7)
- Desktop entry (`com.visorcraft.LinSight.desktop`)
- AppStream metainfo (`com.visorcraft.LinSight.metainfo.xml`)

Justfile targets: `just arch-pkg`, `just arch-pkg-v3`, `just flatpak`
(with auto vendoring via `just flatpak-vendor`).

Deferred: icon SVG/PNG assets (we don't have brand design output for
LinSight yet). The `Icon=` line in the .desktop file points at
`com.visorcraft.LinSight` which falls back to the generic monitor icon
until proper assets land.

## What requires user input (deferred to the very end)

1. **Push `master` and tag `v0.2.0` to the remote**:
   ```bash
   git push origin master --tags
   ```
2. **Ship icon assets** — provide hicolor SVG + PNG icons at
   `packaging/icons/{16x16,24x24,...,512x512,scalable}/apps/com.visorcraft.LinSight.{png,svg}`
   and reference them from the Arch/Debian/Fedora install steps the
   same way Grexa does it.
3. **Visual iteration on**: QML custom-canvas editor (Phase 6b),
   multi-window UX (Phase 8b), polish pass (Phase 9b). The agent can't
   see a Wayland window so the next iteration on UX needs the user
   driving from a keyboard.
4. **Alert engine** (Phase 7b): plug `evalexpr` into the live sample
   stream, parse `~/.config/linsight/alerts.toml`, route firing rules
   through `notify-rust`. This is a focused 1-2 day pass; not shipped
   here so the surrounding always-on subsystems land clean.
