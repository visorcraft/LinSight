<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# LinSight Phase 2 — GUI Overview MVP

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Qt 6 / Kirigami GUI as a thin client of `linsightd`,
showing the Overview preset page with live CPU and RAM tiles updating
in real time.

**Architecture:** A new `apps/linsight-gui` binary (crate name:
`linsight`) using `cxx-qt 0.8` to bridge Rust to Qt 6 / QML. On startup
the GUI tries `$XDG_RUNTIME_DIR/linsight.sock`; if no daemon is
listening, it spawns `linsightd` as a child. A background client
thread owns a postcard connection, subscribes to sensors the visible
page is showing, and pushes samples to a Rust-side model via
`cxx_qt::Threading::queue` callbacks. QML reads the model through
`#[qproperty]` accessors. A new `linsight-sensors-mem` crate exposes
`mem.used_bytes` and `mem.total_bytes` so the Overview tiles have RAM
data to render.

**Tech Stack:** Rust 2024, `cxx-qt 0.8`, `cxx-qt-lib` with
`qt_gui` + `qt_qml`, Qt 6 (>= 6.10), Kirigami 6, `tracing` for logs.
Daemon work uses the existing `linsight-protocol` and `linsight-core`.

**Reference spec:** [`../specs/2026-05-25-linsight-design.md`](../specs/2026-05-25-linsight-design.md)
**Reference roadmap:** [`./2026-05-25-phases-roadmap.md`](./2026-05-25-phases-roadmap.md)

---

## File structure for this plan

```
linsight/
├── apps/
│   └── linsight-gui/                      ← NEW (crate name: `linsight`)
│       ├── Cargo.toml                     ← Task 4
│       ├── build.rs                       ← Task 5 (cxx-qt-build)
│       ├── src/
│       │   ├── main.rs                    ← Task 6
│       │   ├── client.rs                  ← Tasks 8-10 (postcard client)
│       │   ├── workspace.rs               ← Task 7 (shared TLS state)
│       │   └── qobjects/
│       │       ├── mod.rs                 ← Task 11
│       │       ├── overview_model.rs      ← Tasks 12-13
│       │       └── sensor_tile.rs         ← Task 11
│       └── qml/
│           ├── Main.qml                   ← Task 14
│           ├── OverviewPage.qml           ← Task 15
│           └── SensorTile.qml             ← Task 16
│
└── crates/linsight-sensors/
    └── mem/                               ← NEW
        ├── Cargo.toml                     ← Task 1
        └── src/
            ├── lib.rs                     ← Task 1
            ├── meminfo.rs                 ← Task 2
            └── plugin.rs                  ← Task 3
```

Plus daemon registration of `MemPlugin` (Task 3).

End state: `cargo run -p linsight` opens a Kirigami window titled
"LinSight — Overview" showing two tiles ("CPU" and "Memory") with
live values updating at 1 Hz.

---

## Task 1: linsight-sensors-mem crate skeleton

**Files:**
- Create: `crates/linsight-sensors/mem/Cargo.toml`
- Create: `crates/linsight-sensors/mem/src/lib.rs`

Same pattern as `linsight-sensors-cpu`. Cargo.toml depends on
`linsight-core` and `linsight-plugin-sdk` via versioned path deps.
`src/lib.rs` declares `mod meminfo` and `mod plugin`, re-exports
`MemPlugin`. Add the crate to the workspace `members` list.

---

## Task 2: meminfo.rs — parse /proc/meminfo

Mirror the `proc_stat.rs` shape from `linsight-sensors-cpu`: a `Meminfo`
struct, a `parse_meminfo(&str)` returning `Result<Meminfo, MemError>`,
and a `read_meminfo(sysroot: Option<&Path>)` honoring the
`PluginCtx::sysroot` override. Cover with unit tests against a synthetic
`/proc/meminfo` written to a `tempfile::TempDir`. Keys to parse: at
minimum `MemTotal`, `MemAvailable`, `MemFree`, `SwapTotal`, `SwapFree`
(all in kB, multiplied by 1024 to bytes). `used_bytes() = total - available`.

---

## Task 3: MemPlugin + daemon registration

`MemPlugin` impls `LinsightPlugin`, exposes two sensors:
`mem.used_bytes` (1 Hz native) and `mem.total_bytes` (0.1 Hz native, it
rarely changes). Wire it into `apps/linsightd/src/plugin_host.rs`'s
`with_builtins()` alongside `CpuPlugin`, and add the dependency to
`apps/linsightd/Cargo.toml`. Update the daemon's `PluginInfo`
advertisement (currently hard-coded for CPU only in
`transport::unix::serve`) to derive the list from the `PluginHost`.

---

## Task 4: linsight-gui crate skeleton

`apps/linsight-gui/Cargo.toml`:

```toml
[package]
name = "linsight"
# ...workspace inheritance...

[[bin]]
name = "linsight"
path = "src/main.rs"

[dependencies]
anyhow = { workspace = true }
cxx = "1.0"
cxx-qt = "0.8"
cxx-qt-lib = { version = "0.8", features = ["qt_gui", "qt_qml"] }
linsight-core = { path = "../../crates/linsight-core", version = "0.1.0" }
linsight-protocol = { path = "../../crates/linsight-protocol", version = "0.1.0" }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

[build-dependencies]
cxx-qt-build = "0.8"
```

Add `"apps/linsight-gui"` to workspace members.

---

## Task 5: linsight-gui build.rs

```rust
use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new()
        .qt_module("Qml")
        .qt_module("Quick")
        .qml_module(QmlModule {
            uri: "com.visorcraft.LinSight",
            rust_files: &[
                "src/qobjects/overview_model.rs",
                "src/qobjects/sensor_tile.rs",
            ],
            qml_files: &[
                "qml/Main.qml",
                "qml/OverviewPage.qml",
                "qml/SensorTile.qml",
            ],
            ..Default::default()
        })
        .build();
}
```

---

## Task 6: linsight-gui main.rs

Minimal `QGuiApplication` boot, loads `qrc:/qt/qml/com/visorcraft/LinSight/qml/Main.qml`,
runs the Qt event loop via `QGuiApplication`'s standard run method.
Installs `tracing_subscriber` with `LINSIGHT_LOG` env filter.
Constructs a `Workspace` (Task 7) that holds the `Client` (Task 8) and
installs it on the main thread before the engine loads QML.

---

## Task 7: workspace.rs — thread-local shared state

A `Workspace` struct holding the `Arc<Client>`. Installed via
`workspace::install(ws)` on the main thread; QObject `new()` impls
fetch it via `workspace::current()`. Same pattern as Grexa.

---

## Task 8: client.rs — postcard client (request side)

`Client::connect_or_spawn(socket: &Path)`:
- Try `UnixStream::connect(socket)`. If it succeeds, return the stream.
- Otherwise locate the `linsightd` binary (sibling to `current_exe`,
  fallback to `PATH`), spawn it with `--socket=<path>`, poll for the
  socket file every 50 ms up to 3 s, then connect.

Once connected, send `Hello`, expect `Welcome`, spawn a background
thread that runs `FrameReader::read_server()` in a loop, forwarding
`Sample` frames to a `std::sync::mpsc::Sender<Sample>` exposed to the
GUI.

`subscribe(sensors)` / `unsubscribe(sensors)` send the corresponding
`ClientMsg` over a `Mutex<FrameWriter<UnixStream>>`.

---

## Task 9: client.rs — clean teardown of spawned child

On `Client::drop`: send `Goodbye`, then if we own the daemon `Child`,
wait up to 1 s for it to exit; fall back to `child.kill()` on timeout.
Adds an integration test that asserts no orphaned `linsightd` process
remains after the Client is dropped.

---

## Task 10: client.rs — integration test

Use the `escargot`-backed harness from `linsight-cli/tests/helpers/`
(extracted to a shared dev helper or duplicated here for v1). Spin a
daemon against a temp socket; `connect_or_spawn` against it (will
attach, not spawn); subscribe to `cpu.util`; receive >= 1 sample
within 3 s.

---

## Task 11: qobjects/sensor_tile.rs

A `SensorTile` QObject with `#[qproperty]` for `display_name`,
`value_text`, `unit_symbol`. QML binds these directly. The Rust
backing struct is a plain `struct` with `QString` fields; setters are
auto-generated by `cxx-qt` and emit change signals.

---

## Task 12: qobjects/overview_model.rs

An `OverviewModel` QObject that:
- Holds two `SensorTile` instances (one for CPU, one for memory).
- On construction (`new()` impl), grabs the workspace's `Client`,
  subscribes to `cpu.util` and `mem.used_bytes`.
- Spawns a thread that drains the sample receiver and, via
  `cxx_qt::Threading::queue`, posts updates to the corresponding tile's
  `value_text` `qproperty`.

Exposes `cpu_tile()` and `mem_tile()` invokables to QML.

---

## Task 13: qobjects/mod.rs aggregation + cxx-qt registration

```rust
pub mod overview_model;
pub mod sensor_tile;
```

Both modules use `#[cxx_qt::bridge]` and are registered automatically
by `cxx-qt-build` via the `QmlModule` block in `build.rs`. The QML
import is `import com.visorcraft.LinSight`.

---

## Task 14: qml/Main.qml — Kirigami ApplicationWindow

```qml
import QtQuick
import QtQuick.Controls
import org.kde.kirigami as Kirigami

Kirigami.ApplicationWindow {
    id: root
    title: i18n("LinSight — Overview")
    width: 900
    height: 600
    visible: true
    pageStack.initialPage: OverviewPage {}
}
```

---

## Task 15: qml/OverviewPage.qml — two-tile grid

```qml
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import com.visorcraft.LinSight

Kirigami.Page {
    title: i18n("Overview")
    OverviewModel { id: model }
    GridLayout {
        anchors.fill: parent
        columns: 2
        SensorTile { Layout.fillWidth: true; tile: model.cpuTile }
        SensorTile { Layout.fillWidth: true; tile: model.memTile }
    }
}
```

---

## Task 16: qml/SensorTile.qml — visual card

```qml
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Card {
    property var tile
    contentItem: ColumnLayout {
        Kirigami.Heading { level: 4; text: tile.displayName }
        Label {
            text: tile.valueText + " " + tile.unitSymbol
            font.pixelSize: Kirigami.Theme.defaultFont.pixelSize * 2.5
        }
    }
}
```

---

## Task 17: Subscription lifecycle

`OverviewModel`'s constructor subscribes to the sensors it shows; its
`Drop` impl unsubscribes. For multi-page navigation in later phases,
subscription will move to "page visible" signal handlers (a small
refactor — out of scope for this MVP).

---

## Task 18: End-to-end smoke

- [ ] `cargo run -p linsight` opens a window titled
      "LinSight — Overview".
- [ ] Two tiles render: "CPU utilization" (in %) and "Memory used"
      (in B; pretty formatting comes later).
- [ ] Values update at 1 Hz; running a CPU-burner (`yes > /dev/null`)
      pushes CPU% up.
- [ ] Closing the window terminates both the GUI process and the
      spawned daemon child. Verify with `pgrep linsightd`.

---

## Self-review

| Phase 2 goal | Covered by |
|---|---|
| Memory sensor available in daemon | Tasks 1-3 |
| Qt 6 / Kirigami window opens | Tasks 4-6, 14 |
| cxx-qt 0.8 bridge to QML | Tasks 11-13 |
| Postcard client in worker thread | Tasks 8-10 |
| Live tile updates from sample stream | Task 12 |
| GUI auto-spawns daemon if none running | Task 8 `connect_or_spawn` |
| Daemon child is reaped on GUI exit | Task 9 |

### Known follow-ups deferred to later phases

- Pretty unit formatting (e.g., "23.4 GiB" instead of raw bytes) —
  Phase 9 polish.
- Theme / dark mode polish — Phase 9.
- Page switcher / multi-page — Phase 6 custom canvas.
- QML hot-reload during development — never (build-time bundling is
  by design; Grexa same).

### Risks

- **cxx-qt 0.8 + Qt 6.11 compatibility.** Grexa runs this combination
  in production; should be fine. If cxx-qt fails against newer Qt
  signals, document a Qt 6.10 floor in
  `docs/build-and-test.md` until cxx-qt updates.
- **Kirigami 6.26 component drift.** All components used here
  (`ApplicationWindow`, `Page`, `Card`, `Heading`) have been stable
  for several releases.
