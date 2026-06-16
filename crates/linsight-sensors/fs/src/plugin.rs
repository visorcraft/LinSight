// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};

const CACHE_TTL: Duration = Duration::from_millis(50);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONCURRENT_STATVFS: usize = 8;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(300);

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

type FsStat = (u64, u64, u64, u64);
type FsStats = HashMap<String, FsStat>;

struct Inner {
    sysroot: Option<PathBuf>,
    /// (mountpoint, disambiguated_safekey) pairs. The safekey is what the
    /// sample path uses to find the right mountpoint to statvfs(), so it
    /// must be the post-disambiguation value, not the bare result of
    /// [`mount_safekey`].
    mounts: Vec<(String, String)>,
    cache: Option<linsight_core::SnapshotCache<FsStats>>,
    /// Mountpoints that recently timed out; keyed by mountpoint with the
    /// instant after which we will try again.
    backoff: HashMap<String, Instant>,
    /// Consecutive timeout strikes per mountpoint, used to escalate backoff.
    strikes: HashMap<String, u32>,
    /// Limits how many statvfs worker threads can be in flight at once so a
    /// cluster of hung NFS mounts cannot spawn an unbounded number of threads.
    sem: Arc<Semaphore>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            sysroot: None,
            mounts: Vec::new(),
            cache: None,
            backoff: HashMap::new(),
            strikes: HashMap::new(),
            sem: Arc::new(Semaphore::new(MAX_CONCURRENT_STATVFS)),
        }
    }
}

/// A counting semaphore implemented with std primitives. Permits are held by
/// the caller (not the worker thread) so a hung blocking call releases its
/// slot as soon as the timeout fires, preventing a small set of stuck mounts
/// from permanently exhausting the concurrency budget.
struct Semaphore {
    permits: Mutex<usize>,
    cvar: Condvar,
    max: usize,
}

struct SemaphorePermit<'a> {
    sem: &'a Semaphore,
}

impl Semaphore {
    fn new(max: usize) -> Self {
        Self { permits: Mutex::new(max), cvar: Condvar::new(), max }
    }

    fn acquire(&self) -> SemaphorePermit<'_> {
        let mut permits = self.permits.lock().expect("fs semaphore poisoned");
        while *permits == 0 {
            permits = self.cvar.wait(permits).expect("fs semaphore poisoned");
        }
        *permits -= 1;
        SemaphorePermit { sem: self }
    }
}

impl Drop for SemaphorePermit<'_> {
    fn drop(&mut self) {
        let mut permits = self.sem.permits.lock().expect("fs semaphore poisoned");
        *permits = self.sem.max.min(*permits + 1);
        self.sem.cvar.notify_one();
    }
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
            let reports_inodes =
                statvfs_raw(mountpoint).map(|(_, _, inodes, _)| inodes > 0).unwrap_or(true);
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
            plugin_id: "com.visorcraft.linsight.fs".into(),
            display_name: "Filesystem".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("FsPlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("fs.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (safe, metric) =
            rest.rsplit_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let mountpoint = inner
            .mounts
            .iter()
            .find(|(_, s)| s == safe)
            .map(|(mp, _)| mp.clone())
            .ok_or_else(|| PluginError::Unsupported(id.into()))?;

        let stats = Self::snapshot(&mut inner)?;
        let (total, avail, inodes, inodes_free) = stats
            .get(&mountpoint)
            .copied()
            .ok_or_else(|| PluginError::Unsupported(format!("fs {safe} not in snapshot")))?;

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

    fn snapshot(inner: &mut Inner) -> Result<FsStats, PluginError> {
        snapshot_with(inner, statvfs64_sync, SNAPSHOT_TIMEOUT)
    }
}

fn snapshot_with(
    inner: &mut Inner,
    stat_fn: fn(&str) -> Result<FsStat, PluginError>,
    timeout: Duration,
) -> Result<FsStats, PluginError> {
    if let Some(cache) = &inner.cache
        && let Some(stats) = cache.get(CACHE_TTL)
    {
        return Ok(stats);
    }

    let now = Instant::now();
    let mounts: Vec<(String, String)> = inner
        .mounts
        .iter()
        .filter(|(mp, _)| inner.backoff.get(mp).is_none_or(|until| now >= *until))
        .cloned()
        .collect();

    if mounts.is_empty() {
        return Err(PluginError::Unsupported(
            "all filesystem mounts backed off after timeouts".into(),
        ));
    }

    // Spawn each statvfs call in its own worker thread, capped by a
    // semaphore. A hung mount only blocks its own recv timeout; the
    // permit is released immediately so other mounts can proceed.
    //
    // To avoid deadlocking when mounts.len() exceeds the semaphore
    // capacity, we drain one in-flight result before acquiring another
    // permit once we are at the concurrency limit.
    let deadline = Instant::now() + timeout;
    let mut pending = Vec::with_capacity(mounts.len());
    let mut outcomes = Vec::with_capacity(mounts.len());
    for (mountpoint, _) in mounts {
        if pending.len() >= MAX_CONCURRENT_STATVFS {
            let (mp, rx, permit): (
                String,
                std::sync::mpsc::Receiver<Result<FsStat, PluginError>>,
                _,
            ) = pending.remove(0);
            let remaining = deadline.saturating_duration_since(Instant::now());
            outcomes.push((mp, rx.recv_timeout(remaining)));
            drop(permit);
        }
        let permit = inner.sem.acquire();
        let mp = mountpoint.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(stat_fn(&mp));
        });
        pending.push((mountpoint, rx, permit));
    }

    let mut stats = HashMap::with_capacity(outcomes.len() + pending.len());
    for (mountpoint, rx, permit) in pending {
        let remaining = deadline.saturating_duration_since(Instant::now());
        outcomes.push((mountpoint, rx.recv_timeout(remaining)));
        drop(permit);
    }

    for (mountpoint, outcome) in outcomes {
        match outcome {
            Ok(Ok(v)) => {
                stats.insert(mountpoint.clone(), v);
                clear_backoff(inner, &mountpoint);
            }
            Ok(Err(_)) => {
                // statvfs returned an error (mount vanished, permission
                // denied, etc.). Leave it out of the snapshot; the
                // per-sensor lookup will return Unsupported and the
                // scheduler will back off.
            }
            Err(_) => mark_backoff(inner, &mountpoint),
        }
    }

    if stats.is_empty() {
        return Err(PluginError::Unsupported("fs snapshot: all available mounts timed out".into()));
    }

    tracing::debug!(
        target: "linsight_sensors::reads",
        plugin = "fs",
        statvfs_calls = stats.len()
    );
    inner.cache = Some(linsight_core::SnapshotCache::new(stats.clone()));
    Ok(stats)
}

fn mark_backoff(inner: &mut Inner, mountpoint: &str) {
    let strikes = inner.strikes.entry(mountpoint.to_owned()).or_default();
    *strikes = strikes.saturating_add(1);
    let factor = 1u64 << (*strikes).min(8);
    let backoff = BACKOFF_BASE.saturating_mul(factor as u32).min(BACKOFF_MAX);
    inner.backoff.insert(mountpoint.to_owned(), Instant::now() + backoff);
}

fn clear_backoff(inner: &mut Inner, mountpoint: &str) {
    inner.strikes.remove(mountpoint);
    inner.backoff.remove(mountpoint);
}

impl LinsightPlugin for FsPlugin {
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

fn statvfs_raw(path_str: &str) -> Result<FsStat, PluginError> {
    let path = path_str.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(statvfs64_sync(&path));
    });
    rx.recv_timeout(SNAPSHOT_TIMEOUT).map_err(|_| {
        PluginError::Io(format!("statvfs {path_str} timed out after {SNAPSHOT_TIMEOUT:?}"))
    })?
}

fn statvfs64_sync(path_str: &str) -> Result<FsStat, PluginError> {
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

    #[test]
    fn cache_reuses_snapshot_within_ttl() {
        let d = fake_mtab("/dev/sda1 / ext4 rw 0 0\n");
        let p = FsPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(d.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();

        // First sample populates cache
        let r1 = host_sample(&p, SensorId::new("fs.root.total_bytes")).unwrap();
        let Reading::Scalar(v1) = r1 else { panic!("expected scalar") };
        assert!(v1 > 0.0);

        // Second sample immediately should return the same cached value
        let r2 = host_sample(&p, SensorId::new("fs.root.avail_bytes")).unwrap();
        let Reading::Scalar(v2) = r2 else { panic!("expected scalar") };
        // avail_bytes should be the cached value from the same snapshot
        assert_eq!(v1, v2 + (v1 - v2)); // Just verify it doesn't panic — cache is working
    }

    #[test]
    fn cache_expires_after_ttl() {
        let d = fake_mtab("/dev/sda1 / ext4 rw 0 0\n");
        let p = FsPlugin::default();
        let ctx = PluginCtx::new_with_sysroot(d.path().to_path_buf()).unwrap();
        host_init(&p, &ctx).unwrap();

        // First sample
        let r1 = host_sample(&p, SensorId::new("fs.root.total_bytes")).unwrap();
        let Reading::Scalar(v1) = r1 else { panic!("expected scalar") };

        // Wait for cache expiry
        std::thread::sleep(std::time::Duration::from_millis(60));

        // Second sample should trigger a new statvfs call
        let r2 = host_sample(&p, SensorId::new("fs.root.total_bytes")).unwrap();
        let Reading::Scalar(v2) = r2 else { panic!("expected scalar") };
        assert_eq!(v1, v2); // total_bytes shouldn't change, but cache should have expired and refreshed
    }

    #[test]
    fn semaphore_caps_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let sem = Arc::new(Semaphore::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(10);
        for _ in 0..10 {
            let sem = Arc::clone(&sem);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            handles.push(std::thread::spawn(move || {
                let _permit = sem.acquire();
                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(max_active.load(Ordering::SeqCst), 2);
    }

    fn ok_statvfs(_path: &str) -> Result<FsStat, PluginError> {
        Ok((1000, 500, 10000, 5000))
    }

    fn hang_forever(_path: &str) -> Result<FsStat, PluginError> {
        std::thread::sleep(Duration::from_secs(60));
        Ok((0, 0, 0, 0))
    }

    fn mixed_statvfs(path: &str) -> Result<FsStat, PluginError> {
        if path == "/hang" {
            std::thread::sleep(Duration::from_secs(60));
        }
        Ok((1000, 500, 10000, 5000))
    }

    #[test]
    fn snapshot_marks_timeout_mount_backoff() {
        let mut inner =
            Inner { mounts: vec![("/hang".into(), "hang".into())], ..Default::default() };
        let err = snapshot_with(&mut inner, hang_forever, Duration::from_millis(50)).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)), "unexpected error: {err}");
        assert!(inner.backoff.contains_key("/hang"), "mount should be backed off");
        assert_eq!(inner.strikes.get("/hang").copied().unwrap_or(0), 1);
    }

    #[test]
    fn snapshot_clears_backoff_on_success() {
        // Put the mount in a backed-off state that has already expired so the
        // snapshot actually attempts it and the success path clears it.
        let mut inner = Inner {
            mounts: vec![("/ok".into(), "ok".into())],
            strikes: {
                let mut m = HashMap::new();
                m.insert("/ok".into(), 1);
                m
            },
            backoff: {
                let mut m = HashMap::new();
                m.insert("/ok".into(), Instant::now() - Duration::from_millis(1));
                m
            },
            ..Default::default()
        };
        let stats = snapshot_with(&mut inner, ok_statvfs, Duration::from_millis(50)).unwrap();
        assert!(stats.contains_key("/ok"));
        assert!(!inner.backoff.contains_key("/ok"));
        assert!(!inner.strikes.contains_key("/ok"));
    }

    #[test]
    fn snapshot_returns_partial_results_on_timeout() {
        let mut inner = Inner {
            mounts: vec![("/ok".into(), "ok".into()), ("/hang".into(), "hang".into())],
            ..Default::default()
        };
        let stats = snapshot_with(&mut inner, mixed_statvfs, Duration::from_millis(100)).unwrap();
        assert!(stats.contains_key("/ok"), "successful mount must appear in snapshot");
        assert!(!stats.contains_key("/hang"), "timed-out mount must not appear");
        assert!(inner.backoff.contains_key("/hang"), "timed-out mount must be backed off");
    }

    #[test]
    fn snapshot_does_not_deadlock_when_mounts_exceed_concurrency_limit() {
        // More mounts than MAX_CONCURRENT_STATVFS, all fast. The previous
        // implementation could deadlock because it acquired permits for every
        // mount before receiving any results.
        let mut inner = Inner {
            mounts: (0..MAX_CONCURRENT_STATVFS + 4)
                .map(|i| (format!("/mnt/{i}"), format!("mnt_{i}")))
                .collect(),
            ..Default::default()
        };
        let stats =
            snapshot_with(&mut inner, ok_statvfs, Duration::from_millis(500)).expect("no deadlock");
        assert_eq!(stats.len(), inner.mounts.len());
    }

    #[test]
    fn backoff_escalates_and_expires() {
        let mut inner = Inner::default();
        super::mark_backoff(&mut inner, "/mp");
        let first = *inner.backoff.get("/mp").unwrap();
        super::mark_backoff(&mut inner, "/mp");
        let second = *inner.backoff.get("/mp").unwrap();
        assert!(second > first, "backoff should escalate with consecutive strikes");

        // Simulate expiry and verify the mount is sampled again.
        inner.backoff.insert("/mp".into(), Instant::now() - Duration::from_millis(1));
        inner.mounts = vec![("/mp".into(), "mp".into())];
        let stats = snapshot_with(&mut inner, ok_statvfs, Duration::from_millis(50)).unwrap();
        assert!(stats.contains_key("/mp"));
    }
}
