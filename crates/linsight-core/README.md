<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# linsight-core

Shared pure-Rust types for LinSight.

This crate contains the value types used by the daemon, GUI, CLI, protocol,
and plugin SDK: sensor IDs, readings, samples, hardware-device metadata,
dashboard schema helpers, and small pure-logic utilities. It intentionally
does not own Linux I/O, async runtime setup, Qt bindings, or plugin loading.

Most plugin authors should depend on `linsight-plugin-sdk` and use the
`linsight_plugin_sdk::linsight_core` re-export instead of adding this crate
directly.
