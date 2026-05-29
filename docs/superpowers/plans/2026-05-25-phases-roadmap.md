<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# LinSight Phases Roadmap

> **Reference spec:** [`../specs/2026-05-25-linsight-design.md`](../specs/2026-05-25-linsight-design.md)

LinSight v1 is decomposed into 10 phases. Each phase ships working,
testable software on its own and is captured in its own plan document.
Plans are written just-in-time — Plan 1 is fully detailed; later
plans get written as we approach them so they reflect what we
actually learned from earlier phases.

| # | Status | Plan | Goal | End-state | Depends on |
|---|---|---|---|---|---|
| 1 | ✅ shipped v0.1.0 | `2026-05-25-foundation-cli-mvp.md` | Workspace + core types + protocol + plugin SDK + first sensor + daemon + CLI | `linsight-cli read cpu.util` streams live values from the daemon | — |
| 2 | code built, pending visual verification | `2026-05-25-gui-overview-mvp.md` | Qt 6 / Kirigami GUI shell + Overview preset page | Launch `linsight`, see live CPU + RAM tiles | Plan 1 |
| 3 | ✅ shipped v0.2.0 | `2026-MM-DD-multi-gpu-sensors.md` | NVML + Intel xe sensors + multi-GPU rendering | Overview shows your iGPU + 5080 + B70 live | Plan 2 |
| 4 | ✅ shipped v0.2.0 | `2026-MM-DD-storage-network-sensors.md` | NVMe + Network sensors + Storage/Network preset pages | All three preset pages populated | Plan 3 |
| 5 | ✅ shipped v0.2.0 | `2026-MM-DD-runtime-plugin-loading.md` | Dynamic `.so` plugin loading + `linsight-cli plugin new/install` scaffold + quarantine | A third-party plugin can be authored, built, dropped into `~/.local/share/linsight/plugins/`, and appears in the GUI | Plan 4 |
| 6 | data model shipped; QML editor TBD | `2026-MM-DD-custom-canvas.md` | Snap-to-grid custom-page editor + all v1 widget kinds + `dashboard.json` persistence | User can author a custom page with Gauge / Sparkline / Bar / TextValue / Donut / Table / MultiSparkline widgets | Plan 4 |
| 7 | history + Prometheus shipped; alerts TBD | `2026-MM-DD-always-on-mode.md` | `linsight.service` user unit + SQLite history + `evalexpr` alerts + Prometheus `/metrics` | Alerts fire, Prometheus scrapes work, history graphs render | Plan 6 |
| 8 | SSH remote shipped; multi-window TBD | `2026-MM-DD-multi-window-remote.md` | Multi-window GUI + SSH-forwarded remote socket (+ optional mTLS) | A second window shows a remote machine's Overview alongside local | Plan 7 |
| 9 | a11y + qsTr/QTranslator shipped; visual polish TBD | `2026-MM-DD-theme-i18n-polish.md` | Light/dark + accent themes, Fluent i18n (en/de/ja), animations, accessibility pass | Polished v1 visual + a11y baseline | Plan 8 |
| 10 | recipes shipped; icon assets TBD | `2026-MM-DD-packaging-release.md` | Flatpak + AppImage + Arch (x86_64 + x86_64-v3) + Debian + Fedora + openSUSE recipes + first tagged release | `pacman -U linsight-1.0-1-x86_64-v3.pkg.tar.zst` installs on CachyOS | Plan 9 |

## Why this phasing

- **Plan 1 is the deepest bootstrap.** Once the trait + protocol + scheduler exist, every subsequent sensor is mostly "implement the trait" work — the cost curve drops sharply.
- **Plan 2 ships the GUI early** with only the simplest sensors so the cxx-qt + QML integration is settled before the visually rich GPU work starts.
- **Plans 3 and 4 add hardware coverage** sensor-by-sensor. Each plan is independently shippable as a `v0.x` release.
- **Plan 5 (runtime plugin loading)** is deferred until the in-tree sensors are stable, because it dogfoods the SDK that they already use.
- **Plans 7-8 add the ambitious-v1 features** that need the daemon to be solid first.
- **Plan 10 (packaging)** comes last on purpose — packaging early just slows iteration.

## Hardware CI requirements

Plans 3-5 need a CI machine with the actual hardware:

- An NVIDIA GPU (for NVML test coverage)
- An Intel GPU on the `xe` driver (iGPU is sufficient)
- An NVMe drive (test coverage works on any modern system)

The user's daily-driver Razer Blade 16 (2026) covers all three. CI
runners without that hardware run tests behind `#[ignore]` markers
and rely on synthetic sysfs fixtures.
