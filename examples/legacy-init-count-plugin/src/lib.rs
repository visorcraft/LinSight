// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![deny(rust_2018_idioms)]

use std::sync::atomic::{AtomicUsize, Ordering};

use linsight_plugin_sdk::linsight_core::{Category, Reading, SensorId, SensorKind, Unit};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor, export_plugin,
};

static INIT_COUNT: AtomicUsize = AtomicUsize::new(0);

const PLUGIN_ID: &str = "com.visorcraft.linsight.example.legacy-init-count";

#[derive(Default)]
pub struct LegacyInitCountPlugin;

impl LegacyInitCountPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        INIT_COUNT.fetch_add(1, Ordering::SeqCst);
        let mut sensors = vec![SensorDescriptor {
            id: SensorId::new("example.legacy_init_count.value"),
            display_name: "Legacy init count".into(),
            unit: Unit::Count,
            kind: SensorKind::Scalar,
            category: Category::Custom,
            native_rate_hz: 1.0,
            min: None,
            max: None,
            device_id: None,
            device_key: None,
            tags: vec![],
        }];
        if ctx.config().get("enable_extra").and_then(|v| v.as_bool()).unwrap_or(false) {
            sensors.push(SensorDescriptor {
                id: SensorId::new("example.legacy_init_count.extra"),
                display_name: "Legacy config extra".into(),
                unit: Unit::Count,
                kind: SensorKind::Scalar,
                category: Category::Custom,
                native_rate_hz: 1.0,
                min: None,
                max: None,
                device_id: None,
                device_key: None,
                tags: vec![],
            });
        }
        Ok(PluginManifest {
            plugin_id: PLUGIN_ID.into(),
            display_name: "Legacy Init Count".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices: vec![],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        match sensor.as_str() {
            "example.legacy_init_count.value" => {
                Ok(Reading::Scalar(INIT_COUNT.load(Ordering::SeqCst) as f64))
            }
            "example.legacy_init_count.extra" => Ok(Reading::Scalar(1.0)),
            _ => Err(PluginError::Unsupported(sensor.to_string())),
        }
    }
}

impl LinsightPlugin for LegacyInitCountPlugin {
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

export_plugin!(LegacyInitCountPlugin);
