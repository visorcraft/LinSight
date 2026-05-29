// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "tmpfs",
    "devtmpfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "cpuset",
    "efivarfs",
    "hugetlbfs",
    "mqueue",
    "pstore",
    "ramfs",
    "securityfs",
    "tracefs",
    "debugfs",
    "bpf",
    "autofs",
    "overlay",
    "fuse.gvfsd-fuse",
    "fusectl",
    "configfs",
    "squashfs",
];

const SKIP_PREFIXES: &[&str] = &["/sys", "/proc", "/dev", "/run"];

#[derive(Default)]
pub struct FsPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    /// (mountpoint, disambiguated_safekey) pairs. The safekey is what the
    /// sample path uses to find the right mountpoint to statvfs(), so it
    /// must be the post-disambiguation value, not the bare result of
    /// [`mount_safekey`].
    mounts: Vec<(String, String)>,
}

fn mount_safekey(mountpoint: &str) -> String {
    let s = mountpoint.trim_start_matches('/');
    if s.is_empty() { "root".into() } else { s.replace('/', "_") }
}

fn read_mtab(sysroot: Option<&Path>) -> Vec<(String, String)> {
    let path = match sysroot {
        Some(r) => r.join("etc/mtab"),
        None => PathBuf::from("/etc/mtab"),
    };
    let path = if path.exists() {
        path
    } else {
        match sysroot {
            Some(r) => r.join("proc/mounts"),
            None => PathBuf::from("/proc/mounts"),
        }
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        if PSEUDO_FS.contains(&f[2]) {
            continue;
        }
        if SKIP_PREFIXES.iter().any(|p| f[1].starts_with(p)) {
            continue;
        }
        if f[0] == "none" || f[0] == "tmpfs" {
            continue;
        }
        out.push((f[1].to_owned(), f[2].to_owned()));
    }
    out
}

impl FsPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("FsPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        let mtab = read_mtab(inner.sysroot.as_deref());

        let mut sensors = Vec::new();
        let mut devices: Vec<HardwareDevice> = Vec::new();
        // Track which base safekeys we've already used so we can
        // disambiguate collisions deterministically by mtab line order.
        // Common case: `/` -> "root" and `/root` -> "root" both legitimately
        // exist on many Btrfs setups; the first keeps the bare key, later
        // collisions get an "_<n>" suffix where n is the 1-indexed mtab
        // position (stable across reboots barring mount-order changes).
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Stash the (mountpoint, disambiguated safekey) pairs so
        // `sample_inner` can map back without re-running disambiguation.
        let mut resolved_mounts: Vec<(String, String)> = Vec::with_capacity(mtab.len());
        for (mtab_idx, (mountpoint, fstype)) in mtab.iter().enumerate() {
            let base = mount_safekey(mountpoint);
            let safe =
                if taken.insert(base.clone()) { base } else { format!("{base}_{}", mtab_idx + 1) };
            resolved_mounts.push((mountpoint.clone(), safe.clone()));
            let key = HardwareDeviceKey::try_new(format!("fs:{safe}"))
                .map_err(|e| PluginError::Io(format!("fs {safe}: {e}")))?;
            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Storage,
                model: format!("{} ({mountpoint})", fstype),
                vendor: None,
                location: Some(mountpoint.clone()),
                plugin_id: String::new(),
                plugin_device_id: safe.clone(),
                sensor_ids: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("fs.{safe}.total_bytes")),
                display_name: "Filesystem total".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 0.2,
                min: Some(0.0),
                max: None,
                device_id: Some(safe.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("fs.{safe}.used_bytes")),
                display_name: "Filesystem used".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: Some(safe.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("fs.{safe}.avail_bytes")),
                display_name: "Filesystem available".into(),
                unit: Unit::Bytes,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: Some(safe.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("fs.{safe}.inodes_total")),
                display_name: "Inodes total".into(),
                unit: Unit::Count,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 0.2,
                min: Some(0.0),
                max: None,
                device_id: Some(safe.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("fs.{safe}.inodes_used")),
                display_name: "Inodes used".into(),
                unit: Unit::Count,
                kind: SensorKind::Scalar,
                category: Category::Storage,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: None,
                device_id: Some(safe.clone()),
                device_key: Some(key.clone()),
                tags: vec![],
            });
        }
        inner.mounts = resolved_mounts;
        Ok(PluginManifest {
            plugin_id: "io.visorcraft.linsight.fs".into(),
            display_name: "Filesystem".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let inner = self.inner.lock().expect("FsPlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("fs.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (safe, metric) =
            rest.rsplit_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let mountpoint = inner
            .mounts
            .iter()
            .find(|(_, s)| s == safe)
            .map(|(mp, _)| mp)
            .ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (total, avail, inodes, inodes_free) = statvfs_raw(mountpoint)?;
        let value = match metric {
            "total_bytes" => total as f64,
            "used_bytes" => total.saturating_sub(avail) as f64,
            "avail_bytes" => avail as f64,
            "inodes_total" => inodes as f64,
            "inodes_used" => inodes.saturating_sub(inodes_free) as f64,
            _ => return Err(PluginError::Unsupported(id.into())),
        };
        Ok(Reading::Scalar(value))
    }
}

impl LinsightPlugin for FsPlugin {
    extern "C" fn init(&self, ctx: &RPluginCtx) -> RInitResult {
        let host_ctx: PluginCtx = ctx.into();
        match self.init_inner(&host_ctx) {
            Ok(m) => SResult::Ok(<PluginManifest as Into<RPluginManifest>>::into(m)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }
    extern "C" fn sample(&self, sensor: RSensorId) -> RSampleResult {
        let id: SensorId = sensor.into();
        match self.sample_inner(id) {
            Ok(r) => SResult::Ok(<Reading as Into<RReading>>::into(r)),
            Err(e) => SResult::Err(<PluginError as Into<RPluginError>>::into(e)),
        }
    }
}

fn statvfs_raw(path_str: &str) -> Result<(u64, u64, u64, u64), PluginError> {
    #[cfg(target_os = "linux")]
    {
        use std::mem::MaybeUninit;
        unsafe {
            let mut st: MaybeUninit<libc::statvfs64> = MaybeUninit::uninit();
            let c = std::ffi::CString::new(path_str)
                .map_err(|e| PluginError::Io(format!("path: {e}")))?;
            let ret = libc::statvfs64(c.as_ptr(), st.as_mut_ptr());
            if ret != 0 {
                return Err(PluginError::Io(std::io::Error::last_os_error().to_string()));
            }
            let s = st.assume_init();
            Ok((
                s.f_blocks.saturating_mul(s.f_frsize),
                s.f_bavail.saturating_mul(s.f_frsize),
                s.f_files,
                s.f_ffree,
            ))
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(PluginError::Io("unsupported platform".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_plugin_sdk::{host_init, host_sample};

    fn fake_mtab(content: &str) -> tempfile::TempDir {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(d.path().join("etc")).unwrap();
        std::fs::write(d.path().join("etc/mtab"), content).unwrap();
        d
    }

    #[test]
    fn read_mtab_skips_pseudo() {
        let d = fake_mtab("proc proc proc rw 0 0\n/dev/sda1 / ext4 rw 0 0\n");
        assert_eq!(read_mtab(Some(d.path())).len(), 1);
    }

    #[test]
    fn manifest_advertises_fs_sensors() {
        let d = fake_mtab("/dev/sda1 / ext4 rw 0 0\n");
        let p = FsPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(d.path().to_path_buf()).unwrap();
        let m = host_init(&p, &ctx).unwrap();
        assert_eq!(m.sensors.len(), 5);
        assert!(m.sensors.iter().any(|s| s.id.as_str() == "fs.root.total_bytes"));
    }

    #[test]
    fn safekey_handles_paths() {
        assert_eq!(mount_safekey("/"), "root");
        assert_eq!(mount_safekey("/home"), "home");
    }

    #[test]
    fn disambiguated_safekey_is_sampleable() {
        // After disambiguation, the second mount that collided on safekey
        // "root" gets renamed (e.g. "root_2"). The sample path must be
        // able to map that disambiguated key back to the real mountpoint;
        // otherwise the daemon's scheduler logs
        // `plugin no longer supports sensor; backing off` for every reading.
        // Uses real "/" and "/root" paths (both statvfs-callable on the
        // test host) so we exercise the full init → sample flow.
        let d = fake_mtab(
            "/dev/nvme2n1p2 / btrfs rw 0 0\n\
             /dev/nvme2n1p2 /root btrfs rw 0 0\n",
        );
        let p = FsPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(d.path().to_path_buf()).unwrap();
        let m = host_init(&p, &ctx).unwrap();
        let disambiguated_id = m
            .sensors
            .iter()
            .map(|s| s.id.as_str())
            .find(|id| id.starts_with("fs.root_") && id.ends_with(".total_bytes"))
            .expect("expected a disambiguated root_<n> sensor id")
            .to_owned();
        // The actual sample call must succeed for the disambiguated id.
        let result = host_sample(&p, SensorId::new(disambiguated_id.clone()));
        assert!(
            result.is_ok(),
            "sampling disambiguated sensor {disambiguated_id} must succeed: {result:?}"
        );
    }

    #[test]
    fn mounts_with_colliding_safekeys_get_distinct_keys() {
        // On Btrfs setups (and many others) the mount at `/` produces
        // safekey "root", and a separate `/root` mount also produces
        // safekey "root", colliding at the manifest dedup step. Each
        // distinct mount must surface with a unique device key.
        let d = fake_mtab(
            "/dev/nvme2n1p2 / btrfs rw 0 0\n\
             /dev/nvme2n1p2 /root btrfs rw 0 0\n",
        );
        let p = FsPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(d.path().to_path_buf()).unwrap();
        let m = host_init(&p, &ctx).unwrap();
        assert_eq!(m.devices.len(), 2, "both mounts must surface");
        let keys: std::collections::HashSet<_> =
            m.devices.iter().map(|d| d.key.as_str().to_owned()).collect();
        assert_eq!(keys.len(), 2, "device keys must be unique: {keys:?}");
        let ids: std::collections::HashSet<_> =
            m.sensors.iter().map(|s| s.id.as_str().to_owned()).collect();
        assert_eq!(ids.len(), m.sensors.len(), "sensor IDs must be unique across mounts: {ids:?}");
    }

    #[test]
    fn unknown_sensor_errors() {
        let d = fake_mtab("/dev/sda1 / ext4 rw 0 0\n");
        let p = FsPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(d.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();
        assert!(host_sample(&p, SensorId::new("fs.nope.total_bytes")).is_err());
    }
}
