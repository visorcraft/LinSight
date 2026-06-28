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
| `stabby` (FFI-stable types for the runtime plugin ABI) | EPL-2.0 OR Apache-2.0 (taken under Apache-2.0) | [ZettaScaleLabs/stabby](https://github.com/ZettaScaleLabs/stabby) |
| `libloading` (dlopen wrapper for `.so` plugin discovery) | ISC | [nagisa/rust_libloading](https://github.com/nagisa/rust_libloading) |

### Sensors + hardware

| Crate | License | Project |
| ----- | ------- | ------- |
| `nvml-wrapper` | MIT OR Apache-2.0 | [Cldfire/nvml-wrapper](https://github.com/Cldfire/nvml-wrapper) |
| `zbus` (D-Bus client for SMART disk health via udisks2) | MIT | [dbus2/zbus](https://github.com/dbus2/zbus) |

### Daemon ↔ client wire protocol

| Crate | License | Project |
| ----- | ------- | ------- |
| `postcard` (compact serde wire format) | MIT OR Apache-2.0 | [jamesmunns/postcard](https://github.com/jamesmunns/postcard) |
| `signal-hook` | Apache-2.0 OR MIT | [vorner/signal-hook](https://github.com/vorner/signal-hook) |
| `subtle` (constant-time comparison for auth-token checks) | BSD-3-Clause | [dalek-cryptography/subtle](https://github.com/dalek-cryptography/subtle) |
| `libc` (Unix `SO_PEERCRED` auth in the daemon and `statvfs` filesystem stats in the fs sensor) | MIT OR Apache-2.0 | [rust-lang/libc](https://github.com/rust-lang/libc) |

### Optional always-on subsystems

| Crate | License | Project |
| ----- | ------- | ------- |
| `rusqlite` (SQLite for opt-in history) | MIT | [rusqlite/rusqlite](https://github.com/rusqlite/rusqlite) |
| `evalexpr` (alert expression engine) | AGPL-3.0-only | [ISibboI/evalexpr](https://github.com/ISibboI/evalexpr) |
| `notify-rust` (desktop notifications for alerts) | MIT OR Apache-2.0 | [hoodie/notify-rust](https://github.com/hoodie/notify-rust) |
| `ureq` (HTTP client for alert webhook notifications) | MIT OR Apache-2.0 | [algesten/ureq](https://github.com/algesten/ureq) |

### mTLS remote tunnel (`linsight-tunnel`)

| Crate | License | Project |
| ----- | ------- | ------- |
| `tokio` | MIT | [tokio-rs/tokio](https://github.com/tokio-rs/tokio) |
| `tokio-rustls` | MIT OR Apache-2.0 | [rustls/tokio-rustls](https://github.com/rustls/tokio-rustls) |
| `rustls` | Apache-2.0 OR ISC OR MIT | [rustls/rustls](https://github.com/rustls/rustls) |
| `rustls-pki-types` (types shared by the `rustls`/`tokio-rustls` stack; pinned in `workspace.dependencies`) | MIT OR Apache-2.0 | [rustls/pki-types](https://github.com/rustls/pki-types) |
| `x509-parser` (client certificate parsing for mTLS) | MIT OR Apache-2.0 | [rusticata/x509-parser](https://github.com/rusticata/x509-parser) |

### Serialization + CLI plumbing

| Crate | License | Project |
| ----- | ------- | ------- |
| `serde`, `serde_json` | MIT OR Apache-2.0 | [serde-rs/serde](https://github.com/serde-rs/serde) |
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
| `criterion` (benchmark harness for core + protocol) | MIT OR Apache-2.0 | [bheisler/criterion.rs](https://github.com/bheisler/criterion.rs) |
| `tempfile` | MIT OR Apache-2.0 | [Stebalien/tempfile](https://github.com/Stebalien/tempfile) |
| `assert_cmd`, `predicates` | MIT OR Apache-2.0 | [assert-rs](https://github.com/assert-rs) |
| `escargot` (rebuilds the example plugin under `tests/dynamic_load.rs`) | MIT OR Apache-2.0 | [crate-ci/escargot](https://github.com/crate-ci/escargot) |
| `rcgen` (test certificate generation for mTLS smoke tests) | MIT OR Apache-2.0 | [est31/rcgen](https://github.com/est31/rcgen) |

## License compatibility

LinSight is conveyed under GPL-3.0-only. Every dependency above is either
GPL-compatible outright or offered under a dual license whose
GPL-compatible option we take:

- MIT / Apache-2.0 / BSD-3-Clause / ISC are permissive and combine freely.
- `stabby` is `EPL-2.0 OR Apache-2.0`; we take it under **Apache-2.0**.
  EPL-2.0 on its own is not GPL-compatible — the `OR` lets us avoid it,
  and `cargo about` reproduces only the Apache-2.0 text for it.
- `evalexpr` is `AGPL-3.0-only`. GPLv3 §13 explicitly permits combining a
  GPL-3.0 work with AGPL-3.0 code; the combined work as conveyed then
  carries AGPL §13's network-interaction source-offer requirement.
- `Unicode-3.0` (transitive via `unicode-ident`) is FSF-approved as GPL-compatible.
- `Zlib` (used by `foldhash`) is FSF-approved as GPL-compatible.
- `0BSD` (transitive via `adler2`) is a public-domain-equivalent permissive
  license — strictly laxer than ISC — and combines freely.
- `CDLA-Permissive-2.0` (the bundled Mozilla CA-root data in `webpki-roots`)
  is a permissive data license with no copyleft, imposing no conditions on
  combination or conveyance.

The `deny.toml` allowlist also permits `Unlicense` for any future
transitive deps; no current crate uses it. New licenses outside the
allowlist fail the `cargo deny check` step in CI.

## Reporting attribution gaps

If you find code or assets in this repository that we have failed to
credit, please open an issue at
<https://github.com/visorcraft/linsight/issues> and we will correct the
record.
