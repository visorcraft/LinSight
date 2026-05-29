// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Sysfs walker for xe-driven DRM cards.
//!
//! Enumerates `/sys/class/drm/card*` whose `driver` symlink resolves to `xe`,
//! and exposes typed read helpers for the metrics LinSight ships.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum XeError {
    #[error("io: {0}")]
    Io(String),
    #[error("parse: {0}")]
    Parse(String),
}

#[derive(Clone, Debug)]
pub struct XeDevice {
    /// PCI slot, e.g. `0000:06:00.0` — used as a stable instance key.
    pub pci_slot: String,
    /// Sysfs root, e.g. `/sys/class/drm/card2/device`.
    pub device_root: PathBuf,
    /// First hwmon under the device, if any. iGPUs typically have none;
    /// discrete cards (B70, etc.) expose temp/power/fan here.
    pub hwmon_root: Option<PathBuf>,
    /// PCI vendor id, e.g. `0x8086`. None if the sysfs read failed.
    pub vendor_id: Option<u16>,
    /// PCI device id (model), e.g. `0xe223`. None if the sysfs read failed.
    pub device_id: Option<u16>,
}

impl XeDevice {
    /// Read the most-recent actuated frequency in MHz.
    pub fn act_freq_mhz(&self) -> Result<u32, XeError> {
        read_u32(&self.device_root.join("tile0/gt0/freq0/act_freq"))
    }

    /// Read the "package" temperature in m°C from the first hwmon, if any.
    /// Searches `temp*_label` for one whose value is "package" / "pkg" /
    /// "gpu" and returns the matching `temp*_input`. Falls back to
    /// `temp1_input` if no labelled sensor matches.
    pub fn package_temp_milli_c(&self) -> Option<i32> {
        let hwmon = self.hwmon_root.as_ref()?;
        find_labelled_temp(hwmon).or_else(|| read_i32(&hwmon.join("temp1_input")).ok())
    }

    /// Read instantaneous fan speed in RPM from `fan1_input`, if present.
    pub fn fan_rpm(&self) -> Option<u32> {
        let hwmon = self.hwmon_root.as_ref()?;
        read_u32(&hwmon.join("fan1_input")).ok()
    }

    /// Best-effort VRAM total for the card, in bytes. The xe driver
    /// on current kernels (≤ 7.1 at time of writing) doesn't expose a
    /// `vram_total` sysfs entry, so we read the PCI BAR2 size from
    /// the device's `resource` file. On Intel discrete GPUs with
    /// Resizable BAR (the default on modern boards) BAR2 maps the
    /// entire local-memory aperture, so its size equals VRAM total.
    /// Returns `None` for iGPUs (no discrete VRAM, BAR may be tiny
    /// or absent) and on systems with ReBAR disabled (BAR clamped to
    /// 256 MiB, which we filter out below).
    pub fn vram_total_bytes(&self) -> Option<u64> {
        let raw = fs::read_to_string(self.device_root.join("resource")).ok()?;
        // The `resource` file lists every BAR on its own line:
        //   <start_hex> <end_hex> <flags_hex>
        // BAR2 (index 2) is the VRAM aperture on Intel discrete cards.
        let line = raw.lines().nth(2)?;
        let mut parts = line.split_whitespace();
        let start = u64::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()?;
        let end = u64::from_str_radix(parts.next()?.trim_start_matches("0x"), 16).ok()?;
        if end <= start {
            return None;
        }
        let size = end - start + 1;
        // Filter out the legacy 256 MiB non-ReBAR BAR — that's the
        // PCIe spec default, not VRAM. iGPUs and ReBAR-disabled
        // configurations land here; the user gets no sensor rather
        // than a misleading "256 MiB VRAM".
        if size <= 256 * 1024 * 1024 {
            return None;
        }
        Some(size)
    }
}

/// Enumerate every DRM card whose driver is `xe`, in `card<N>` order.
pub fn enumerate(sysroot: Option<&Path>) -> Result<Vec<XeDevice>, XeError> {
    let drm_root = match sysroot {
        Some(root) => root.join("sys/class/drm"),
        None => PathBuf::from("/sys/class/drm"),
    };
    let entries = match fs::read_dir(&drm_root) {
        Ok(e) => e,
        // No /sys/class/drm at all (kernel without DRM, container without
        // sysfs, etc.) is not a hardware failure — it's "this machine has
        // no Intel xe GPUs". Match the nvme + nvml graceful-degrade
        // contract: return an empty list, log nothing alarming.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(XeError::Io(format!("read_dir {}: {e}", drm_root.display()))),
    };
    let mut cards: Vec<(u32, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(idx) = parse_card_index(&name) else {
            continue;
        };
        let device_root = entry.path().join("device");
        if !device_root.exists() {
            continue;
        }
        let driver_link = device_root.join("driver");
        let driver_name = match fs::read_link(&driver_link) {
            Ok(p) => p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            Err(_) => continue,
        };
        if driver_name != "xe" {
            continue;
        }
        cards.push((idx, device_root));
    }
    cards.sort_by_key(|(idx, _)| *idx);

    let mut out = Vec::with_capacity(cards.len());
    for (_, device_root) in cards {
        // device_root is always `<drm_root>/card<N>/device`, so its
        // parent is guaranteed to exist — but be defensive anyway, since
        // a future sysfs layout change shouldn't take down the daemon.
        let pci_slot = device_root
            .parent()
            .and_then(|parent| fs::read_link(parent.join("device")).ok())
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "unknown".into());
        let hwmon_root = find_first_hwmon(&device_root);
        let vendor_id = linsight_core::parse_sysfs_pci_id(&device_root.join("vendor"));
        let device_id = linsight_core::parse_sysfs_pci_id(&device_root.join("device"));
        out.push(XeDevice { pci_slot, device_root, hwmon_root, vendor_id, device_id });
    }
    Ok(out)
}

fn parse_card_index(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("card")?;
    rest.parse::<u32>().ok()
}

fn find_first_hwmon(device_root: &Path) -> Option<PathBuf> {
    let hwmon_dir = device_root.join("hwmon");
    let entries = fs::read_dir(&hwmon_dir).ok()?;
    let mut names: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|s| s.to_str()).is_some_and(|n| n.starts_with("hwmon")))
        .collect();
    names.sort();
    names.into_iter().next()
}

fn find_labelled_temp(hwmon: &Path) -> Option<i32> {
    let entries = fs::read_dir(hwmon).ok()?;
    let mut labels: Vec<(u32, String)> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("temp").and_then(|r| r.strip_suffix("_label")) else {
            continue;
        };
        let Ok(idx) = rest.parse::<u32>() else { continue };
        let Ok(label) = fs::read_to_string(e.path()) else { continue };
        labels.push((idx, label.trim().to_ascii_lowercase()));
    }
    labels.sort_by_key(|(i, _)| *i);
    for (idx, label) in &labels {
        if (label.contains("package") || label.contains("pkg") || label == "gpu")
            && let Ok(v) = read_i32(&hwmon.join(format!("temp{idx}_input")))
        {
            return Some(v);
        }
    }
    None
}

fn read_u32(p: &Path) -> Result<u32, XeError> {
    let s = fs::read_to_string(p).map_err(|e| XeError::Io(format!("{}: {e}", p.display())))?;
    s.trim().parse::<u32>().map_err(|e| XeError::Parse(format!("{}: {e}", p.display())))
}

fn read_i32(p: &Path) -> Result<i32, XeError> {
    let s = fs::read_to_string(p).map_err(|e| XeError::Io(format!("{}: {e}", p.display())))?;
    s.trim().parse::<i32>().map_err(|e| XeError::Parse(format!("{}: {e}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_card(root: &Path, idx: u32, driver: &str) -> PathBuf {
        let card = root.join(format!("sys/class/drm/card{idx}"));
        fs::create_dir_all(card.join("device/tile0/gt0/freq0")).unwrap();
        let driver_target = root.join(format!("sys/bus/pci/drivers/{driver}"));
        fs::create_dir_all(&driver_target).unwrap();
        std::os::unix::fs::symlink(&driver_target, card.join("device/driver")).unwrap();
        fs::write(card.join("device/tile0/gt0/freq0/act_freq"), "1200\n").unwrap();
        fs::write(card.join("device/vendor"), "0x8086\n").unwrap();
        fs::write(card.join("device/device"), "0xe223\n").unwrap();
        card
    }

    #[test]
    fn enumerate_finds_xe_cards_only() {
        let dir = tempfile::TempDir::new().unwrap();
        make_card(dir.path(), 0, "xe");
        make_card(dir.path(), 1, "nvidia");
        make_card(dir.path(), 2, "xe");
        let devices = enumerate(Some(dir.path())).unwrap();
        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn read_freq() {
        let dir = tempfile::TempDir::new().unwrap();
        make_card(dir.path(), 0, "xe");
        let devices = enumerate(Some(dir.path())).unwrap();
        let dev = &devices[0];
        assert_eq!(dev.act_freq_mhz().unwrap(), 1200);
        assert!(dev.package_temp_milli_c().is_none());
    }
}
