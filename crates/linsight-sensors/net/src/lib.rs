// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

//! Network interface sensor backend.
//!
//! Per interface in `/sys/class/net/*`:
//! * `net.<iface>.rx_bytes` — cumulative bytes received (Counter)
//! * `net.<iface>.tx_bytes` — cumulative bytes transmitted (Counter)
//! * `net.<iface>.rx_packets` — cumulative packets received (Counter)
//! * `net.<iface>.tx_packets` — cumulative packets transmitted (Counter)
//! * `net.<iface>.rx_errors` — cumulative receive errors (Counter)
//! * `net.<iface>.tx_errors` — cumulative transmit errors (Counter)
//! * `net.<iface>.rx_dropped` — cumulative receive drops (Counter)
//! * `net.<iface>.tx_dropped` — cumulative transmit drops (Counter)
//! * `net.<iface>.link_state` — "up" / "down" / "unknown" (State)
//! * `net.<iface>.speed_mbps` — negotiated link speed where exposed (Scalar)
//!
//! The loopback interface `lo` is the kernel's software interface and
//! has never been useful in a system monitor — `enumerate()` skips it
//! so it doesn't pollute the Hardware page or the sensor catalogue.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use linsight_core::{
    Category, HardwareCategory, HardwareDevice, HardwareDeviceKey, Reading, SensorId, SensorKind,
    Unit,
};
use linsight_plugin_sdk::pciids::PciIdDb;
use linsight_plugin_sdk::stabby::result::Result as SResult;
use linsight_plugin_sdk::{
    LinsightPlugin, PluginCtx, PluginError, PluginManifest, RInitResult, RPluginCtx, RPluginError,
    RPluginManifest, RReading, RSampleResult, RSensorId, SensorDescriptor,
};
use tracing::{debug, warn};

const CACHE_TTL: Duration = Duration::from_millis(50);

/// Read the PCI vendor/device pair for a network interface, if it has a
/// PCI parent. Returns `None` for purely logical interfaces (bonds, veth,
/// wireguard, loopback) which lack a `device/` subtree.
fn read_iface_pci_ids(sysroot: Option<&Path>, ifname: &str) -> Option<(u16, u16)> {
    let net_base = match sysroot {
        Some(r) => r.join("sys/class/net"),
        None => PathBuf::from("/sys/class/net"),
    };
    let dev = net_base.join(ifname).join("device");
    let v = linsight_core::parse_sysfs_pci_id(&dev.join("vendor"))?;
    let d = linsight_core::parse_sysfs_pci_id(&dev.join("device"))?;
    Some((v, d))
}

#[derive(Default)]
pub struct NetPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    interfaces: Vec<String>,
    cache: Option<NetCache>,
}

#[derive(Clone)]
struct NetIfaceStats {
    rx_bytes: u64,
    tx_bytes: u64,
    rx_packets: u64,
    tx_packets: u64,
    rx_errors: u64,
    tx_errors: u64,
    rx_dropped: u64,
    tx_dropped: u64,
    operstate: String,
    speed: i64,
}

struct NetCache {
    captured_at: Instant,
    stats: HashMap<String, NetIfaceStats>,
}

/// Statistics file names and their corresponding sensor metric suffix.
const STAT_SENSORS: &[(&str, &str)] = &[
    ("rx_bytes", "rx_bytes"),
    ("tx_bytes", "tx_bytes"),
    ("rx_packets", "rx_packets"),
    ("tx_packets", "tx_packets"),
    ("rx_errors", "rx_errors"),
    ("tx_errors", "tx_errors"),
    ("rx_dropped", "rx_dropped"),
    ("tx_dropped", "tx_dropped"),
];

impl NetPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("NetPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());
        let exclude = parse_exclude_patterns(ctx.config(), "exclude_interfaces");
        inner.interfaces =
            enumerate(ctx.sysroot(), &exclude).map_err(|e| PluginError::Io(e.to_string()))?;

        let sensors_per_iface = 4 + STAT_SENSORS.len() - 2; // 4 existing + 6 new = 10
        let mut sensors = Vec::with_capacity(inner.interfaces.len() * sensors_per_iface);
        let mut devices: Vec<HardwareDevice> = Vec::with_capacity(inner.interfaces.len());
        for iface in &inner.interfaces {
            let device_id = Some(iface.clone());
            let key = HardwareDeviceKey::try_new(format!("net:{iface}"))
                .map_err(|e| PluginError::Io(format!("net {iface} bad key: {e}")))?;
            let (vendor, model) = match read_iface_pci_ids(inner.sysroot.as_deref(), iface) {
                Some((v, d)) => {
                    let db = PciIdDb::shared();
                    let model = db.lookup(v, d).unwrap_or_else(|| iface.clone());
                    (db.vendor_name(v), model)
                }
                None => (None, iface.clone()),
            };
            devices.push(HardwareDevice {
                key: key.clone(),
                category: HardwareCategory::Network,
                model,
                vendor,
                location: None,
                plugin_id: String::new(),
                plugin_device_id: iface.clone(),
                sensor_ids: vec![],
            });

            // rx_bytes / tx_bytes — Unit::Bytes, Counter
            for &(stat_file, metric) in &[("rx_bytes", "rx_bytes"), ("tx_bytes", "tx_bytes")] {
                let _ = stat_file;
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("net.{iface}.{metric}")),
                    display_name: "Network received bytes".into(),
                    unit: Unit::Bytes,
                    kind: SensorKind::Counter,
                    category: Category::Network,
                    native_rate_hz: 2.0,
                    min: Some(0.0),
                    max: None,
                    device_id: device_id.clone(),
                    device_key: Some(key.clone()),
                    tags: vec![],
                });
            }
            // Fix the display name for tx_bytes
            let tx_sensor = sensors.last_mut().unwrap();
            tx_sensor.display_name = "Network transmitted bytes".into();

            // rx_packets, tx_packets, rx_errors, tx_errors, rx_dropped, tx_dropped — Unit::Count, Counter
            const EXTRA_METRICS: &[(&str, &str)] = &[
                ("rx_packets", "rx_packets"),
                ("tx_packets", "tx_packets"),
                ("rx_errors", "rx_errors"),
                ("tx_errors", "tx_errors"),
                ("rx_dropped", "rx_dropped"),
                ("tx_dropped", "tx_dropped"),
            ];
            for &(stat_file, metric) in EXTRA_METRICS {
                let _ = stat_file;
                let display: String = match metric {
                    "rx_packets" => "Network received packets".into(),
                    "tx_packets" => "Network transmitted packets".into(),
                    "rx_errors" => "Network receive errors".into(),
                    "tx_errors" => "Network transmit errors".into(),
                    "rx_dropped" => "Network receive drops".into(),
                    "tx_dropped" => "Network transmit drops".into(),
                    _ => unreachable!(),
                };
                sensors.push(SensorDescriptor {
                    id: SensorId::new(format!("net.{iface}.{metric}")),
                    display_name: display,
                    unit: Unit::Count,
                    kind: SensorKind::Counter,
                    category: Category::Network,
                    native_rate_hz: 2.0,
                    min: Some(0.0),
                    max: None,
                    device_id: device_id.clone(),
                    device_key: Some(key.clone()),
                    tags: vec![],
                });
            }

            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("net.{iface}.link_state")),
                display_name: "Network link state".into(),
                unit: Unit::Custom(String::new()),
                kind: SensorKind::State,
                category: Category::Network,
                native_rate_hz: 0.5,
                min: None,
                max: None,
                device_id: device_id.clone(),
                device_key: Some(key.clone()),
                tags: vec![],
            });
            // Speed is only meaningful for "up" interfaces and may be -1
            // when unknown; expose it anyway, samples will surface -1 as
            // "unknown" handled by the GUI.
            sensors.push(SensorDescriptor {
                id: SensorId::new(format!("net.{iface}.speed_mbps")),
                display_name: "Network link speed".into(),
                unit: Unit::Custom("Mb/s".into()),
                kind: SensorKind::Scalar,
                category: Category::Network,
                native_rate_hz: 0.2,
                min: None,
                max: None,
                device_id,
                device_key: Some(key),
                tags: vec![],
            });
        }
        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.net".into(),
            display_name: "Network".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices,
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let mut inner = self.inner.lock().expect("NetPlugin poisoned");
        let id = sensor.as_str();
        let rest = id.strip_prefix("net.").ok_or_else(|| PluginError::Unsupported(id.into()))?;
        let (iface, metric) =
            rest.rsplit_once('.').ok_or_else(|| PluginError::Unsupported(id.into()))?;
        if !inner.interfaces.iter().any(|i| i == iface) {
            return Err(PluginError::Unsupported(id.into()));
        }

        let stats = Self::snapshot(&mut inner)?;
        let s = stats
            .get(iface)
            .ok_or_else(|| PluginError::Unsupported(format!("net.{iface} not in snapshot")))?;

        match metric {
            "rx_bytes" => Ok(Reading::Counter(s.rx_bytes)),
            "tx_bytes" => Ok(Reading::Counter(s.tx_bytes)),
            "rx_packets" => Ok(Reading::Counter(s.rx_packets)),
            "tx_packets" => Ok(Reading::Counter(s.tx_packets)),
            "rx_errors" => Ok(Reading::Counter(s.rx_errors)),
            "tx_errors" => Ok(Reading::Counter(s.tx_errors)),
            "rx_dropped" => Ok(Reading::Counter(s.rx_dropped)),
            "tx_dropped" => Ok(Reading::Counter(s.tx_dropped)),
            "link_state" => Ok(Reading::State(s.operstate.clone())),
            "speed_mbps" => Ok(Reading::Scalar(s.speed as f64)),
            _ => Err(PluginError::Unsupported(id.into())),
        }
    }

    fn snapshot(inner: &mut Inner) -> Result<HashMap<String, NetIfaceStats>, PluginError> {
        if let Some(cache) = &inner.cache
            && cache.captured_at.elapsed() <= CACHE_TTL
        {
            return Ok(cache.stats.clone());
        }

        let mut stats = HashMap::with_capacity(inner.interfaces.len());
        let mut files_read = 0usize;
        for iface in &inner.interfaces {
            let base = match &inner.sysroot {
                Some(r) => r.join("sys/class/net").join(iface),
                None => Path::new("/sys/class/net").join(iface),
            };
            let Ok(rx_bytes) = read_u64(&base.join("statistics/rx_bytes")) else { continue; };
            let Ok(tx_bytes) = read_u64(&base.join("statistics/tx_bytes")) else { continue; };
            let Ok(rx_packets) = read_u64(&base.join("statistics/rx_packets")) else { continue; };
            let Ok(tx_packets) = read_u64(&base.join("statistics/tx_packets")) else { continue; };
            let Ok(rx_errors) = read_u64(&base.join("statistics/rx_errors")) else { continue; };
            let Ok(tx_errors) = read_u64(&base.join("statistics/tx_errors")) else { continue; };
            let Ok(rx_dropped) = read_u64(&base.join("statistics/rx_dropped")) else { continue; };
            let Ok(tx_dropped) = read_u64(&base.join("statistics/tx_dropped")) else { continue; };
            let operstate = read_string(&base.join("operstate")).unwrap_or_else(|_| "unknown".into());
            let speed = read_i64(&base.join("speed")).unwrap_or(-1);
            files_read += 10;
            stats.insert(
                iface.clone(),
                NetIfaceStats {
                    rx_bytes,
                    tx_bytes,
                    rx_packets,
                    tx_packets,
                    rx_errors,
                    tx_errors,
                    rx_dropped,
                    tx_dropped,
                    operstate,
                    speed,
                },
            );
        }
        tracing::debug!(target: "linsight_sensors::reads", plugin = "net", files_read);
        inner.cache = Some(NetCache {
            captured_at: Instant::now(),
            stats: stats.clone(),
        });
        Ok(stats)
    }
}

impl LinsightPlugin for NetPlugin {
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

fn enumerate(sysroot: Option<&Path>, exclude: &[String]) -> Result<Vec<String>, std::io::Error> {
    let root = match sysroot {
        Some(r) => r.join("sys/class/net"),
        None => PathBuf::from("/sys/class/net"),
    };
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => {
                let name = e.file_name().to_string_lossy().into_owned();
                // Loopback is a kernel software interface — not hardware.
                // Skipping at the enumerator level means it never appears
                // in the manifest's devices, the sensor catalogue, the
                // Hardware page, or the Prometheus output.
                if name == "lo" {
                    continue;
                }
                if matches_exclude(&name, exclude) {
                    continue;
                }
                names.push(name);
            }
            Err(e) => warn!(error = %e, "/sys/class/net: skipping unreadable entry"),
        }
    }
    names.sort();
    Ok(names)
}

fn read_u64(p: &Path) -> Result<u64, PluginError> {
    let s = fs::read_to_string(p).map_err(|e| PluginError::Io(format!("{}: {e}", p.display())))?;
    s.trim().parse::<u64>().map_err(|e| PluginError::Parse(format!("{}: {e}", p.display())))
}

fn read_i64(p: &Path) -> Result<i64, PluginError> {
    let s = fs::read_to_string(p).map_err(|e| PluginError::Io(format!("{}: {e}", p.display())))?;
    s.trim().parse::<i64>().map_err(|e| PluginError::Parse(format!("{}: {e}", p.display())))
}

fn read_string(p: &Path) -> Result<String, PluginError> {
    let s = fs::read_to_string(p).map_err(|e| PluginError::Io(format!("{}: {e}", p.display())))?;
    Ok(s.trim().to_owned())
}

fn parse_exclude_patterns(config: &serde_json::Value, key: &str) -> Vec<String> {
    match config.get(key) {
        Some(serde_json::Value::Array(arr)) => {
            arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        }
        _ => vec![],
    }
}

fn matches_exclude(name: &str, patterns: &[String]) -> bool {
    for p in patterns {
        let trimmed = p.trim_end_matches('*');
        if trimmed.is_empty() {
            continue;
        }
        if name.starts_with(trimmed) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    use linsight_plugin_sdk::{host_init, host_sample};

    /// Create a fake sysfs tree under a TempDir.
    ///
    /// Each tuple: (name, rx_bytes, tx_bytes, rx_packets, tx_packets,
    ///               rx_errors, tx_errors, rx_dropped, tx_dropped,
    ///               operstate, speed)
    #[allow(clippy::type_complexity)]
    fn fake_net_sysroot(
        interfaces: &[(&str, &str, &str, &str, &str, &str, &str, &str, &str, &str, &str)],
    ) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        for (
            iface,
            rx_bytes,
            tx_bytes,
            rx_packets,
            tx_packets,
            rx_errors,
            tx_errors,
            rx_dropped,
            tx_dropped,
            oper,
            speed,
        ) in interfaces
        {
            let p = dir.path().join("sys/class/net").join(iface).join("statistics");
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("rx_bytes"), format!("{rx_bytes}\n")).unwrap();
            fs::write(p.join("tx_bytes"), format!("{tx_bytes}\n")).unwrap();
            fs::write(p.join("rx_packets"), format!("{rx_packets}\n")).unwrap();
            fs::write(p.join("tx_packets"), format!("{tx_packets}\n")).unwrap();
            fs::write(p.join("rx_errors"), format!("{rx_errors}\n")).unwrap();
            fs::write(p.join("tx_errors"), format!("{tx_errors}\n")).unwrap();
            fs::write(p.join("rx_dropped"), format!("{rx_dropped}\n")).unwrap();
            fs::write(p.join("tx_dropped"), format!("{tx_dropped}\n")).unwrap();
            let iface_root = p.parent().unwrap();
            fs::write(iface_root.join("operstate"), format!("{oper}\n")).unwrap();
            fs::write(iface_root.join("speed"), format!("{speed}\n")).unwrap();
        }
        dir
    }

    /// Shorthand helper for tests that only need the original 5 fields.
    /// Uses placeholder values ("0") for the 6 new statistics fields.
    fn fake_net_sysroot_legacy(interfaces: &[(&str, &str, &str, &str, &str)]) -> tempfile::TempDir {
        let extended: Vec<_> = interfaces
            .iter()
            .map(|(name, rx, tx, oper, speed)| {
                (*name, *rx, *tx, "0", "0", "0", "0", "0", "0", *oper, *speed)
            })
            .collect();
        fake_net_sysroot(&extended)
    }

    fn ctx_for(dir: &tempfile::TempDir) -> PluginCtx {
        PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap()
    }

    #[test]
    fn enumerate_synthetic_net() {
        let dir = fake_net_sysroot_legacy(&[
            ("lo", "1000", "2000", "up", "-1"),
            ("eth0", "10", "20", "up", "1000"),
        ]);
        // `lo` is a kernel software interface and is filtered out by
        // `enumerate()` — only physical / logical hardware interfaces
        // make it through.
        let ifaces = enumerate(Some(dir.path()), &[]).unwrap();
        assert_eq!(ifaces, vec!["eth0"]);
    }

    #[test]
    fn enumerate_drops_loopback() {
        let dir = fake_net_sysroot_legacy(&[("lo", "0", "0", "up", "-1")]);
        let ifaces = enumerate(Some(dir.path()), &[]).unwrap();
        assert!(ifaces.is_empty(), "lo should be filtered out, got {ifaces:?}");
    }

    #[test]
    fn init_advertises_ten_sensors_per_interface() {
        let dir = fake_net_sysroot_legacy(&[("eth0", "0", "0", "up", "1000")]);
        let p = NetPlugin::default();
        let m = host_init(&p, &ctx_for(&dir)).unwrap();
        assert_eq!(m.sensors.len(), 10);
    }

    #[test]
    fn sample_rx_tx_match_sysfs_values() {
        let dir = fake_net_sysroot_legacy(&[("eth0", "12345", "67890", "up", "1000")]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let rx = host_sample(&p, SensorId::new("net.eth0.rx_bytes")).unwrap();
        let tx = host_sample(&p, SensorId::new("net.eth0.tx_bytes")).unwrap();
        assert!(matches!(rx, Reading::Counter(12345)));
        assert!(matches!(tx, Reading::Counter(67890)));
    }

    #[test]
    fn sample_link_state_returns_operstate() {
        let dir = fake_net_sysroot_legacy(&[("eth0", "0", "0", "down", "-1")]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let s = host_sample(&p, SensorId::new("net.eth0.link_state")).unwrap();
        assert!(matches!(s, Reading::State(ref v) if v == "down"));
    }

    #[test]
    fn sample_speed_unknown_returns_minus_one() {
        let dir = fake_net_sysroot_legacy(&[("eth0", "0", "0", "up", "-1")]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let s = host_sample(&p, SensorId::new("net.eth0.speed_mbps")).unwrap();
        assert!(matches!(s, Reading::Scalar(v) if v == -1.0));
    }

    /// Verify all 6 extra statistics sensors are advertised in the manifest.
    #[test]
    fn manifest_includes_extra_stat_sensors() {
        let dir = fake_net_sysroot_legacy(&[("eth0", "0", "0", "up", "1000")]);
        let p = NetPlugin::default();
        let m = host_init(&p, &ctx_for(&dir)).unwrap();
        let ids: Vec<_> = m.sensors.iter().map(|s| s.id.as_str().to_owned()).collect();
        for metric in
            &["rx_packets", "tx_packets", "rx_errors", "tx_errors", "rx_dropped", "tx_dropped"]
        {
            let sid = format!("net.eth0.{metric}");
            assert!(ids.contains(&sid), "missing sensor {sid}");
        }
        // Also verify existing sensors are still present
        for metric in &["rx_bytes", "tx_bytes", "link_state", "speed_mbps"] {
            let sid = format!("net.eth0.{metric}");
            assert!(ids.contains(&sid), "missing sensor {sid}");
        }
    }

    /// Verify rx_packets and tx_packets return the values written to sysfs.
    #[test]
    fn sample_rx_tx_packets() {
        let dir = fake_net_sysroot(&[(
            "eth0", "100", "200", "42", "99", "0", "0", "0", "0", "up", "1000",
        )]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let rx_p = host_sample(&p, SensorId::new("net.eth0.rx_packets")).unwrap();
        let tx_p = host_sample(&p, SensorId::new("net.eth0.tx_packets")).unwrap();
        assert!(matches!(rx_p, Reading::Counter(42)));
        assert!(matches!(tx_p, Reading::Counter(99)));
    }

    /// Verify rx_errors, tx_errors, rx_dropped, tx_dropped return correct values.
    #[test]
    fn sample_error_and_drop_counters() {
        let dir = fake_net_sysroot(&[(
            "eth0", "100", "200", "42", "99", "3", "7", "1", "2", "up", "1000",
        )]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let rx_e = host_sample(&p, SensorId::new("net.eth0.rx_errors")).unwrap();
        let tx_e = host_sample(&p, SensorId::new("net.eth0.tx_errors")).unwrap();
        let rx_d = host_sample(&p, SensorId::new("net.eth0.rx_dropped")).unwrap();
        let tx_d = host_sample(&p, SensorId::new("net.eth0.tx_dropped")).unwrap();
        assert!(matches!(rx_e, Reading::Counter(3)));
        assert!(matches!(tx_e, Reading::Counter(7)));
        assert!(matches!(rx_d, Reading::Counter(1)));
        assert!(matches!(tx_d, Reading::Counter(2)));
    }

    #[test]
    fn manifest_emits_net_devices() {
        // One PCI-backed iface (enp4s0) and one purely-logical iface (wg0).
        // The fake_net_sysroot helper sets up the statistics tree; we then
        // augment enp4s0 with `device/vendor` and `device/device` files so
        // the PciIdDb branch runs.
        let dir = fake_net_sysroot(&[
            ("enp4s0", "0", "0", "0", "0", "0", "0", "0", "0", "up", "1000"),
            ("wg0", "0", "0", "0", "0", "0", "0", "0", "0", "unknown", "-1"),
        ]);
        let dev = dir.path().join("sys/class/net/enp4s0/device");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("vendor"), "0x8086\n").unwrap();
        fs::write(dev.join("device"), "0x125c\n").unwrap();

        let plugin = NetPlugin::default();
        let manifest = host_init(&plugin, &ctx_for(&dir)).unwrap();

        let enp = manifest
            .devices
            .iter()
            .find(|d| d.plugin_device_id == "enp4s0")
            .expect("enp4s0 device");
        assert_eq!(enp.key.as_str(), "net:enp4s0");
        assert_eq!(enp.category, linsight_core::HardwareCategory::Network);

        let wg = manifest.devices.iter().find(|d| d.plugin_device_id == "wg0").expect("wg0 device");
        assert_eq!(wg.key.as_str(), "net:wg0");
        assert!(wg.vendor.is_none());

        // Every emitted sensor must reference a manifest device.
        let keys: std::collections::HashSet<_> =
            manifest.devices.iter().map(|d| d.key.as_str().to_owned()).collect();
        for s in &manifest.sensors {
            let k = s.device_key.as_ref().expect("net sensors must have device_key");
            assert!(keys.contains(k.as_str()), "sensor key {k} not in manifest devices");
        }
    }

    #[test]
    fn sample_unknown_interface_errors() {
        let dir = fake_net_sysroot_legacy(&[("eth0", "0", "0", "up", "1000")]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let err = host_sample(&p, SensorId::new("net.ghost.rx_bytes")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn enumerate_respects_exclude_patterns() {
        let dir = fake_net_sysroot_legacy(&[
            ("eth0", "0", "0", "up", "1000"),
            ("docker0", "0", "0", "up", "0"),
            ("veth1234", "0", "0", "up", "0"),
        ]);
        let exclude = vec!["docker*".into(), "veth*".into()];
        let ifaces = enumerate(Some(dir.path()), &exclude).unwrap();
        assert_eq!(ifaces, vec!["eth0"]);
    }

    #[test]
    fn matches_exclude_handles_star_suffix() {
        let patterns = vec!["docker*".into(), "br-".into()];
        assert!(matches_exclude("docker0", &patterns));
        assert!(matches_exclude("br-abcd", &patterns));
        assert!(!matches_exclude("eth0", &patterns));
    }

    #[test]
    fn cache_reuses_snapshot_within_ttl() {
        let dir = fake_net_sysroot(&[(
            "eth0", "100", "200", "42", "99", "0", "0", "0", "0", "up", "1000",
        )]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();

        // First sample populates cache
        let r1 = host_sample(&p, SensorId::new("net.eth0.rx_bytes")).unwrap();
        assert!(matches!(r1, Reading::Counter(100)));

        // Mutate the sysfs files
        let stat_path = dir.path().join("sys/class/net/eth0/statistics/rx_bytes");
        fs::write(&stat_path, "999\n").unwrap();

        // Second sample immediately should still see cached value
        let r2 = host_sample(&p, SensorId::new("net.eth0.rx_bytes")).unwrap();
        assert!(matches!(r2, Reading::Counter(100)), "cache should serve stale value");
    }

    #[test]
    fn cache_expires_after_ttl() {
        let dir = fake_net_sysroot(&[(
            "eth0", "100", "200", "42", "99", "0", "0", "0", "0", "up", "1000",
        )]);
        let p = NetPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();

        // First sample
        let r1 = host_sample(&p, SensorId::new("net.eth0.rx_bytes")).unwrap();
        assert!(matches!(r1, Reading::Counter(100)));

        // Mutate the sysfs files
        let stat_path = dir.path().join("sys/class/net/eth0/statistics/rx_bytes");
        fs::write(&stat_path, "999\n").unwrap();

        // Wait for cache expiry
        std::thread::sleep(std::time::Duration::from_millis(60));

        // Second sample should see new value
        let r2 = host_sample(&p, SensorId::new("net.eth0.rx_bytes")).unwrap();
        assert!(matches!(r2, Reading::Counter(999)), "cache should reflect new value after expiry");
    }
}
