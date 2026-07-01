// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! `linsight-cli plugin {new,install,ls,remove}` subcommands.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Path to the user-level plugin directory. Returns an error rather than
/// falling back to a CWD-relative path when neither `XDG_DATA_HOME` nor
/// `HOME` is set — that fallback let `plugin install` and `plugin ls`
/// silently operate on different directories than where the daemon actually
/// reads (especially in root shells or in containers without `HOME`).
fn user_plugin_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(d).join("linsight/plugins"));
    }
    if let Some(h) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(h).join(".local/share/linsight/plugins"));
    }
    anyhow::bail!(
        "neither $XDG_DATA_HOME nor $HOME is set; cannot determine the user plugin directory. \
         Set one of them explicitly, or pass `--plugin-dir <path>` (if your daemon was built with that flag).",
    )
}

pub fn new(name: &str) -> Result<()> {
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        anyhow::bail!("invalid plugin name: {name} (alphanumeric / `-` / `_` only)");
    }
    let root = PathBuf::from(name);
    if root.exists() {
        anyhow::bail!("directory already exists: {}", root.display());
    }
    std::fs::create_dir_all(root.join("src"))
        .with_context(|| format!("mkdir {}", root.join("src").display()))?;
    let cargo_toml = root.join("Cargo.toml");
    let sdk_version = env!("CARGO_PKG_VERSION");
    std::fs::write(
        &cargo_toml,
        format!(
            r#"# SPDX-FileCopyrightText: 2026 ${{your name}}
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
license = "GPL-3.0-only"

[lib]
crate-type = ["cdylib"]

[dependencies]
linsight-plugin-sdk = "{sdk_version}"

# Direct stabby dep is required because the `export_plugin!` macro
# expands `#[stabby::export]` at the plugin crate's call site, and
# stabby's proc-macros locate the crate via the plugin's Cargo.toml.
stabby = "72"
"#,
        ),
    )
    .with_context(|| format!("write {}", cargo_toml.display()))?;
    let lib_rs = root.join("src/lib.rs");
    std::fs::write(
        &lib_rs,
        r#"// SPDX-FileCopyrightText: 2026 ${your name}
// SPDX-License-Identifier: GPL-3.0-only

use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx,
    RPluginError, RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
    export_plugin,
};
use linsight_plugin_sdk::linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId,
    SensorKind, Unit,
};
use stabby::result::Result as SResult;

#[derive(Default)]
pub struct MyPlugin;

impl MyPlugin {
    fn init_inner(&self, _ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        // Every sensor belongs to a HardwareDevice. The host validates that
        // each `SensorDescriptor.device_key` resolves to an entry in
        // `PluginManifest.devices`, so populate `devices` before stamping
        // keys onto sensors. The `plugin:` scheme is reserved for keys
        // owned by a single plugin; substitute your real plugin id below.
        let key = HardwareDeviceKey::try_new("plugin:io.example.myplugin:demo").unwrap();
        Ok(PluginManifest {
            plugin_id: "io.example.myplugin".into(),
            display_name: "My plugin".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors: vec![SensorDescriptor {
                id: SensorId::new("example.hello"),
                display_name: "Hello sensor".into(),
                unit: Unit::Count,
                kind: SensorKind::Scalar,
                category: Category::Custom,
                native_rate_hz: 1.0,
                min: None,
                max: None,
                device_id: Some("demo".into()),
                device_key: Some(key.clone()),
                tags: vec![],
            }],
            devices: vec![HardwareDevice {
                key,
                category: HardwareCategory::Other,
                model: "My plugin demo device".into(),
                vendor: None,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: "demo".into(),
                sensor_ids: vec![],
            }],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        if sensor.as_str() == "example.hello" {
            Ok(Reading::Scalar(42.0))
        } else {
            Err(PluginError::Unsupported(sensor.to_string()))
        }
    }
}

impl LinsightPlugin for MyPlugin {
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

export_plugin!(
    MyPlugin,
    metadata: {
        plugin_id: "io.example.myplugin",
        display_name: "My plugin",
        version: env!("CARGO_PKG_VERSION"),
    }
);
"#,
    )
    .with_context(|| format!("write {}", lib_rs.display()))?;
    let readme = root.join("README.md");
    std::fs::write(
        &readme,
        format!(
            "# {name}\n\nA LinSight plugin.\n\n## Build\n\n\
             ```\ncargo build --release\n```\n\n## Install\n\n\
             ```\nlinsight-cli plugin install target/release/lib{}.so\n```\n",
            name.replace('-', "_")
        ),
    )
    .with_context(|| format!("write {}", readme.display()))?;
    println!("Scaffolded plugin crate at {}/", root.display());
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  cargo build --release");
    println!("  linsight-cli plugin install target/release/lib{}.so", name.replace('-', "_"));
    Ok(())
}

pub fn install(path: &Path) -> Result<()> {
    if path.extension().and_then(|e| e.to_str()) != Some("so") {
        anyhow::bail!("not a .so file: {}", path.display());
    }
    let dest_dir = user_plugin_dir()?;
    std::fs::create_dir_all(&dest_dir).with_context(|| format!("mkdir {}", dest_dir.display()))?;
    let filename = path.file_name().ok_or_else(|| anyhow::anyhow!("source has no filename"))?;
    let dest = dest_dir.join(filename);
    std::fs::copy(path, &dest)
        .with_context(|| format!("copy {} -> {}", path.display(), dest.display()))?;
    println!("Installed {}", dest.display());
    println!("Restart linsightd for the plugin to load.");
    Ok(())
}

pub fn ls() -> Result<()> {
    let dir = user_plugin_dir()?;
    if !dir.exists() {
        println!("No user plugins installed (directory {} doesn't exist).", dir.display());
        return Ok(());
    }
    // Read entries explicitly so per-entry errors (permission denied on a
    // symlink target, transient FS state) are surfaced rather than silently
    // dropped by `.flatten()`. A partial listing with a warning beats a
    // partial listing with no signal at all.
    let mut entries: Vec<std::fs::DirEntry> = Vec::new();
    let mut hidden_errors = 0u32;
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))? {
        match entry {
            Ok(e) if e.path().extension().and_then(|s| s.to_str()) == Some("so") => entries.push(e),
            Ok(_) => {}
            Err(e) => {
                hidden_errors += 1;
                eprintln!("warning: skipping directory entry: {e}");
            }
        }
    }
    if entries.is_empty() {
        println!("No user plugins installed in {}.", dir.display());
    } else {
        for entry in &entries {
            let path = entry.path();
            match std::fs::metadata(&path) {
                Ok(m) => println!("{}\t{} bytes", path.display(), m.len()),
                Err(e) => eprintln!("warning: stat {}: {e}", path.display()),
            }
        }
    }
    if hidden_errors > 0 {
        eprintln!("({hidden_errors} entries skipped due to errors)");
    }
    Ok(())
}

pub fn remove(name: &str) -> Result<()> {
    if name.contains('/') || name.contains("..") {
        anyhow::bail!("invalid plugin name {name:?}: must not contain '/' or '..'");
    }
    let dir = user_plugin_dir()?;
    // Accept either bare name (mylib) or filename (libmylib.so).
    let candidates =
        [dir.join(name), dir.join(format!("{name}.so")), dir.join(format!("lib{name}.so"))];
    for path in &candidates {
        if path.exists() {
            std::fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
            println!("Removed {}", path.display());
            println!("Restart linsightd for the change to take effect.");
            return Ok(());
        }
    }
    anyhow::bail!("no plugin named {name} in {}", dir.display())
}
