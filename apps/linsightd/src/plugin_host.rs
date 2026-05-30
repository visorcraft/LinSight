// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;
use linsight_core::{HardwareDevice, Reading, Sample, SensorId};
use linsight_plugin_sdk::{
    LINSIGHT_PLUGIN_ABI_VERSION, LinsightPlugin, LinsightPluginDyn, PluginCtx, PluginError,
    PluginManifest, SensorDescriptor, host_init, host_sample,
};
use linsight_sensors_amdgpu::AmdgpuPlugin;
use linsight_sensors_containers::ContainersPlugin;
use linsight_sensors_cpu::CpuPlugin;
use linsight_sensors_disk::DiskPlugin;
use linsight_sensors_fs::FsPlugin;
use linsight_sensors_hwmon::HwmonPlugin;
use linsight_sensors_i915::I915Plugin;
use linsight_sensors_mem::MemPlugin;
use linsight_sensors_net::NetPlugin;
use linsight_sensors_nvme::NvmePlugin;
use linsight_sensors_nvml::NvmlPlugin;
use linsight_sensors_proc::ProcPlugin;
use linsight_sensors_sock::SockPlugin;
use linsight_sensors_system::SystemPlugin;
use linsight_sensors_systemd::SystemdPlugin;
use linsight_sensors_xe::XePlugin;
use linsight_sensors_zram::ZramPlugin;
use stabby::dynptr;
use stabby::libloading::StabbyLibrary;
use tracing::{info, warn};

/// A loaded plugin entry. Drop order matters: the `Arc<dyn LinsightPlugin>`
/// (and any data it owns) must drop before `library`, otherwise vtable
/// pointers become dangling. Rust drops struct fields in declaration order,
/// so `plugin` is listed FIRST.
///
/// Safety invariant: do NOT clone the `plugin` Arc outside this struct.
/// Field-order-based drop only guarantees the vtable outlives `library` if
/// no other `Arc<dyn LinsightPlugin>` holds a reference after `PluginHost`
/// is dropped. The `Arc` is used (instead of `Box`) so the scheduler/tick
/// path can borrow read-only without `&mut`-aliasing the host; it is never
/// cloned out.
struct PluginEntry {
    plugin: Arc<dyn LinsightPlugin>,
    /// `None` for in-tree statically-linked plugins, `Some` for dynamically
    /// loaded `.so` plugins. RAII guard: held purely to keep the dynamic
    /// library mapped for the lifetime of the entry — dropping it
    /// `dlclose`s the `.so` and invalidates every vtable pointer borrowed
    /// from it. The field is intentionally write-only; the `#[allow]`
    /// suppresses clippy noise about that, NOT a deferred caller.
    #[allow(dead_code)]
    library: Option<Library>,
    meta: PluginMeta,
    sensor_count: u32,
    /// ABI v4 hardware devices declared on this plugin's manifest. The
    /// host fills in `plugin_id` on these later (HardwareRegistry::build);
    /// plugins themselves leave it empty.
    devices: Vec<HardwareDevice>,
    /// Per-plugin sensor descriptors. This deliberately duplicates the
    /// values stored in `self.registry` (`HashMap<SensorId, (idx, desc)>`):
    /// the registry stays as a fast `SensorId -> owner+descriptor` lookup
    /// for the scheduler hot path, while this Vec lets us hand
    /// `HardwareRegistry::build` a per-plugin slice without sorting and
    /// regrouping the global map on every iteration. The duplication is a
    /// few KB at most for the worst-case sensor count we ship.
    sensors: Vec<SensorDescriptor>,
}

/// Lightweight plugin identity that the daemon ships to clients in
/// `ServerMsg::Welcome`. Derived from `PluginManifest` at registration time
/// so we don't have to re-call `init()` on every Welcome.
#[derive(Clone, Debug)]
pub struct PluginMeta {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
}

pub struct PluginHost {
    plugins: Vec<PluginEntry>,
    /// Maps every sensor id to (plugin index, descriptor).
    registry: HashMap<SensorId, (usize, SensorDescriptor)>,
}

impl PluginHost {
    #[allow(dead_code)]
    pub fn with_builtins() -> Self {
        Self::with_builtins_and_config(&HashMap::new())
    }

    pub fn with_builtins_and_config(plugin_configs: &HashMap<String, serde_json::Value>) -> Self {
        let mut host = Self { plugins: Vec::new(), registry: HashMap::new() };
        let cfg = |id: &str| plugin_configs.get(id).cloned().unwrap_or(serde_json::Value::Null);
        host.register_with_config(
            Arc::new(CpuPlugin::default()),
            None,
            cfg("linsight-sensors-cpu"),
        );
        host.register_with_config(
            Arc::new(AmdgpuPlugin::default()),
            None,
            cfg("linsight-sensors-amdgpu"),
        );
        host.register_with_config(
            Arc::new(MemPlugin::default()),
            None,
            cfg("linsight-sensors-mem"),
        );
        host.register_with_config(Arc::new(XePlugin::default()), None, cfg("linsight-sensors-xe"));
        host.register_with_config(
            Arc::new(SystemPlugin::default()),
            None,
            cfg("linsight-sensors-system"),
        );
        host.register_with_config(
            Arc::new(DiskPlugin::default()),
            None,
            cfg("linsight-sensors-disk"),
        );
        host.register_with_config(
            Arc::new(HwmonPlugin::default()),
            None,
            cfg("linsight-sensors-hwmon"),
        );
        host.register_with_config(Arc::new(FsPlugin::default()), None, cfg("linsight-sensors-fs"));
        host.register_with_config(
            Arc::new(NvmlPlugin::default()),
            None,
            cfg("linsight-sensors-nvml"),
        );
        host.register_with_config(
            Arc::new(NvmePlugin::default()),
            None,
            cfg("linsight-sensors-nvme"),
        );
        host.register_with_config(
            Arc::new(NetPlugin::default()),
            None,
            cfg("linsight-sensors-net"),
        );
        host.register_with_config(
            Arc::new(ProcPlugin::default()),
            None,
            cfg("linsight-sensors-proc"),
        );
        host.register_with_config(
            Arc::new(ZramPlugin::default()),
            None,
            cfg("linsight-sensors-zram"),
        );
        host.register_with_config(
            Arc::new(I915Plugin::default()),
            None,
            cfg("linsight-sensors-i915"),
        );
        host.register_with_config(
            Arc::new(SystemdPlugin::default()),
            None,
            cfg("linsight-sensors-systemd"),
        );
        host.register_with_config(
            Arc::new(SockPlugin::default()),
            None,
            cfg("linsight-sensors-sock"),
        );
        host.register_with_config(
            Arc::new(ContainersPlugin::default()),
            None,
            cfg("linsight-sensors-containers"),
        );
        host
    }

    /// Scan the standard plugin directories and load every `.so` whose
    /// reported ABI version matches `LINSIGHT_PLUGIN_ABI_VERSION`. Errors
    /// are logged per-file; the daemon keeps running even if every plugin
    /// fails to load.
    pub fn load_dynamic_plugins(&mut self, plugin_configs: &HashMap<String, serde_json::Value>) {
        for dir in plugin_dirs() {
            self.load_from_dir(&dir, plugin_configs);
        }
    }

    fn load_from_dir(&mut self, dir: &Path, plugin_configs: &HashMap<String, serde_json::Value>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("so") {
                continue;
            }
            let LoadedPlugin { plugin, library } = match unsafe { load_one(&path) } {
                Ok(p) => p,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "plugin load failed; skipping");
                    continue;
                }
            };
            info!(path = %path.display(), "loaded plugin");

            // A plugin's id lives in its manifest, which is only known
            // after `init` runs — but `init` is also what consumes the
            // per-plugin config. Resolve the ordering problem by running a
            // throwaway "probe" `init` with an empty context to read the
            // id, then look up the config keyed by that id.
            let probe_manifest = match host_init(plugin.as_ref(), &PluginCtx::default()) {
                Ok(m) => m,
                Err(e) => {
                    warn!(path = %path.display(), error = ?e, "plugin init failed; skipping");
                    continue;
                }
            };
            let config = plugin_configs.get(&probe_manifest.plugin_id).cloned();

            match config {
                // No per-plugin config: keep the already-initialized probe
                // instance as-is — re-running `init` with the same empty
                // config would only add work.
                None | Some(serde_json::Value::Null) => {
                    self.finish_register(plugin, Some(library), probe_manifest);
                }
                // Config present: discard the probe (releasing whatever its
                // `init` acquired) and build a fresh instance from the same
                // library so `init` runs exactly once, this time with the
                // looked-up config. Using a new instance means no live
                // plugin is ever double-initialized.
                Some(config) => {
                    plugin.shutdown();
                    drop(plugin);
                    match unsafe { instantiate(&library) } {
                        Ok(configured) => {
                            self.register_with_config(configured, Some(library), config);
                        }
                        Err(e) => {
                            warn!(path = %path.display(), error = %e,
                                "plugin re-instantiation failed; skipping");
                        }
                    }
                }
            }
        }
    }

    fn register_with_config(
        &mut self,
        plugin: Arc<dyn LinsightPlugin>,
        library: Option<Library>,
        config: serde_json::Value,
    ) {
        let ctx = PluginCtx::default().with_config(config);
        let manifest = match host_init(plugin.as_ref(), &ctx) {
            Ok(m) => m,
            Err(e) => {
                warn!(error = ?e, "plugin init failed; skipping");
                return;
            }
        };
        self.finish_register(plugin, library, manifest);
    }

    /// Register an already-initialized plugin together with the manifest
    /// its `init` returned. Split out of [`register_with_config`] so the
    /// dynamic loader can register a plugin it has already `init`-ed (to
    /// discover its id) without paying for a second `init`.
    fn finish_register(
        &mut self,
        plugin: Arc<dyn LinsightPlugin>,
        library: Option<Library>,
        manifest: PluginManifest,
    ) {
        let idx = self.plugins.len();
        let meta = PluginMeta {
            plugin_id: manifest.plugin_id.clone(),
            display_name: manifest.display_name.clone(),
            version: manifest.version.clone(),
        };
        // Stash devices BEFORE the sensor loop consumes the manifest by
        // move. The HardwareRegistry will fill in `plugin_id` on these at
        // build time; plugins leave that field empty.
        let devices = manifest.devices;
        let mut accepted: u32 = 0;
        let mut sensors: Vec<SensorDescriptor> = Vec::with_capacity(manifest.sensors.len());
        for desc in manifest.sensors {
            if self.registry.contains_key(&desc.id) {
                warn!(sensor = %desc.id, "duplicate sensor id, first registration wins");
                continue;
            }
            sensors.push(desc.clone());
            self.registry.insert(desc.id.clone(), (idx, desc));
            accepted += 1;
        }
        self.plugins.push(PluginEntry {
            plugin,
            library,
            meta,
            sensor_count: accepted,
            devices,
            sensors,
        });
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &SensorDescriptor> {
        self.registry.values().map(|(_, d)| d)
    }

    /// Iterate over every successfully-loaded plugin's identity. Used by the
    /// transport layer to populate `ServerMsg::Welcome.plugins`.
    pub fn plugins(&self) -> impl Iterator<Item = (&PluginMeta, u32)> {
        self.plugins.iter().map(|e| (&e.meta, e.sensor_count))
    }

    /// Look up the plugin that owns `id`. Returns `None` for unknown sensors.
    pub fn plugin_id_for(&self, id: &SensorId) -> Option<&str> {
        let (idx, _) = self.registry.get(id)?;
        Some(self.plugins[*idx].meta.plugin_id.as_str())
    }

    /// Per-plugin manifest view used by [`crate::hardware::HardwareRegistry::build`]
    /// at startup. Yields one tuple per loaded plugin:
    /// `(plugin_id, &devices, &sensor_descriptors)`. The slices are
    /// borrowed straight out of the [`PluginEntry`] vectors — no
    /// per-call allocation, no re-grouping pass over the global
    /// `self.registry` HashMap.
    pub fn devices_by_plugin(
        &self,
    ) -> impl Iterator<Item = (&str, &[HardwareDevice], &[SensorDescriptor])> {
        self.plugins
            .iter()
            .map(|e| (e.meta.plugin_id.as_str(), e.devices.as_slice(), e.sensors.as_slice()))
    }

    pub fn sample(&self, id: &SensorId) -> Result<Reading, PluginError> {
        let (idx, _) =
            self.registry.get(id).ok_or_else(|| PluginError::Unsupported(id.to_string()))?;
        host_sample(self.plugins[*idx].plugin.as_ref(), id.clone())
    }

    pub fn sample_to(&self, id: &SensorId, ts_micros: u64) -> Result<Sample, PluginError> {
        let reading = self.sample(id)?;
        Ok(Sample { sensor: id.clone(), ts_micros, reading })
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        // Invoke each plugin's `shutdown` hook. The default impl is a
        // no-op, but plugins owning background threads or hardware
        // handles use it to release resources before their `Drop` runs.
        // Iteration order matches insertion order; reverse-iteration
        // would be more typical for resource teardown but only matters
        // when one plugin depends on another, which the SDK does not
        // currently model.
        for entry in &self.plugins {
            entry.plugin.shutdown();
        }
    }
}

struct LoadedPlugin {
    plugin: Arc<dyn LinsightPlugin>,
    library: Library,
}

/// The stabbified factory's return type. Must match what the SDK's
/// `export_plugin!` macro emits.
type PluginFactory =
    extern "C" fn() -> dynptr!(stabby::boxed::Box<dyn LinsightPlugin + Send + Sync>);

/// SAFETY: the caller asserts the file at `path` exports the
/// `linsight_plugin_abi_version` symbol and a `#[stabby::export]`-annotated
/// `linsight_plugin_v5` factory whose return type is
/// `dynptr!(Box<dyn LinsightPlugin + Send + Sync>)`. Type compatibility is
/// verified via stabby's reflection (`StabbyLibrary::get_stabbied`).
unsafe fn load_one(path: &Path) -> Result<LoadedPlugin, String> {
    let library = unsafe { Library::new(path) }.map_err(|e| format!("dlopen: {e}"))?;
    let version_fn: libloading::Symbol<'_, unsafe extern "C" fn() -> u32> = unsafe {
        library
            .get(b"linsight_plugin_abi_version\0")
            .map_err(|e| format!("missing linsight_plugin_abi_version: {e}"))?
    };
    let version = unsafe { version_fn() };
    if version != LINSIGHT_PLUGIN_ABI_VERSION {
        return Err(format!(
            "ABI mismatch: plugin reports v{version}, daemon expects v{LINSIGHT_PLUGIN_ABI_VERSION}"
        ));
    }
    let plugin = unsafe { instantiate(&library) }?;
    Ok(LoadedPlugin { plugin, library })
}

/// Build a plugin instance from an already-opened, version-checked
/// library by invoking its stabby factory. Separated from [`load_one`] so
/// the dynamic loader can construct a second instance from the same `.so`
/// (to re-`init` with per-plugin config) without re-opening the library.
///
/// SAFETY: `library` must export the `#[stabby::export]`-annotated
/// `linsight_plugin_v5` factory whose return type is
/// `dynptr!(Box<dyn LinsightPlugin + Send + Sync>)`. Type compatibility is
/// verified by stabby's reflection (`get_stabbied`).
unsafe fn instantiate(library: &Library) -> Result<Arc<dyn LinsightPlugin>, String> {
    let factory = unsafe {
        library
            .get_stabbied::<PluginFactory>(b"linsight_plugin_v5")
            .map_err(|e| format!("stabbied symbol load failed: {e}"))?
    };
    let dyn_box = factory();
    // The dynptr is `Dyn<'static, Box<()>, ...>` carrying our trait's
    // vtable. Wrap it in an `Arc<dyn LinsightPlugin>` by converting via a
    // trait object that boxes the dynptr.
    Ok(Arc::new(DynBoxPlugin(dyn_box)))
}

/// Adapter: a `dynptr!(Box<dyn LinsightPlugin + Send + Sync>)` exposes the
/// trait methods directly via stabby's `Deref`-style indirection, but to
/// store it behind a `dyn LinsightPlugin` of our own we wrap it in this
/// concrete type and re-implement the trait, forwarding every call.
struct DynBoxPlugin(dynptr!(stabby::boxed::Box<dyn LinsightPlugin + Send + Sync>));

// SAFETY: the inner dynptr's vtable is restricted to `LinsightPlugin + Send + Sync`,
// so the wrapper inherits those bounds.
unsafe impl Send for DynBoxPlugin {}
unsafe impl Sync for DynBoxPlugin {}

impl LinsightPlugin for DynBoxPlugin {
    extern "C" fn init(
        &self,
        ctx: &linsight_plugin_sdk::RPluginCtx,
    ) -> linsight_plugin_sdk::RInitResult {
        self.0.init(ctx)
    }

    extern "C" fn sample(
        &self,
        sensor: linsight_plugin_sdk::RSensorId,
    ) -> linsight_plugin_sdk::RSampleResult {
        self.0.sample(sensor)
    }

    extern "C" fn shutdown(&self) {
        self.0.shutdown()
    }
}

fn plugin_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    out.push(PathBuf::from("/usr/lib/linsight/plugins"));
    out.push(PathBuf::from("/usr/local/lib/linsight/plugins"));
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        out.push(PathBuf::from(xdg).join("linsight/plugins"));
    } else if let Some(home) = std::env::var_os("HOME") {
        out.push(PathBuf::from(home).join(".local/share/linsight/plugins"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use linsight_core::SensorId;

    use super::*;

    /// Build `examples/echo-plugin` once via escargot and return its `.so`.
    fn echo_plugin_so() -> PathBuf {
        static PATH: OnceLock<PathBuf> = OnceLock::new();
        PATH.get_or_init(|| {
            let build = escargot::CargoBuild::new()
                .package("linsight-example-echo-plugin")
                .exec()
                .expect("cargo build the echo example plugin");
            for msg in build {
                let msg = msg.expect("read cargo build message");
                if let escargot::format::Message::CompilerArtifact(art) =
                    msg.decode().expect("decode cargo message")
                    && art.target.name == "linsight_example_echo_plugin"
                {
                    for path in art.filenames {
                        let p: PathBuf = path.into_owned();
                        if p.extension().and_then(|s| s.to_str()) == Some("so") {
                            return p;
                        }
                    }
                }
            }
            panic!("escargot produced no .so for the echo plugin");
        })
        .clone()
    }

    /// Stage the built echo `.so` into a fresh temp plugin directory.
    fn staged_plugin_dir() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::copy(echo_plugin_so(), dir.path().join("libecho.so")).unwrap();
        dir
    }

    fn empty_host() -> PluginHost {
        PluginHost { plugins: Vec::new(), registry: HashMap::new() }
    }

    #[test]
    fn dynamic_plugin_without_config_has_base_sensors_only() {
        let dir = staged_plugin_dir();
        let mut host = empty_host();
        host.load_from_dir(dir.path(), &HashMap::new());
        let ids: Vec<&str> = host.descriptors().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"example.echo.value"), "base sensor missing: {ids:?}");
        assert!(
            !ids.contains(&"example.echo.extra"),
            "config-gated sensor must be absent without config: {ids:?}",
        );
    }

    #[test]
    fn dynamic_plugin_receives_per_plugin_config() {
        let dir = staged_plugin_dir();
        let mut host = empty_host();
        let mut configs: HashMap<String, serde_json::Value> = HashMap::new();
        configs.insert(
            "io.visorcraft.linsight.example.echo".to_string(),
            serde_json::json!({ "enable_extra": true }),
        );
        host.load_from_dir(dir.path(), &configs);
        let ids: Vec<&str> = host.descriptors().map(|d| d.id.as_str()).collect();
        assert!(
            ids.contains(&"example.echo.extra"),
            "dynamically-loaded plugin did not receive its per-plugin config; sensors = {ids:?}",
        );
    }

    #[test]
    fn with_builtins_registers_cpu() {
        let host = PluginHost::with_builtins();
        let ids: Vec<_> = host.descriptors().map(|d| d.id.clone()).collect();
        assert!(ids.iter().any(|s| s.as_str() == "cpu.util"));
    }

    #[test]
    fn sample_routes_to_owning_plugin() {
        let host = PluginHost::with_builtins();
        let id = SensorId::new("cpu.util");
        let _first = host.sample(&id).unwrap();
        let _second = host.sample(&id).unwrap();
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let host = PluginHost::with_builtins();
        let err = host.sample(&SensorId::new("nope.nope")).unwrap_err();
        assert!(err.to_string().contains("nope.nope"));
    }

    #[test]
    fn with_builtins_and_config_passes_config_to_plugins() {
        let mut configs = HashMap::new();
        configs.insert(
            "linsight-sensors-net".into(),
            serde_json::json!({"exclude_interfaces": ["docker*"]}),
        );
        let host = PluginHost::with_builtins_and_config(&configs);
        let ids: Vec<_> = host.descriptors().map(|d| d.id.clone()).collect();
        assert!(ids.iter().any(|s| s.as_str().starts_with("cpu.")));
    }
}
