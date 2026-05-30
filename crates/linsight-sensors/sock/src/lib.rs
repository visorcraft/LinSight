// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

//! Socket statistics sensor backend.
//!
//! System-wide socket telemetry derived from `/proc/net`:
//! * `sock.tcp_established` — TCP connections in the ESTABLISHED state
//! * `sock.tcp_listen` — TCP sockets in the LISTEN state
//! * `sock.tcp_time_wait` — TCP sockets in the TIME_WAIT state
//! * `sock.udp_inuse` — UDP sockets in use
//! * `sock.tcp_mem_bytes` — kernel memory committed to TCP sockets
//!
//! The three TCP counts are derived by tallying the per-connection state
//! field (column 4, `st`) across `/proc/net/tcp` and `/proc/net/tcp6`.
//! `udp_inuse` and the TCP memory figure come from the summary in
//! `/proc/net/sockstat` (the `mem` field is page-denominated; we report
//! it in bytes). All five are instantaneous gauges, not counters.

use std::fs;
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

/// Linux memory accounting in `/proc/net/sockstat` is denominated in
/// pages. LinSight targets x86_64 / aarch64 where the base page is 4 KiB;
/// we multiply by this constant to report `sock.tcp_mem_bytes` in bytes.
const PAGE_SIZE: u64 = 4096;

#[derive(Default)]
pub struct SockPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
}

/// TCP connection counts tallied by state across `/proc/net/tcp{,6}`.
#[derive(Default, Debug, PartialEq, Eq)]
struct TcpStates {
    established: u64,
    listen: u64,
    time_wait: u64,
}

/// Summary figures pulled from `/proc/net/sockstat`.
#[derive(Default, Debug, PartialEq, Eq)]
struct SockStat {
    udp_inuse: u64,
    tcp_mem_pages: u64,
}

impl SockPlugin {
    fn init_inner(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("SockPlugin poisoned");
        inner.sysroot = ctx.sysroot().map(|p| p.to_path_buf());

        let key = HardwareDeviceKey::try_new("system:sock")
            .map_err(|e| PluginError::Manifest(e.to_string()))?;
        let device_id = Some("sock".to_string());

        let specs: &[(&str, &str, Unit)] = &[
            ("tcp_established", "TCP established connections", Unit::Count),
            ("tcp_listen", "TCP listening sockets", Unit::Count),
            ("tcp_time_wait", "TCP time-wait sockets", Unit::Count),
            ("udp_inuse", "UDP sockets in use", Unit::Count),
            ("tcp_mem_bytes", "TCP socket memory", Unit::Bytes),
        ];

        let sensors = specs
            .iter()
            .map(|(metric, display, unit)| SensorDescriptor {
                id: SensorId::new(format!("sock.{metric}")),
                display_name: (*display).to_string(),
                unit: unit.clone(),
                kind: SensorKind::Scalar,
                category: Category::Network,
                native_rate_hz: 0.5,
                min: Some(0.0),
                max: None,
                device_id: device_id.clone(),
                device_key: Some(key.clone()),
                tags: vec![],
            })
            .collect();

        Ok(PluginManifest {
            plugin_id: "io.visorcraft.linsight.sock".into(),
            display_name: "Sockets".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors,
            devices: vec![HardwareDevice {
                key,
                category: HardwareCategory::Network,
                model: "Network Sockets".into(),
                vendor: None,
                location: None,
                plugin_id: "io.visorcraft.linsight.sock".into(),
                plugin_device_id: "sock".into(),
                sensor_ids: vec![],
            }],
        })
    }

    fn sample_inner(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        let inner = self.inner.lock().expect("SockPlugin poisoned");
        let metric = sensor
            .as_str()
            .strip_prefix("sock.")
            .ok_or_else(|| PluginError::Unsupported(sensor.to_string()))?;
        let net = net_dir(inner.sysroot.as_deref());
        match metric {
            "tcp_established" => Ok(Reading::Scalar(tcp_states(&net).established as f64)),
            "tcp_listen" => Ok(Reading::Scalar(tcp_states(&net).listen as f64)),
            "tcp_time_wait" => Ok(Reading::Scalar(tcp_states(&net).time_wait as f64)),
            "udp_inuse" => Ok(Reading::Scalar(
                parse_sockstat(&read_opt(&net.join("sockstat"))).udp_inuse as f64,
            )),
            "tcp_mem_bytes" => {
                let pages = parse_sockstat(&read_opt(&net.join("sockstat"))).tcp_mem_pages;
                Ok(Reading::Scalar((pages * PAGE_SIZE) as f64))
            }
            _ => Err(PluginError::Unsupported(sensor.to_string())),
        }
    }
}

impl LinsightPlugin for SockPlugin {
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

fn net_dir(sysroot: Option<&Path>) -> PathBuf {
    match sysroot {
        Some(r) => r.join("proc/net"),
        None => PathBuf::from("/proc/net"),
    }
}

/// Read a file, returning an empty string if it is absent. `/proc/net`
/// files are always present on Linux, but a synthetic sysroot (tests) or
/// a `/proc`-less sandbox may lack them; treating "missing" as "no
/// sockets" keeps the sensor reporting 0 rather than erroring.
fn read_opt(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Tally TCP connection states across both the IPv4 and IPv6 tables.
fn tcp_states(net: &Path) -> TcpStates {
    let mut s = count_tcp_states(&read_opt(&net.join("tcp")));
    let v6 = count_tcp_states(&read_opt(&net.join("tcp6")));
    s.established += v6.established;
    s.listen += v6.listen;
    s.time_wait += v6.time_wait;
    s
}

/// Count ESTABLISHED (`01`), LISTEN (`0A`) and TIME_WAIT (`06`) entries in
/// a `/proc/net/tcp`-format table. The first line is the column header.
/// The connection state is the 4th whitespace-separated field (`st`).
fn count_tcp_states(content: &str) -> TcpStates {
    let mut s = TcpStates::default();
    for line in content.lines().skip(1) {
        match line.split_whitespace().nth(3) {
            Some("01") => s.established += 1,
            Some("0A") => s.listen += 1,
            Some("06") => s.time_wait += 1,
            _ => {}
        }
    }
    s
}

/// Parse the `UDP: inuse N` and `TCP: ... mem N` figures out of
/// `/proc/net/sockstat`.
fn parse_sockstat(content: &str) -> SockStat {
    let mut stat = SockStat::default();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("TCP:")
            && let Some(v) = field_after(rest, "mem")
        {
            stat.tcp_mem_pages = v;
        } else if let Some(rest) = line.strip_prefix("UDP:")
            && let Some(v) = field_after(rest, "inuse")
        {
            stat.udp_inuse = v;
        }
    }
    stat
}

/// In a whitespace-separated token stream, find `key` and parse the token
/// immediately after it as a `u64`.
fn field_after(s: &str, key: &str) -> Option<u64> {
    let mut tokens = s.split_whitespace();
    tokens.by_ref().position(|t| t == key)?;
    tokens.next().and_then(|v| v.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    use linsight_plugin_sdk::{host_init, host_sample};

    const SOCKSTAT: &str = "sockets: used 1234\n\
TCP: inuse 10 orphan 0 tw 5 alloc 20 mem 7\n\
UDP: inuse 8 mem 2\n\
UDPLITE: inuse 0\n\
RAW: inuse 0\n\
FRAG: inuse 0 memory 0\n";

    // Header line + three connections: one LISTEN (0A), one ESTABLISHED
    // (01), one TIME_WAIT (06).
    const TCP4: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid\n\
   0: 0100007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000   101\n\
   1: 0100007F:1F90 0100007F:C3A2 01 00000000:00000000 00:00000000 00000000  1000\n\
   2: 0100007F:1F90 0100007F:C3A3 06 00000000:00000000 00:00000000 00000000  1000\n";

    // One additional ESTABLISHED connection over IPv6.
    const TCP6: &str = "  sl  local_address                         remote_address                        st\n\
   0: 00000000000000000000000000000000:0050 00000000000000000000000000000000:0000 01\n";

    fn fake_sysroot(sockstat: &str, tcp4: &str, tcp6: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let net = dir.path().join("proc/net");
        fs::create_dir_all(&net).unwrap();
        fs::write(net.join("sockstat"), sockstat).unwrap();
        fs::write(net.join("tcp"), tcp4).unwrap();
        fs::write(net.join("tcp6"), tcp6).unwrap();
        dir
    }

    fn ctx_for(dir: &tempfile::TempDir) -> PluginCtx {
        PluginCtx::new_with_sysroot(dir.path().to_path_buf()).unwrap()
    }

    #[test]
    fn parse_sockstat_extracts_tcp_mem_and_udp_inuse() {
        let stat = parse_sockstat(SOCKSTAT);
        assert_eq!(stat, SockStat { udp_inuse: 8, tcp_mem_pages: 7 });
    }

    #[test]
    fn count_tcp_states_counts_each_state() {
        let s = count_tcp_states(TCP4);
        assert_eq!(s, TcpStates { established: 1, listen: 1, time_wait: 1 });
    }

    #[test]
    fn init_advertises_five_socket_sensors() {
        let dir = fake_sysroot(SOCKSTAT, TCP4, TCP6);
        let m = host_init(&SockPlugin::default(), &ctx_for(&dir)).unwrap();
        let ids: Vec<&str> = m.sensors.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(m.sensors.len(), 5);
        for want in [
            "sock.tcp_established",
            "sock.tcp_listen",
            "sock.tcp_time_wait",
            "sock.udp_inuse",
            "sock.tcp_mem_bytes",
        ] {
            assert!(ids.contains(&want), "missing sensor {want}");
        }
        assert_eq!(m.devices.len(), 1);
        assert_eq!(m.devices[0].key.as_str(), "system:sock");
    }

    #[test]
    fn sample_tcp_established_sums_ipv4_and_ipv6() {
        let dir = fake_sysroot(SOCKSTAT, TCP4, TCP6);
        let p = SockPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        // One ESTABLISHED in tcp4 + one in tcp6 = 2.
        let r = host_sample(&p, SensorId::new("sock.tcp_established")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 2.0), "got {r:?}");
    }

    #[test]
    fn sample_tcp_listen_and_time_wait() {
        let dir = fake_sysroot(SOCKSTAT, TCP4, TCP6);
        let p = SockPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let listen = host_sample(&p, SensorId::new("sock.tcp_listen")).unwrap();
        let tw = host_sample(&p, SensorId::new("sock.tcp_time_wait")).unwrap();
        assert!(matches!(listen, Reading::Scalar(v) if v == 1.0));
        assert!(matches!(tw, Reading::Scalar(v) if v == 1.0));
    }

    #[test]
    fn sample_udp_inuse_from_sockstat() {
        let dir = fake_sysroot(SOCKSTAT, TCP4, TCP6);
        let p = SockPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let r = host_sample(&p, SensorId::new("sock.udp_inuse")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 8.0), "got {r:?}");
    }

    #[test]
    fn sample_tcp_mem_bytes_is_pages_times_page_size() {
        let dir = fake_sysroot(SOCKSTAT, TCP4, TCP6);
        let p = SockPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let r = host_sample(&p, SensorId::new("sock.tcp_mem_bytes")).unwrap();
        // 7 pages * 4096 = 28672 bytes.
        assert!(matches!(r, Reading::Scalar(v) if v == 28672.0), "got {r:?}");
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let dir = fake_sysroot(SOCKSTAT, TCP4, TCP6);
        let p = SockPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let err = host_sample(&p, SensorId::new("sock.bogus")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }

    #[test]
    fn missing_proc_files_sample_zero() {
        // Empty sysroot: no /proc/net at all. Sensors must still read 0.
        let dir = tempfile::TempDir::new().unwrap();
        let p = SockPlugin::default();
        host_init(&p, &ctx_for(&dir)).unwrap();
        let est = host_sample(&p, SensorId::new("sock.tcp_established")).unwrap();
        let mem = host_sample(&p, SensorId::new("sock.tcp_mem_bytes")).unwrap();
        assert!(matches!(est, Reading::Scalar(v) if v == 0.0));
        assert!(matches!(mem, Reading::Scalar(v) if v == 0.0));
    }
}
