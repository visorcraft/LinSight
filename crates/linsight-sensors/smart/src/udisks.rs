// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! udisks2 D-Bus interface for SMART data.
//!
//! ATA drives expose `org.freedesktop.UDisks2.Drive.Ata`:
//! - SmartTemperature (Kelvin → °C)
//! - SmartFailing (bool → State)
//! - SmartPowerOnSeconds (seconds → hours)
//! - SmartNumAttributes (reallocated sectors as scalar)
//!
//! NVMe (udisks ≥ 2.10) exposes `org.freedesktop.UDisks2.NVMe.Controller`:
//! - SmartTemperature (Kelvin → °C)
//! - SmartPowerOnHours
//! - SmartPercentUsed (wear percentage)

use std::collections::HashMap;

use linsight_core::{Reading, SensorId, SensorKind, Unit};
use linsight_plugin_sdk::{PluginError, SensorDescriptor};

/// Extract SMART sensors from a udisks2 drive property map.
///
/// `props` is the interface property dictionary for either
/// `org.freedesktop.UDisks2.Drive.Ata` or `org.freedesktop.UDisks2.NVMe.Controller`.
pub fn sensors_from_drive(
    disk_name: &str,
    props: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> Result<Vec<(SensorId, SensorDescriptor, Reading)>, PluginError> {
    let mut out = Vec::new();

    // Temperature (both ATA and NVMe)
    if let Some(temp_k) = props.get("SmartTemperature").and_then(|v| v.downcast_ref::<f64>().ok()) {
        let temp_c = temp_k - 273.15;
        out.push(make_sensor(
            disk_name,
            "smart_temp_c",
            "SMART temperature",
            Unit::Celsius,
            SensorKind::Scalar,
            Reading::Scalar(temp_c),
        )?);
    }

    // ATA-specific fields
    if let Some(failing) = props.get("SmartFailing").and_then(|v| v.downcast_ref::<bool>().ok()) {
        out.push(make_sensor(
            disk_name,
            "smart_health",
            "SMART health",
            Unit::Custom(String::new()),
            SensorKind::State,
            Reading::State(if failing { "failing".into() } else { "ok".into() }),
        )?);
    }

    if let Some(secs) = props.get("SmartPowerOnSeconds").and_then(|v| v.downcast_ref::<u64>().ok())
    {
        out.push(make_sensor(
            disk_name,
            "smart_power_on_hours",
            "SMART power-on hours",
            Unit::Custom("h".into()),
            SensorKind::Scalar,
            Reading::Scalar((secs as f64) / 3600.0),
        )?);
    }

    if let Some(attrs) = props.get("SmartNumAttributes").and_then(|v| v.downcast_ref::<u64>().ok())
    {
        out.push(make_sensor(
            disk_name,
            "smart_realloc_sectors",
            "SMART reallocated sectors",
            Unit::Count,
            SensorKind::Scalar,
            Reading::Scalar(attrs as f64),
        )?);
    }

    // NVMe-specific fields (udisks ≥ 2.10)
    if let Some(hours) = props.get("SmartPowerOnHours").and_then(|v| v.downcast_ref::<u64>().ok()) {
        out.push(make_sensor(
            disk_name,
            "smart_power_on_hours",
            "SMART power-on hours",
            Unit::Custom("h".into()),
            SensorKind::Scalar,
            Reading::Scalar(hours as f64),
        )?);
    }

    if let Some(used) = props.get("SmartPercentUsed").and_then(|v| v.downcast_ref::<f64>().ok()) {
        out.push(make_sensor(
            disk_name,
            "smart_wear_pct",
            "SMART wear",
            Unit::Percent,
            SensorKind::Scalar,
            Reading::Scalar(used),
        )?);
    }

    Ok(out)
}

fn make_sensor(
    disk_name: &str,
    metric: &str,
    display_name: &str,
    unit: Unit,
    kind: SensorKind,
    reading: Reading,
) -> Result<(SensorId, SensorDescriptor, Reading), PluginError> {
    let id = SensorId::new(format!("disk.{disk_name}.{metric}"));
    let key = linsight_core::HardwareDeviceKey::try_new(format!("block:{disk_name}"))
        .map_err(|e| PluginError::Io(format!("block {disk_name} bad key: {e}")))?;
    let desc = SensorDescriptor {
        id: id.clone(),
        display_name: display_name.into(),
        unit,
        kind,
        category: linsight_core::Category::Storage,
        native_rate_hz: 0.2,
        min: Some(0.0),
        max: None,
        device_id: Some(disk_name.into()),
        device_key: Some(key),
        tags: vec![],
    };
    Ok((id, desc, reading))
}

/// Fetch all drives/controllers that expose SMART data.
///
/// Returns a map of kernel disk name → property dictionary for the
/// SMART-bearing interface (`Drive.Ata` or `NVMe.Controller`).
pub fn fetch_smart_drives()
-> Result<HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>>, String> {
    let conn =
        zbus::blocking::Connection::system().map_err(|e| format!("D-Bus connection: {e}"))?;

    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.UDisks2",
        "/org/freedesktop/UDisks2",
        "org.freedesktop.DBus.ObjectManager",
    )
    .map_err(|e| format!("ObjectManager proxy: {e}"))?;

    let managed_objects: HashMap<
        zbus::zvariant::OwnedObjectPath,
        HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>>,
    > = proxy
        .call_method("GetManagedObjects", &())
        .and_then(|m| m.body().deserialize())
        .map_err(|e| format!("GetManagedObjects: {e}"))?;

    let mut out = HashMap::new();
    for (_path, ifaces) in managed_objects {
        // Find the block device interface to get the kernel name
        let Some(block) = ifaces.get("org.freedesktop.UDisks2.Block") else {
            continue;
        };
        let Some(device_val) = block.get("Device") else {
            continue;
        };
        let disk_name = if let Ok(arr) = zbus::zvariant::Array::try_from(&**device_val) {
            let bytes: Vec<u8> = arr.iter().filter_map(|v| v.downcast_ref::<u8>().ok()).collect();
            String::from_utf8_lossy(&bytes)
                .trim_end_matches('\0')
                .trim_start_matches("/dev/")
                .to_string()
        } else {
            String::new()
        };
        if disk_name.is_empty() {
            continue;
        }

        // Check for ATA SMART interface
        if let Some(ata) = ifaces.get("org.freedesktop.UDisks2.Drive.Ata")
            && (ata.contains_key("SmartTemperature") || ata.contains_key("SmartFailing"))
        {
            out.insert(disk_name, ata.clone());
            continue;
        }

        // Check for NVMe SMART interface
        if let Some(nvme) = ifaces.get("org.freedesktop.UDisks2.NVMe.Controller")
            && (nvme.contains_key("SmartTemperature") || nvme.contains_key("SmartPercentUsed"))
        {
            out.insert(disk_name, nvme.clone());
            continue;
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;

    fn owned(v: impl Into<OwnedValue>) -> OwnedValue {
        v.into()
    }

    #[test]
    fn ata_drive_maps_temp_failing_and_realloc() {
        let mut props = HashMap::new();
        props.insert("SmartTemperature".to_string(), owned(305.15_f64));
        props.insert("SmartFailing".to_string(), owned(false));
        props.insert("SmartPowerOnSeconds".to_string(), owned(3600_u64));
        props.insert("SmartNumAttributes".to_string(), owned(0_u64));

        let sensors = sensors_from_drive("sda", &props).unwrap();
        assert_eq!(sensors.len(), 4);

        let (id, _, reading) = &sensors[0];
        assert_eq!(id.as_str(), "disk.sda.smart_temp_c");
        assert!(matches!(reading, Reading::Scalar(v) if (*v - 32.0).abs() < 0.01));

        let (id, _, reading) = &sensors[1];
        assert_eq!(id.as_str(), "disk.sda.smart_health");
        assert!(matches!(reading, Reading::State(v) if v == "ok"));

        let (id, _, reading) = &sensors[3];
        assert_eq!(id.as_str(), "disk.sda.smart_realloc_sectors");
        assert!(matches!(reading, Reading::Scalar(v) if *v == 0.0));
    }

    #[test]
    fn nvme_controller_maps_wear_and_power_on_hours() {
        let mut props = HashMap::new();
        props.insert("SmartTemperature".to_string(), owned(300.15_f64));
        props.insert("SmartPowerOnHours".to_string(), owned(5000_u64));
        props.insert("SmartPercentUsed".to_string(), owned(3.0_f64));

        let sensors = sensors_from_drive("nvme0n1", &props).unwrap();
        assert_eq!(sensors.len(), 3);

        let (id, _, reading) = &sensors[0];
        assert_eq!(id.as_str(), "disk.nvme0n1.smart_temp_c");
        assert!(matches!(reading, Reading::Scalar(v) if (*v - 27.0).abs() < 0.01));

        let (id, _, reading) = &sensors[1];
        assert_eq!(id.as_str(), "disk.nvme0n1.smart_power_on_hours");
        assert!(matches!(reading, Reading::Scalar(v) if *v == 5000.0));

        let (id, _, reading) = &sensors[2];
        assert_eq!(id.as_str(), "disk.nvme0n1.smart_wear_pct");
        assert!(matches!(reading, Reading::Scalar(v) if *v == 3.0));
    }

    #[test]
    fn missing_smart_support_yields_no_sensors() {
        let props = HashMap::new();
        let sensors = sensors_from_drive("sda", &props).unwrap();
        assert!(sensors.is_empty());
    }
}
