// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![deny(rust_2018_idioms)]

//! Reference plugin used by `crates/linsight-plugin-sdk/tests/dynamic_load.rs`
//! to verify that the `export_plugin!` macro produces a `.so` that the
//! daemon can dlopen via `StabbyLibrary::get_stabbied`. Also serves as
//! the minimal end-to-end example for third-party plugin authors.
//!
//! Emits one sensor (`example.echo.value`) that returns a constant
//! scalar so the test can assert the round-trip without flakiness.

use linsight_plugin_sdk::linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor, export_plugin,
};

#[derive(Default)]
pub struct EchoPlugin;

impl EchoPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let key =
            HardwareDeviceKey::try_new("plugin:io.visorcraft.linsight.example.echo:demo").unwrap();
        let mut sensors = vec![SensorDescriptor {
            id: SensorId::new("example.echo.value"),
            display_name: "Echo value".into(),
            unit: Unit::Count,
            kind: SensorKind::Scalar,
            category: Category::Custom,
            native_rate_hz: 1.0,
            min: None,
            max: None,
            device_id: Some("demo".into()),
            device_key: Some(key.clone()),
            tags: vec![],
        }];
        // Per-plugin config demo: when the host passes
        // `{"enable_extra": true}` (via `plugins.toml` keyed by this
        // plugin's id) the plugin advertises a second sensor. Proves
        // that dynamically-loaded `.so` plugins receive their config.
        if ctx.config().get("enable_extra").and_then(|v| v.as_bool()).unwrap_or(false) {
            sensors.push(SensorDescriptor {
                id: SensorId::new("example.echo.extra"),
                display_name: "Echo extra (config-gated)".into(),
                unit: Unit::Count,
                kind: SensorKind::Scalar,
                category: Category::Custom,
                native_rate_hz: 1.0,
                min: None,
                max: None,
                device_id: Some("demo".into()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
        }
        Ok(PluginManifest {
            plugin_id: "io.visorcraft.linsight.example.echo".into(),
            display_name: "Echo example".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices: vec![HardwareDevice {
                key,
                category: HardwareCategory::Other,
                model: "Echo demo device".into(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: "demo".into(),
                sensor_ids: vec![],
            }],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        match sensor.as_str() {
            "example.echo.value" => Ok(Reading::Scalar(42.0)),
            "example.echo.extra" => Ok(Reading::Scalar(99.0)),
            _ => Err(PluginError::Unsupported(sensor.to_string())),
        }
    }
}

impl LinsightPlugin for EchoPlugin {
    extern "C-unwind" fn init(&self, ctx: &RPluginCtx) -> RInitResult {
        let host_ctx: PluginCtx = ctx.into();
        match self.init_inner(&host_ctx) {
            Ok(m) => SResult::Ok(<PluginManifest as Into<RPluginManifest>>::into(m)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }

    extern "C-unwind" fn sample(&self, sensor: RSensorId) -> RSampleResult {
        let id: SensorId = sensor.into();
        match self.sample_inner(id) {
            Ok(r) => SResult::Ok(<Reading as Into<RReading>>::into(r)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }
}

export_plugin!(EchoPlugin);
