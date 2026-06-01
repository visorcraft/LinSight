// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use linsight_core::{Reading, SensorId};
use stabby::result::Result as SResult;
use stabby::string::String as SString;
use thiserror::Error;

use crate::manifest::{PluginManifest, RPluginManifest};
use crate::mirror::{RReading, RSensorId};

// ---------------------------------------------------------------------------
// Host-facing error type.
// ---------------------------------------------------------------------------

#[derive(Debug, Error, Clone)]
pub enum PluginError {
    #[error("io: {0}")]
    Io(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("unsupported sensor: {0}")]
    Unsupported(String),
    #[error("transient: {0}")]
    Transient(String),
    /// ABI v4 host-side validation failure on a plugin's manifest. This
    /// variant has no corresponding `RPluginError` discriminant — the
    /// daemon synthesizes it during `host_init` after inspecting the
    /// raw R-mirror manifest. Plugins cannot produce it across the FFI
    /// boundary; cross-process error payloads from a plugin route
    /// through `Io` / `Parse` / `Unsupported` / `Transient` instead.
    #[error("manifest: {0}")]
    Manifest(String),
    /// Plugin code panicked during `init` or `sample`. Caught by
    /// `catch_unwind` so the daemon stays alive.
    #[error("plugin panicked: {0}")]
    Panic(String),
}

// ---------------------------------------------------------------------------
// R-mirror error type — what the stabbified trait returns across the FFI
// boundary. The first field tags the variant; the second carries the
// message.
// ---------------------------------------------------------------------------

#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum RPluginErrorKind {
    Io,
    Parse,
    Unsupported,
    Transient,
}

#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct RPluginError {
    pub kind: RPluginErrorKind,
    pub message: SString,
}

impl From<PluginError> for RPluginError {
    fn from(e: PluginError) -> Self {
        let (kind, message) = match e {
            PluginError::Io(s) => (RPluginErrorKind::Io, s),
            PluginError::Parse(s) => (RPluginErrorKind::Parse, s),
            PluginError::Unsupported(s) => (RPluginErrorKind::Unsupported, s),
            PluginError::Transient(s) => (RPluginErrorKind::Transient, s),
            // `Manifest` is host-side only — see the variant doc. If a
            // plugin somehow returns one (it shouldn't be able to: there
            // is no `RPluginErrorKind::Manifest`), route it through `Io`
            // with the message preserved, so the operator still sees
            // the diagnostic text.
            PluginError::Manifest(s) => (RPluginErrorKind::Io, format!("manifest: {s}")),
            PluginError::Panic(s) => (RPluginErrorKind::Io, format!("plugin panicked: {s}")),
        };
        Self { kind, message: message.as_str().into() }
    }
}

impl From<RPluginError> for PluginError {
    fn from(r: RPluginError) -> Self {
        let msg = r.message.as_str().to_owned();
        match r.kind {
            RPluginErrorKind::Io => PluginError::Io(msg),
            RPluginErrorKind::Parse => PluginError::Parse(msg),
            RPluginErrorKind::Unsupported => PluginError::Unsupported(msg),
            RPluginErrorKind::Transient => PluginError::Transient(msg),
        }
    }
}

// ---------------------------------------------------------------------------
// PluginCtx — read-only context passed to `init`. v2 carries an optional
// sysroot override (used by test harnesses to point a plugin at synthetic
// sysfs); future fields (logger handle, etc.) extend this struct.
//
// The sysroot crosses the FFI boundary as a `stabby::Option<SString>` so
// `None` is encoded structurally rather than through a paired bool. The
// path must be valid UTF-8: an OS path with non-UTF-8 bytes is rejected at
// the `PluginCtx::new_with_sysroot` constructor with an explicit error
// rather than being silently lossy. (Linux paths are arbitrary bytes, but
// LinSight's in-tree test fixtures all use ASCII tempdir paths, so this
// is a safe contract for plugin authors.)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct PluginCtx {
    sysroot: Option<std::path::PathBuf>,
    config: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginCtxError {
    #[error("sysroot path is not valid UTF-8: {0:?}")]
    NonUtf8Sysroot(std::path::PathBuf),
}

impl PluginCtx {
    pub fn new_with_sysroot(path: std::path::PathBuf) -> Result<Self, PluginCtxError> {
        if path.to_str().is_none() {
            return Err(PluginCtxError::NonUtf8Sysroot(path));
        }
        Ok(Self { sysroot: Some(path), config: serde_json::Value::Null })
    }

    pub fn sysroot(&self) -> Option<&std::path::Path> {
        self.sysroot.as_deref()
    }

    pub fn config(&self) -> &serde_json::Value {
        &self.config
    }

    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = config;
        self
    }
}

// `PluginCtx::default()` (derived) and the removed `new()` returned the
// same value (`sysroot: None`); keeping both was redundant. Callers
// that want the empty context should write `PluginCtx::default()`.

/// R-mirror context — the FFI form of [`PluginCtx`]. `sysroot` is
/// carried as an `SString` paired with a `sysroot_set: u8` bool flag
/// (kept from v2 — the v2→v3 ABI bump was driven by the stabby
/// release-mode `match_owned` bug on RUnit/RReading/RCell, not by
/// this struct). Migration to `stabby::Option<SString>` is tracked
/// in the open-followups doc for the next ABI revision.
#[stabby::stabby]
#[derive(Clone, Debug, Default)]
pub struct RPluginCtx {
    pub sysroot: SString,
    pub sysroot_set: u8,
    pub config_json: SString,
}

impl From<&PluginCtx> for RPluginCtx {
    fn from(ctx: &PluginCtx) -> Self {
        let sysroot = match &ctx.sysroot {
            Some(p) => {
                let s: SString = p.to_str().expect("PluginCtx invariant: sysroot is UTF-8").into();
                (s, 1u8)
            }
            None => (SString::default(), 0u8),
        };
        let config_json = match &ctx.config {
            serde_json::Value::Null => SString::default(),
            v => v.to_string().into(),
        };
        Self { sysroot: sysroot.0, sysroot_set: sysroot.1, config_json }
    }
}

impl From<&RPluginCtx> for PluginCtx {
    fn from(r: &RPluginCtx) -> Self {
        let sysroot = if r.sysroot_set != 0 {
            Some(std::path::PathBuf::from(r.sysroot.as_str()))
        } else {
            None
        };
        let config = if r.config_json.as_str().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(r.config_json.as_str()).unwrap_or(serde_json::Value::Null)
        };
        Self { sysroot, config }
    }
}

// ---------------------------------------------------------------------------
// The stabbified plugin trait.
//
// Every method uses `extern "C-unwind"` plus R-mirror types so the resulting
// vtable is FFI-safe across rustc minor versions. `C-unwind` (rather than
// plain `C`) lets a panic inside a plugin method unwind across the FFI
// boundary so the host's `catch_unwind` (in `host_init`/`host_sample`) can
// turn it into a `PluginError::Panic` instead of the plugin force-aborting
// the whole daemon at the boundary. (Panic isolation also requires the
// daemon to be built with `panic = "unwind"`; see the release profile.)
// Plugins typically construct linsight-core values internally and call
// `.into()` at the return.
// ---------------------------------------------------------------------------

pub type RInitResult = SResult<RPluginManifest, RPluginError>;
pub type RSampleResult = SResult<RReading, RPluginError>;

#[stabby::stabby]
pub trait LinsightPlugin: Send + Sync {
    extern "C-unwind" fn init(&self, ctx: &RPluginCtx) -> RInitResult;
    extern "C-unwind" fn sample(&self, sensor: RSensorId) -> RSampleResult;
    extern "C-unwind" fn shutdown(&self) {}
}

// ---------------------------------------------------------------------------
// Host-side convenience: call the trait with host-side types and get
// host-side types back. Plugin authors should not need this — they
// implement the trait directly.
// ---------------------------------------------------------------------------

/// Convenience wrapper: call `plugin.init(&ctx)` with a host-side
/// [`PluginCtx`] and get a host-side [`PluginManifest`] back.
///
/// Validates every sensor descriptor's ID via [`SensorId::try_new`] before
/// returning. The `From<RSensorId> for SensorId` conversion uses the
/// infallible `SensorId::new` (which only `debug_assert!`s on invariants);
/// without this check, a release-mode plugin returning an empty or
/// whitespace-bearing ID would silently corrupt the daemon's registry.
#[must_use = "host_init returns Result<PluginManifest, PluginError>; ignoring the manifest discards the plugin's sensor catalogue"]
pub fn host_init(
    plugin: &dyn LinsightPlugin,
    ctx: &PluginCtx,
) -> Result<PluginManifest, PluginError> {
    let rctx: RPluginCtx = ctx.into();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.init(&rctx)))
        .map_err(|_| PluginError::Panic("plugin init panicked".into()))?;
    let std_res: core::result::Result<RPluginManifest, RPluginError> = r.into();
    let r_manifest = std_res.map_err(PluginError::from)?;
    // Validate every sensor ID's raw FFI string BEFORE the From-conversion
    // calls the infallible (debug_assert!-only) `SensorId::new`. Doing this
    // on the std-typed manifest after conversion would already have hit
    // `debug_assert!` in debug builds, but in release builds the bad id
    // would have slipped silently into the registry. Walk the raw stabby
    // strings and run them through `try_new`.
    let plugin_id_for_err = r_manifest.plugin_id.as_str().to_owned();
    for i in 0..r_manifest.sensors.len() {
        // SVec doesn't expose `iter()` directly across all stabby
        // versions; index access via slice works.
        let raw = r_manifest.sensors.as_slice()[i].id.value.as_str();
        SensorId::try_new(raw).map_err(|e| {
            PluginError::Parse(format!(
                "plugin `{plugin_id_for_err}` returned invalid sensor id `{raw}`: {e}",
            ))
        })?;
    }
    // ABI v4: enforce manifest invariants on `devices` + `device_key`
    // BEFORE the From-conversion runs (the std-typed conversion calls
    // `HardwareDeviceKey::try_new(...).expect(...)` and would panic on
    // a malformed key — `validate_manifest` returns a structured error
    // instead).
    crate::manifest::validate_manifest(&r_manifest)?;
    Ok(r_manifest.into())
}

/// Convenience wrapper: call `plugin.sample(id)` with a host-side
/// [`SensorId`] and get a host-side [`Reading`] back.
#[must_use = "host_sample returns the sampled Reading; ignoring it drops the value the plugin produced"]
pub fn host_sample(plugin: &dyn LinsightPlugin, id: SensorId) -> Result<Reading, PluginError> {
    let rid: RSensorId = id.into();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plugin.sample(rid)))
        .map_err(|_| PluginError::Panic("plugin sample panicked".into()))?;
    let std_res: core::result::Result<RReading, RPluginError> = r.into();
    match std_res {
        Ok(r) => Ok(r.into()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use linsight_core::SensorId;
    use stabby::result::Result as SResult;

    use super::*;
    use crate::manifest::{PluginManifest, RPluginManifest};

    struct NoopPlugin;

    impl LinsightPlugin for NoopPlugin {
        extern "C-unwind" fn init(&self, _ctx: &RPluginCtx) -> RInitResult {
            let m = PluginManifest {
                plugin_id: "test".into(),
                display_name: "Test".into(),
                version: "0.0.1".into(),
                sensors: vec![],
                devices: vec![],
            };
            let r: RPluginManifest = m.into();
            SResult::Ok(r)
        }

        extern "C-unwind" fn sample(&self, _sensor: RSensorId) -> RSampleResult {
            let e: RPluginError = PluginError::Unsupported("no sensors".into()).into();
            SResult::Err(e)
        }
    }

    #[test]
    fn noop_plugin_init_runs() {
        let p = NoopPlugin;
        let m = host_init(&p, &PluginCtx::default()).unwrap();
        assert_eq!(m.plugin_id, "test");
    }

    #[test]
    fn noop_plugin_sample_errors() {
        let p = NoopPlugin;
        let err = host_sample(&p, SensorId::new("foo")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    /// Plugin that returns a manifest with an invalid sensor ID (interior
    /// whitespace). Used to exercise `host_init`'s validation pass.
    /// Without that pass, the bad ID would slip through the FFI mirror
    /// because `From<RSensorId> for SensorId` calls `SensorId::new`,
    /// which is `debug_assert!`-only in release builds.
    struct BadIdPlugin;

    impl LinsightPlugin for BadIdPlugin {
        extern "C-unwind" fn init(&self, _ctx: &RPluginCtx) -> RInitResult {
            // We deliberately bypass `SensorId::try_new` here to construct
            // a value that violates the invariant — this is what a
            // misbehaving plugin compiled in release mode could produce
            // through the FFI mirror.
            let bad: SString = "bad id".into(); // contains whitespace
            let r_desc = crate::manifest::RSensorDescriptor {
                id: RSensorId { value: bad },
                display_name: "Bad".into(),
                unit: crate::mirror::RUnit {
                    kind: crate::mirror::RUnitKind::Count,
                    custom: stabby::option::Option::None(),
                },
                kind: crate::mirror::RSensorKind::Scalar,
                category: crate::mirror::RCategory::Custom,
                native_rate_hz: 1.0,
                min: stabby::option::Option::None(),
                max: stabby::option::Option::None(),
                device_id: stabby::option::Option::None(),
                device_key: stabby::option::Option::None(),
                tags: stabby::vec::Vec::new(),
            };
            let mut sensors = stabby::vec::Vec::new();
            sensors.push(r_desc);
            let r = RPluginManifest {
                plugin_id: "test.bad".into(),
                display_name: "Bad".into(),
                version: "0.0.1".into(),
                sensors,
                devices: stabby::vec::Vec::new(),
            };
            SResult::Ok(r)
        }

        extern "C-unwind" fn sample(&self, _: RSensorId) -> RSampleResult {
            let e: RPluginError = PluginError::Unsupported("no".into()).into();
            SResult::Err(e)
        }
    }

    #[test]
    fn host_init_rejects_plugin_with_invalid_sensor_id() {
        // Regression guard for the FFI validation gap: a plugin returning
        // a manifest with a whitespace-bearing sensor ID must be rejected
        // by `host_init` (not silently accepted into the registry as it
        // would be by the debug_assert-only `SensorId::new`).
        let p = BadIdPlugin;
        let err = host_init(&p, &PluginCtx::default()).unwrap_err();
        match err {
            PluginError::Parse(msg) => {
                assert!(
                    msg.contains("bad id") || msg.contains("whitespace"),
                    "expected error to name the bad id or invariant; got: {msg}",
                );
            }
            other => panic!("expected PluginError::Parse, got {other:?}"),
        }
    }

    /// Plugin whose `init`/`sample` panic. With the `extern "C-unwind"`
    /// trait ABI, the panic unwinds across the boundary and `host_init` /
    /// `host_sample` catch it into `PluginError::Panic`. Under the old
    /// `extern "C"` ABI this would force-abort the process instead — so
    /// this is the regression guard for M3 (plugin panic isolation).
    struct PanicPlugin;

    impl LinsightPlugin for PanicPlugin {
        extern "C-unwind" fn init(&self, _ctx: &RPluginCtx) -> RInitResult {
            panic!("boom in init");
        }

        extern "C-unwind" fn sample(&self, _: RSensorId) -> RSampleResult {
            panic!("boom in sample");
        }
    }

    #[test]
    fn host_init_catches_plugin_panic() {
        let err = host_init(&PanicPlugin, &PluginCtx::default()).unwrap_err();
        assert!(matches!(err, PluginError::Panic(_)), "expected Panic, got {err:?}");
    }

    #[test]
    fn host_sample_catches_plugin_panic() {
        let err = host_sample(&PanicPlugin, SensorId::new("x")).unwrap_err();
        assert!(matches!(err, PluginError::Panic(_)), "expected Panic, got {err:?}");
    }

    #[test]
    fn plugin_ctx_rejects_non_utf8_sysroot() {
        // OsString built from raw bytes that are not valid UTF-8. On
        // Linux, PathBuf accepts these. The constructor must refuse them
        // so the FFI mirror's UTF-8 contract holds.
        use std::os::unix::ffi::OsStringExt;
        let bytes: Vec<u8> = vec![b'/', 0xff, 0xfe, b'/', b'x'];
        let bad = std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes));
        let err = PluginCtx::new_with_sysroot(bad).unwrap_err();
        assert!(matches!(err, PluginCtxError::NonUtf8Sysroot(_)));
    }
}
