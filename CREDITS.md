# Credits and Attribution

## Copyright

LinSight is © VisorCraft LLC and contributors, distributed under the
[GNU General Public License v3.0](LICENSE).

## Runtime dependencies

LinSight links against the following system runtimes at execution time.
None are bundled into the GPL source distribution; downstream packagers
(Flatpak, AppImage, distro repos) handle redistribution.

| Component | License | Project |
| --------- | ------- | ------- |
| Qt 6 (Core, Qml, Gui, Quick) | LGPL-3.0 / GPL-3.0 / commercial | https://www.qt.io |
| KDE Frameworks 6 — Kirigami | LGPL-2.1+ | https://invent.kde.org/frameworks/kirigami |
| NVIDIA Management Library (`libnvidia-ml.so`) | NVIDIA proprietary, redistributable | https://developer.nvidia.com/nvidia-management-library-nvml |
| Linux kernel sysfs / drm subsystem (`/sys/class/drm`, `/sys/class/hwmon`) | GPL-2.0 | https://kernel.org |
| Linux kernel `xe` driver (Intel Arc) | MIT / GPL-2.0 dual | https://kernel.org |
| `libsensors` (host hwmon abstraction, transitive via hwmon sysfs) | LGPL-2.1+ | https://github.com/lm-sensors/lm-sensors |

## Rust crate dependencies

LinSight pulls in the following directly-used crates from crates.io. The
full machine-generated transitive supplement — every crate, its exact
version, and the full text of every distinct license — lives in the
in-app **Licenses → Third-party** page and is mirrored at
[`docs/third-party-notices.md`](docs/third-party-notices.md). Regenerate
it via `just credits` (which runs `cargo about` over `Cargo.lock`).
`cargo deny check` (configured in `deny.toml`) enforces license
compatibility on every CI run.

### Qt / GUI bridge

| Crate | License | Project |
| ----- | ------- | ------- |
| `cxx-qt`, `cxx-qt-lib`, `cxx-qt-build`, `qt-build-utils` | MIT OR Apache-2.0 | [KDAB/cxx-qt](https://github.com/KDAB/cxx-qt) |
| `cxx` | MIT OR Apache-2.0 | [dtolnay/cxx](https://github.com/dtolnay/cxx) |

### Plugin ABI

| Crate | License | Project |
| ----- | ------- | ------- |
| `stabby` (FFI-stable types for the runtime plugin ABI) | MIT OR Apache-2.0 | [ZettaScaleLabs/stabby](https://github.com/ZettaScaleLabs/stabby) |
| `libloading` (dlopen wrapper for `.so` plugin discovery) | ISC | [nagisa/rust_libloading](https://github.com/nagisa/rust_libloading) |

### Sensors + hardware

| Crate | License | Project |
| ----- | ------- | ------- |
| `nvml-wrapper` | MIT OR Apache-2.0 | [Cldfire/nvml-wrapper](https://github.com/Cldfire/nvml-wrapper) |

### Daemon ↔ client wire protocol

| Crate | License | Project |
| ----- | ------- | ------- |
| `postcard` (compact serde wire format) | MIT OR Apache-2.0 | [jamesmunns/postcard](https://github.com/jamesmunns/postcard) |
| `polling` (epoll/kqueue wrapper for the sync daemon hot path) | Apache-2.0 OR MIT | [smol-rs/polling](https://github.com/smol-rs/polling) |
| `signal-hook` | Apache-2.0 OR MIT | [vorner/signal-hook](https://github.com/vorner/signal-hook) |

### Optional always-on subsystems

| Crate | License | Project |
| ----- | ------- | ------- |
| `rusqlite` (SQLite for opt-in history) | MIT | [rusqlite/rusqlite](https://github.com/rusqlite/rusqlite) |
| `evalexpr` (alert expression engine) | MIT | [ISibboI/evalexpr](https://github.com/ISibboI/evalexpr) |
| `notify-rust` (desktop notifications for alerts) | MIT OR Apache-2.0 | [hoodie/notify-rust](https://github.com/hoodie/notify-rust) |

### mTLS remote tunnel (`linsight-tunnel`)

| Crate | License | Project |
| ----- | ------- | ------- |
| `tokio`, `tokio-rustls` | MIT | [tokio-rs/tokio](https://github.com/tokio-rs/tokio) |
| `rustls`, `rustls-pki-types` | Apache-2.0 OR ISC OR MIT | [rustls/rustls](https://github.com/rustls/rustls) |

### Serialization + CLI plumbing

| Crate | License | Project |
| ----- | ------- | ------- |
| `serde`, `serde_derive`, `serde_json` | MIT OR Apache-2.0 | [serde-rs/serde](https://github.com/serde-rs/serde) |
| `clap` | MIT OR Apache-2.0 | [clap-rs/clap](https://github.com/clap-rs/clap) |
| `toml` | MIT OR Apache-2.0 | [toml-rs/toml](https://github.com/toml-rs/toml) |
| `anyhow`, `thiserror` | MIT OR Apache-2.0 | [dtolnay/anyhow](https://github.com/dtolnay/anyhow), [dtolnay/thiserror](https://github.com/dtolnay/thiserror) |
| `chrono` (RFC3339 timestamps on persisted dashboards) | MIT OR Apache-2.0 | [chronotope/chrono](https://github.com/chronotope/chrono) |

### Logging

| Crate | License | Project |
| ----- | ------- | ------- |
| `tracing`, `tracing-subscriber` | MIT | [tokio-rs/tracing](https://github.com/tokio-rs/tracing) |

### Dev / test-only

| Crate | License | Project |
| ----- | ------- | ------- |
| `proptest` | MIT OR Apache-2.0 | [proptest-rs/proptest](https://github.com/proptest-rs/proptest) |
| `tempfile` | MIT OR Apache-2.0 | [Stebalien/tempfile](https://github.com/Stebalien/tempfile) |
| `assert_cmd`, `predicates` | MIT OR Apache-2.0 | [assert-rs](https://github.com/assert-rs) |
| `escargot` (rebuilds the example plugin under `tests/dynamic_load.rs`) | MIT OR Apache-2.0 | [assert-rs/escargot](https://github.com/assert-rs/escargot) |

## License compatibility

GPL-3.0-only is compatible with every license listed above. Specifically:

- MIT / Apache-2.0 / BSD-3-Clause / ISC are permissive and combine freely.
- `Unicode-3.0` (transitive via ICU) is FSF-approved as GPL-compatible.
- `Unlicense` (any ripgrep-family deps reached transitively) is FSF-approved as GPL-compatible.
- `Zlib` (used by `miniz_oxide`, `foldhash`, `tinyvec`) is FSF-approved as GPL-compatible.

The `deny.toml` allowlist enforces this. New licenses outside the
allowlist fail the `cargo deny check` step in CI.

## Reporting attribution gaps

If you find code or assets in this repository that we have failed to
credit, please open an issue at
<https://github.com/visorcraft/linsight/issues> and we will correct the
record.
