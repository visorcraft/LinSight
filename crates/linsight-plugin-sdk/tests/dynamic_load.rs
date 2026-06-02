// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! End-to-end dynamic-load test for the `export_plugin!` macro.
//!
//! Builds `examples/echo-plugin` via escargot, dlopens the resulting
//! `.so`, runs the daemon-side reflection-checked loader (the same one
//! `linsightd::plugin_host::load_one` uses), and asserts:
//!
//!   * The `linsight_plugin_abi_version` symbol is present and returns
//!     [`LINSIGHT_PLUGIN_ABI_VERSION`].
//!   * `StabbyLibrary::get_stabbied` resolves the
//!     `linsight_plugin_v6` factory with a type signature that matches
//!     the SDK's expectation.
//!   * `host_init` succeeds and returns the expected `plugin_id`.
//!   * `host_sample` returns the expected `Reading::Scalar(42.0)`.
//!
//! Closes the "fabricated test claim" gap flagged by the 2026-05-25
//! peer review of commit 8c301d5: that commit's message stated a
//! scaffolded plugin was loaded via `get_stabbied`. No such test
//! existed. This is that test.

use std::path::PathBuf;
use std::sync::OnceLock;

use libloading::Library;
use linsight_core::{Reading, SensorId};
use linsight_plugin_sdk::stabby::dynptr;
use linsight_plugin_sdk::stabby::libloading::StabbyLibrary;
use linsight_plugin_sdk::{
    LINSIGHT_PLUGIN_ABI_VERSION, LinsightPlugin, LinsightPluginDyn, PluginCtx, host_init,
    host_sample,
};

type PluginFactory = extern "C" fn() -> dynptr!(
    linsight_plugin_sdk::stabby::boxed::Box<dyn LinsightPlugin + Send + Sync>
);

fn echo_plugin_so() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        // escargot already sets --message-format=json internally;
        // adding it ourselves causes a "two kinds of message-format"
        // error. Just package + exec_with_messages.
        let bin = escargot::CargoBuild::new()
            .package("linsight-example-echo-plugin")
            .exec()
            .expect("cargo build the example plugin");
        for msg in bin {
            let msg = msg.expect("read cargo build message");
            if let escargot::format::Message::CompilerArtifact(art) = msg.decode().expect("decode")
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
        panic!("escargot did not produce a .so for linsight-example-echo-plugin");
    })
}

#[test]
fn dynamic_load_exercises_abi_version_symbol() {
    let so = echo_plugin_so();
    let library = unsafe { Library::new(so) }.expect("dlopen example plugin");
    let version_fn: libloading::Symbol<'_, unsafe extern "C" fn() -> u32> =
        unsafe { library.get(b"linsight_plugin_abi_version\0") }
            .expect("plugin exports linsight_plugin_abi_version");
    let v = unsafe { version_fn() };
    assert_eq!(
        v, LINSIGHT_PLUGIN_ABI_VERSION,
        "plugin reports ABI v{v}; SDK expects v{LINSIGHT_PLUGIN_ABI_VERSION}"
    );
}

#[test]
fn dynamic_load_exercises_get_stabbied_factory() {
    let so = echo_plugin_so();
    let library = unsafe { Library::new(so) }.expect("dlopen example plugin");
    let factory = unsafe {
        library
            .get_stabbied::<PluginFactory>(b"linsight_plugin_v6")
            .expect("stabby reflection accepts the factory")
    };
    let _dyn_box = factory();
}

struct DynBoxPlugin(
    dynptr!(linsight_plugin_sdk::stabby::boxed::Box<dyn LinsightPlugin + Send + Sync>),
);

unsafe impl Send for DynBoxPlugin {}
unsafe impl Sync for DynBoxPlugin {}

impl LinsightPlugin for DynBoxPlugin {
    extern "C-unwind" fn init(
        &self,
        ctx: &linsight_plugin_sdk::RPluginCtx,
    ) -> linsight_plugin_sdk::RInitResult {
        self.0.init(ctx)
    }

    extern "C-unwind" fn sample(
        &self,
        sensor: linsight_plugin_sdk::RSensorId,
    ) -> linsight_plugin_sdk::RSampleResult {
        self.0.sample(sensor)
    }

    extern "C-unwind" fn shutdown(&self) {
        self.0.shutdown()
    }
}

#[test]
fn dynamic_load_init_and_sample_round_trip() {
    let so = echo_plugin_so();
    let library = unsafe { Library::new(so) }.expect("dlopen example plugin");
    let factory = unsafe {
        library.get_stabbied::<PluginFactory>(b"linsight_plugin_v6").expect("get_stabbied")
    };
    let dyn_box = factory();
    let plugin = DynBoxPlugin(dyn_box);

    let manifest = host_init(&plugin, &PluginCtx::default()).expect("host_init");
    assert_eq!(manifest.plugin_id, "com.visorcraft.linsight.example.echo");
    assert_eq!(manifest.sensors.len(), 1);
    assert_eq!(manifest.sensors[0].id.as_str(), "example.echo.value");
    assert_eq!(manifest.devices.len(), 1);
    assert_eq!(
        manifest.devices[0].key.as_str(),
        "plugin:com.visorcraft.linsight.example.echo:demo"
    );

    let reading = host_sample(&plugin, SensorId::new("example.echo.value")).expect("host_sample");
    match reading {
        Reading::Scalar(v) => assert_eq!(v, 42.0, "echo plugin must return its constant 42.0"),
        other => panic!("expected Reading::Scalar(42.0), got {other:?}"),
    }

    let unknown = host_sample(&plugin, SensorId::new("example.echo.absent"));
    assert!(unknown.is_err(), "unknown sensor should error");

    // CRITICAL: do NOT explicit-drop `library` here. Rust drops local
    // bindings in reverse declaration order, so `plugin` (and the
    // dynptr it owns) drops first — which calls destructors through
    // the .so vtable while it is still mapped — and `library` drops
    // last, unmapping the .so safely. Reversing this order (drop the
    // library before the dynptr's destructor runs) segfaults.
}

// `LinsightPluginDyn` is the auto-generated trait-object companion that
// `#[stabby::stabby]` emits next to `LinsightPlugin`. The `use` import
// at the top of the file is load-bearing — `get_stabbied` looks up the
// reflection slot through it. The `_` rebind below pins the symbol in
// scope so a future "tidy unused imports" pass doesn't silently break
// the dlopen path.
#[allow(dead_code, unused_imports)]
use LinsightPluginDyn as _;
