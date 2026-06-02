<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Plugin SDK

A LinSight plugin is a Rust `cdylib` exporting a stabby-annotated
factory. The daemon `dlopen`s it at startup via
`StabbyLibrary::get_stabbied`, type-checks the FFI vtable via
stabby's `_stabbied_v3_report` companion symbol, checks the
reported ABI version, validates every sensor ID via
`SensorId::try_new`, and registers the plugin's sensors alongside
the in-tree ones.

## Quickstart

```bash
linsight-cli plugin new my-sensor
cd my-sensor
# Edit src/lib.rs — implement LinsightPlugin.
# Then point linsight-plugin-sdk's `path = "..."` line in Cargo.toml
# at your local LinSight checkout (the scaffold defaults to
# `../linsight/crates/linsight-plugin-sdk`; the registry version
# isn't published yet).
cargo build --release
linsight-cli plugin install target/release/libmy_sensor.so
# Restart linsightd (or `systemctl --user restart linsight` for
# always-on mode).
linsight-cli list | grep my-sensor
```

The `plugin new` scaffold includes a working `LinsightPlugin` impl,
the required `stabby = "36"` dep, and a `[lib]` section with
`crate-type = ["cdylib"]`.

For a known-good reference implementation, see
[`examples/echo-plugin/`](../examples/echo-plugin/). The SDK's
`tests/dynamic_load.rs` builds it as a real `.so` and exercises the
full load path, so its shape is guaranteed to remain compatible with
whatever the current daemon expects.

## The trait

```rust
use linsight_plugin_sdk::{
    LinsightPlugin, RPluginCtx, RPluginError, RPluginManifest,
    RSensorDescriptor, export_plugin,
};
use linsight_plugin_sdk::mirror::{RReading, RUnit, RSensorKind, RCategory};
use linsight_plugin_sdk::linsight_core::SensorId;

#[derive(Default)]
pub struct MyPlugin;

impl LinsightPlugin for MyPlugin {
    extern "C-unwind" fn init(
        &self,
        _ctx: &RPluginCtx,
    ) -> stabby::result::Result<RPluginManifest, RPluginError> {
        let descriptor = RSensorDescriptor {
            id: SensorId::new("example.hello").into(),
            display_name: "Hello sensor".into(),
            unit: RUnit::Count,
            kind: RSensorKind::Scalar,
            category: RCategory::Custom,
            native_rate_hz: 1.0,
            min: None.into(),
            max: None.into(),
            device_id: None.into(),
        };
        let manifest = RPluginManifest {
            plugin_id: "io.example.myplugin".into(),
            display_name: "My plugin".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors: vec![descriptor].into(),
        };
        stabby::result::Result::Ok(manifest)
    }

    extern "C-unwind" fn sample(
        &self,
        sensor: &linsight_plugin_sdk::RSensorId,
    ) -> stabby::result::Result<RReading, RPluginError> {
        if sensor.as_str() == "example.hello" {
            stabby::result::Result::Ok(RReading::Scalar(42.0))
        } else {
            stabby::result::Result::Err(
                RPluginError::Unsupported(sensor.to_string().into()),
            )
        }
    }
}

export_plugin!(MyPlugin);
```

The `extern "C-unwind" fn` calling convention on every trait method is a
stabby requirement; bodies stay plain Rust. `C-unwind` (as of ABI v6,
rather than plain `C`) lets a panic inside a plugin method unwind across the
FFI boundary so the daemon can catch it instead of aborting — your impl
signatures must say `extern "C-unwind" fn`. The conversion glue
between R-mirror types and the host's std-typed `linsight-core`
values lives in `linsight-plugin-sdk::mirror` and runs at the FFI
boundary only.

## ABI compatibility

`linsight_plugin_sdk::LINSIGHT_PLUGIN_ABI_VERSION` is `6`. The daemon
refuses to load a `.so` whose reported version doesn't match — it logs an
actionable error ("rebuild the plugin against linsight-plugin-sdk v6") and
**skips** that plugin; it does not crash, and a mismatched vtable is a
second backstop via stabby's reflection check. Bump the version on every
breaking change to the trait, the mirror types, the manifest, or the export
macro; the `export_plugin!` macro renames the factory symbol on each bump
(`linsight_plugin_v5` → `_v6`) so a stale `.so` fails the symbol lookup
rather than loading with an incompatible vtable.

**v6 migration:** v6 only changed the trait-method ABI from `extern "C"` to
`extern "C-unwind"` (for panic isolation). To port a v5 plugin, change each
`extern "C" fn init/sample/shutdown` in your `impl LinsightPlugin` to
`extern "C-unwind" fn` and rebuild — the compiler flags any you miss. No
other source changes are required.

The ABI uses **R-mirror types** in `linsight_plugin_sdk::mirror`
(`RUnit`, `RSensorKind`, `RCategory`, `RReading`, `RTableRow`,
`RCell`) that cross the FFI vtable. Unit-only enums are
`#[repr(u8)]`; payload-bearing types (`RUnit`, `RReading`, `RCell`)
are structs with an explicit `<...>Kind` `#[repr(u8)]` discriminant
plus payload fields — NOT stabby tagged enums. See
[`docs/adr/0001-plugin-abi-stabby-deferral.md`](adr/0001-plugin-abi-stabby-deferral.md)
for why; short version: stabby 36.2.2's tagged-enum `match_owned`
misroutes closures at `opt-level >= 1`, so a Percent value
round-trips to Celsius in release builds. The kind+payload struct
encoding bypasses the broken matcher entirely. They convert via
`From`/`Into` to the host's `linsight-core` types at the boundary,
so plugin authors get clean Rust types and the FFI surface stays
stabby-clean.

## Plugin lookup directories

`linsightd` scans these on startup, in order:

1. `/usr/lib/linsight/plugins/` — distro-shipped plugins
2. `/usr/local/lib/linsight/plugins/` — admin-installed
3. `$XDG_DATA_HOME/linsight/plugins/` — user-installed
   (defaults to `~/.local/share/linsight/plugins/`)

Sensor-id collisions log a warning; first registration wins.

## Idiomatic plugin shape

- One plugin = one hardware family or one feature surface.
- `init()` should be cheap and idempotent — it runs synchronously
  before the daemon accepts client connections.
- `sample()` runs on the per-client pump thread. Block all you
  want, but keep latency low: <1 ms per sample keeps the daemon
  responsive across many subscribers.
- Use the `device_id` field on `RSensorDescriptor` to group sensors
  belonging to the same physical device. The GUI uses it to render
  per-device cards on the preset pages.
- `native_rate_hz` is a hint to the scheduler; it clamps to
  `[MIN_RATE_HZ, MAX_RATE_HZ]` (currently `[0.1, 20.0]` — exposed
  as constants in `linsight_plugin_sdk::manifest`). Pick the rate
  that matches how often the underlying data changes meaningfully.
- Keep plugin bodies in private `init_inner` / `sample_inner` helpers
  with plain Rust types if you find the `extern "C-unwind" fn` signatures
  noisy — the in-tree sensors and `examples/echo-plugin/` all follow
  this pattern.
- **Sensor IDs**: every ID a plugin returns is run through
  `SensorId::try_new` by the host. An empty or whitespace-bearing
  string produces a `PluginError::Parse` and the plugin is rejected;
  the registry is never poisoned. Construct IDs via
  `SensorId::new(...)` in your inner code (debug-asserts the
  invariant) — the FFI validation will catch anything that slips
  past in release builds.
- **Sysroot override** (`PluginCtx::sysroot()`): if your plugin
  reads `/sys` or `/proc`, honor this — it's how the host points
  you at synthetic fixtures during tests. Construct contexts via
  `PluginCtx::new_with_sysroot(PathBuf)` which rejects non-UTF-8
  paths up front; the FFI mirror's UTF-8 contract holds because of
  that constructor check.
- **`shutdown()`** has a default no-op impl; override it if your
  plugin owns background threads or hardware handles that need
  explicit teardown. The host calls it for every plugin during
  `PluginHost::drop()`.

## Contributing back

Plugins that add support for hardware many users have should be
pull-requested into `crates/linsight-sensors/`. The structure of
existing in-tree plugins is the template; CI runs the same clippy +
test gates against your PR.
