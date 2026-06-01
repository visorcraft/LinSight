// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

/// Define the entry points an out-of-tree plugin's `cdylib` must export.
///
/// Pass the type that implements [`LinsightPlugin`](crate::LinsightPlugin).
/// The type must implement `Default`.
///
/// ABI v6: the factory returns a stabby dyn-ptr
/// (`stabby::dynptr!(Box<dyn LinsightPlugin + Send + Sync>)`) annotated
/// with `#[stabby::export]`. The host loads the symbol via
/// [`StabbyLibrary::get_stabbied`](stabby::libloading::StabbyLibrary::get_stabbied)
/// so the entire signature is validated by stabby's reflection report.
///
/// A plain `extern "C" fn() -> u32` named `linsight_plugin_abi_version`
/// is emitted alongside so loaders can do a cheap version-compatibility
/// short-circuit before paying the stabby cost. The factory symbol is
/// `linsight_plugin_v6` — renamed from `linsight_plugin_v5` so a v5
/// plugin's `.so` will fail symbol lookup at load time rather than
/// silently exchanging vtables whose methods differ in unwind ABI (v6
/// switches the trait methods to `extern "C-unwind"` for panic isolation).
#[macro_export]
macro_rules! export_plugin {
    ($ty:ty) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn linsight_plugin_abi_version() -> u32 {
            $crate::LINSIGHT_PLUGIN_ABI_VERSION
        }

        #[$crate::stabby::export]
        pub extern "C" fn linsight_plugin_v6() -> $crate::stabby::dynptr!(
            $crate::stabby::boxed::Box<dyn $crate::LinsightPlugin + Send + Sync>
        ) {
            let boxed: $crate::stabby::boxed::Box<$ty> =
                $crate::stabby::boxed::Box::new(<$ty as Default>::default());
            boxed.into()
        }
    };
}

#[cfg(test)]
mod tests {
    use linsight_core::SensorId;
    use stabby::result::Result as SResult;

    use crate::{
        LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx,
        RPluginError, RPluginManifest, RReading, RSampleResult, RSensorId, host_init, host_sample,
    };

    #[derive(Default)]
    struct EchoPlugin;

    impl LinsightPlugin for EchoPlugin {
        extern "C-unwind" fn init(&self, _ctx: &RPluginCtx) -> RInitResult {
            let m = PluginManifest {
                plugin_id: "echo".into(),
                display_name: "Echo".into(),
                version: "0.0.1".into(),
                sensors: vec![],
                devices: vec![],
            };
            let r: RPluginManifest = m.into();
            SResult::Ok(r)
        }

        extern "C-unwind" fn sample(&self, _: RSensorId) -> RSampleResult {
            let r: RReading = linsight_core::Reading::Scalar(1.0).into();
            SResult::Ok(r)
        }
    }

    // We don't `export_plugin!(EchoPlugin)` in cfg(test) inside the SDK
    // itself, because the macro emits `#[no_mangle]` symbols that would
    // collide with the SDK's own test harness when other crates link it.
    // The macro is exercised via the dynamic plugin in
    // `examples/echo-plugin` (not currently present) and via the
    // in-tree sensor crates which call it through their own cdylibs.

    #[test]
    fn host_init_round_trips() {
        let p = EchoPlugin;
        let m = host_init(&p, &PluginCtx::default()).unwrap();
        assert_eq!(m.plugin_id, "echo");
    }

    #[test]
    fn host_sample_round_trips() {
        let p = EchoPlugin;
        let r = host_sample(&p, SensorId::new("anything")).unwrap();
        assert!(matches!(r, linsight_core::Reading::Scalar(v) if v == 1.0));
        // Unused-by-this-test error type — keep the import live.
        let _ = std::any::TypeId::of::<PluginError>();
        let _ = std::any::TypeId::of::<RPluginError>();
    }
}
