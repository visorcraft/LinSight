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

fn read_mtab(sysroot: Option<&Path>) -> Vec<(String, String, String)> {
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
        out.push((f[0].to_owned(), f[1].to_owned(), f[2].to_owned()));
    }
    out
}

/// Resolve a `/proc/mounts` source device (column 0) to the physical device id
/// that the disk/nvme plugins use as their `device_id`, so fs tiles can be
/// grouped under their backing disk in the GUI.
///
/// Returns `None` when the source is not a real block device, or resolves to a
/// device the disk/nvme plugins do not expose (zram, dm/LVM, loop, md, network
/// sources). Such mounts stay as their own top-level sections in the UI.
fn resolve_parent_device(source: &str, sysroot: Option<&Path>) -> Option<String> {
    let dev = source.strip_prefix("/dev/")?;
    // dm/LVM, loop, md, zram are skipped by the disk plugin -> no disk section
    // to nest under; treat as unresolved.
    if dev.starts_with("mapper/")
        || dev.starts_with("dm-")
        || dev.starts_with("loop")
        || dev.starts_with("md")
        || dev.starts_with("zram")
    {
        return None;
    }
    let sys_block = match sysroot {
        Some(r) => r.join("sys/block"),
        None => PathBuf::from("/sys/block"),
    };
    let disk = find_block_disk(&sys_block, dev)?;
    Some(nvme_controller(&disk))
}

/// Find the whole-disk kernel name that owns block device `dev` by walking
/// `/sys/block`. A whole disk appears directly (`sda`, `nvme0n1`); a partition
/// appears as a subdirectory of its disk (`sda/sda3`, `nvme0n1/nvme0n1p2`).
/// Uses directory topology, not name-stripping, so it is correct across
/// nvme/mmc/sd naming.
fn find_block_disk(sys_block: &Path, dev: &str) -> Option<String> {
    let direct = sys_block.join(dev);
    if direct.is_dir() && !direct.join("partition").exists() {
        return Some(dev.to_owned()); // whole disk
    }
    for entry in std::fs::read_dir(sys_block).ok()?.flatten() {
        if entry.path().join(dev).is_dir() {
            return Some(entry.file_name().to_string_lossy().into_owned());
        }
    }
    None
}

/// Map an NVMe namespace to its controller (`nvme0n1` -> `nvme0`); the nvme
/// plugin keys its disk by controller name. Non-nvme names pass through.
fn nvme_controller(disk: &str) -> String {
    if let Some(rest) = disk.strip_prefix("nvme")
        && let Some(npos) = rest.find('n')
    {
        let ctrl = &rest[..npos];
        if !ctrl.is_empty() && ctrl.chars().all(|c| c.is_ascii_digit()) {
            return format!("nvme{ctrl}");
        }
    }
    disk.to_owned()
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
        for (mtab_idx, (source, mountpoint, fstype)) in mtab.iter().enumerate() {
            let parent_tags: Vec<String> = resolve_parent_device(source, inner.sysroot.as_deref())
                .map(|p| vec![format!("parent:{p}")])
                .unwrap_or_default();
            // Some filesystems (btrfs, FAT/vfat, exFAT, ...) don't expose inode
            // counts via statvfs (f_files == 0), so inodes_total/inodes_used are
            // perpetually 0. Skip those sensors for such mounts. A statvfs error
            // (e.g. a mount that vanished) defaults to keeping them.
            let reports_inodes = statvfs_raw(mountpoint)
                .map(|(_, _, inodes, _)| inodes > 0)
                .unwrap_or(true);
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
                tags: parent_tags.clone(),
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
                tags: parent_tags.clone(),
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
                tags: parent_tags.clone(),
            });
            if reports_inodes {
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
                    tags: parent_tags.clone(),
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
                    tags: parent_tags.clone(),
                });
            }
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
    fn nvme_namespace_maps_to_controller() {
        assert_eq!(super::nvme_controller("nvme0n1"), "nvme0");
        assert_eq!(super::nvme_controller("nvme10n2"), "nvme10");
        assert_eq!(super::nvme_controller("sda"), "sda");
        assert_eq!(super::nvme_controller("nvmexn1"), "nvmexn1"); // non-numeric ctrl: passthrough
    }

    fn block_fixture(disks: &[&str], parts: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let block = dir.path().join("sys/block");
        for d in disks {
            std::fs::create_dir_all(block.join(d)).unwrap();
        }
        for (disk, part) in parts {
            let p = block.join(disk).join(part);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("partition"), "1\n").unwrap(); // marks it a partition
        }
        dir
    }

    #[test]
    fn resolves_sata_partition_to_disk() {
        let dir = block_fixture(&["sda"], &[("sda", "sda3")]);
        let got = super::resolve_parent_device("/dev/sda3", Some(dir.path()));
        assert_eq!(got, Some("sda".to_owned()));
    }

    #[test]
    fn resolves_nvme_partition_to_controller() {
        let dir = block_fixture(&["nvme0n1"], &[("nvme0n1", "nvme0n1p2")]);
        let got = super::resolve_parent_device("/dev/nvme0n1p2", Some(dir.path()));
        assert_eq!(got, Some("nvme0".to_owned()));
    }

    #[test]
    fn resolves_whole_disk_to_itself() {
        let dir = block_fixture(&["sdb"], &[]);
        let got = super::resolve_parent_device("/dev/sdb", Some(dir.path()));
        assert_eq!(got, Some("sdb".to_owned()));
    }

    #[test]
    fn unresolvable_sources_return_none() {
        let dir = block_fixture(&["sda"], &[("sda", "sda1")]);
        assert_eq!(super::resolve_parent_device("nas:/export", Some(dir.path())), None);
        assert_eq!(super::resolve_parent_device("none", Some(dir.path())), None);
        assert_eq!(super::resolve_parent_device("/dev/mapper/vg-root", Some(dir.path())), None);
        assert_eq!(super::resolve_parent_device("/dev/zram0", Some(dir.path())), None);
        assert_eq!(super::resolve_parent_device("/dev/dm-0", Some(dir.path())), None);
    }

    #[test]
    fn fs_sensors_carry_parent_tag_for_backed_mounts() {
        // sysroot with one btrfs mount on /dev/nvme0n1p2 and one nfs mount.
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("sys/block/nvme0n1/nvme0n1p2")).unwrap();
        std::fs::write(dir.path().join("sys/block/nvme0n1/nvme0n1p2/partition"), "2\n").unwrap();
        std::fs::create_dir_all(dir.path().join("proc")).unwrap();
        std::fs::write(
            dir.path().join("proc/mounts"),
            "/dev/nvme0n1p2 /home btrfs rw 0 0\nnas:/media /mnt/media nfs rw 0 0\n",
        )
        .unwrap();

        let plugin = super::FsPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap();
        let manifest = plugin.init_inner(&ctx).unwrap();

        let home = manifest
            .sensors
            .iter()
            .find(|s| s.id.as_str() == "fs.home.total_bytes")
            .expect("home sensor");
        assert!(home.tags.iter().any(|t| t == "parent:nvme0"), "tags={:?}", home.tags);

        let media = manifest
            .sensors
            .iter()
            .find(|s| s.id.as_str() == "fs.mnt_media.total_bytes")
            .expect("media sensor");
        assert!(!media.tags.iter().any(|t| t.starts_with("parent:")), "nfs should have no parent");
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
        let reports_inodes =
            super::statvfs_raw("/").map(|(_, _, inodes, _)| inodes > 0).unwrap_or(true);
        assert_eq!(m.sensors.len(), if reports_inodes { 5 } else { 3 });
        assert!(m.sensors.iter().any(|s| s.id.as_str() == "fs.root.total_bytes"));
        assert_eq!(
            m.sensors.iter().any(|s| s.id.as_str() == "fs.root.inodes_total"),
            reports_inodes
        );
        assert_eq!(
            m.sensors.iter().any(|s| s.id.as_str() == "fs.root.inodes_used"),
            reports_inodes
        );
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
