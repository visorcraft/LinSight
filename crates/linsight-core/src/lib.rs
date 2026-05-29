// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

pub mod atomic_write;
pub mod dashboard;
pub mod error;
pub mod hardware;
pub mod types;

pub use atomic_write::atomic_write_json;

pub use error::{CoreError, CoreResult};
pub use hardware::{
    HardwareCategory, HardwareDevice, HardwareDeviceKey, KeyError, NICKNAME_MAX_CHARS,
    NicknameError, compute_device_label, parse_sysfs_pci_id, validate_nickname,
};
pub use types::*;

/// Sensor tag marking a value as effectively constant for the process
/// lifetime (e.g. total VRAM / RAM capacity). The daemon's scheduler
/// samples a `STATIC_TAG` sensor once per subscription instead of polling
/// it, and the GUI both omits its trend chart and renders it as a rounded
/// whole-GB capacity rather than a fractional binary size. Plugins opt in
/// by pushing this string into a sensor's `tags`.
pub const STATIC_TAG: &str = "static";
