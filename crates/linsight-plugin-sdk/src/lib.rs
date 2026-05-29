// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![deny(rust_2018_idioms)]
#![deny(unsafe_op_in_unsafe_fn)]
// `#[stabby::stabby]` on a trait expands to vtable-shim functions that
// fold the trait's `&self` plus every parameter into one extern "C" call.
// For traits with three parameters that's already 4-5 arguments, and
// stabby adds vtable / metadata fields on top. The lint isn't actionable
// at our level — the shape is mechanically determined by the macro.
#![allow(clippy::too_many_arguments)]

pub mod export;
pub mod manifest;
pub mod mirror;
pub mod pciids;
pub mod plugin;

pub use manifest::*;
pub use mirror::*;
pub use plugin::*;

/// Re-export of `linsight-core` so plugin authors can pin against the SDK
/// version alone. The types every plugin needs (`SensorId`, `Reading`,
/// `Unit`, `SensorKind`, `Category`, etc.) live there.
pub use linsight_core;

/// Re-export of `stabby` so plugin authors can pull in stabby types
/// without adding a duplicate dependency.
pub use stabby;

/// Bump only on breaking changes to the plugin ABI. The daemon refuses
/// to load plugins whose returned abi version does not match this
/// constant.
///
/// * v1: pre-stabby raw-fat-pointer factory; `Box::from_raw` on the host.
/// * v2: `#[stabby::stabby]` trait, R-mirror types on the FFI boundary,
///   factory returns `stabby::dynptr!(Box<dyn LinsightPlugin + Send + Sync>)`
///   and is loaded via `StabbyLibrary::get_stabbied`. Used
///   `#[repr(stabby)]` tagged enums for RUnit / RReading / RCell.
/// * v3: same factory shape as v2, but every former tagged enum
///   (RUnit, RReading, RCell) replaced with an explicit
///   `(kind: <Repr>Kind, payload_fields)` struct. The change works
///   around a stabby 36.2.2 bug where `match_owned` on tagged enums
///   with mixed unit + payload variants misroutes closures at
///   `opt-level >= 1`. v2 plugins fail the version check at load.
/// * v4: PluginManifest gains `devices: Vec<HardwareDevice>` and
///   SensorDescriptor gains `device_key: Option<HardwareDeviceKey>`
///   so each plugin reports its hardware identities for the
///   daemon's Hardware page + nickname store. v3 plugins fail the
///   symbol lookup at load (`linsight_plugin_v3` → `_v4`).
/// * v5: `RPluginCtx` gains `config_json: SString` — a JSON-encoded
///   per-plugin config blob read from `plugins.toml` by the daemon.
///   Plugins opt in by deserializing it. v4 plugins fail the symbol
///   lookup at load (`linsight_plugin_v4` → `_v5`).
pub const LINSIGHT_PLUGIN_ABI_VERSION: u32 = 5;

/// Re-export of [`linsight_core::STATIC_TAG`] — the canonical sensor tag
/// marking a value as constant for the process lifetime. Defined in
/// linsight-core so the daemon, plugins, SDK, and GUI all share one source
/// of truth.
pub use linsight_core::STATIC_TAG;
