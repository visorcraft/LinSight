<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# linsight-plugin-sdk

Public SDK for authoring LinSight runtime sensor plugins.

Plugins are Rust `cdylib` crates that implement `LinsightPlugin` and export
the implementation with `export_plugin!`. LinSight's daemon loads the `.so`
with stabby reflection, checks `LINSIGHT_PLUGIN_ABI_VERSION`, validates every
sensor ID at the FFI boundary, and registers the plugin's sensors alongside
the built-in ones.

```toml
[dependencies]
linsight-plugin-sdk = "1.20.5"
stabby = "72"
```

The direct `stabby` dependency is required because `export_plugin!` expands
stabby proc-macros in the plugin crate.

For the full guide, see the LinSight repository's `docs/plugin-sdk.md`. For a
working reference plugin, see `examples/echo-plugin` in the same repository.
