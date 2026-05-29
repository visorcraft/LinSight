// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Parser for the kernel's `pci.ids` database (`/usr/share/hwdata/
//! pci.ids` on Arch/Debian/Fedora/SUSE; `/usr/share/misc/pci.ids` on
//! Debian as an alternative).
//!
//! Format:
//! ```text
//! vendor_hex  Vendor Name
//! \tdevice_hex  Device Name
//! \t\tsubvendor_hex subdevice_hex  Subsystem Name
//! ```
//! We only parse the vendor and device levels; subsystem lines are
//! skipped. Lines starting with `#` and blank lines are ignored.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Default)]
pub struct PciIdDb {
    vendors: HashMap<u16, String>,
    devices: HashMap<(u16, u16), String>,
}

impl PciIdDb {
    pub fn parse(text: &str) -> Self {
        let mut db = Self::default();
        let mut current_vendor: Option<u16> = None;
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if line.starts_with("\t\t") {
                continue;
            }
            if let Some(rest) = line.strip_prefix('\t') {
                let Some(vendor) = current_vendor else { continue };
                let mut parts = rest.splitn(2, char::is_whitespace);
                let Some(dev_hex) = parts.next() else { continue };
                let name = parts.next().map(str::trim).unwrap_or("");
                if let Ok(dev) = u16::from_str_radix(dev_hex, 16) {
                    db.devices.insert((vendor, dev), name.to_owned());
                }
                continue;
            }
            let mut parts = line.splitn(2, char::is_whitespace);
            let Some(vendor_hex) = parts.next() else { continue };
            let name = parts.next().map(str::trim).unwrap_or("");
            if let Ok(v) = u16::from_str_radix(vendor_hex, 16) {
                current_vendor = Some(v);
                db.vendors.insert(v, name.to_owned());
            } else {
                current_vendor = None;
            }
        }
        db
    }

    pub fn lookup(&self, vendor: u16, device: u16) -> Option<String> {
        self.devices.get(&(vendor, device)).cloned()
    }

    pub fn vendor_name(&self, vendor: u16) -> Option<String> {
        self.vendors.get(&vendor).cloned()
    }

    /// Load from the canonical path, falling back to the Debian-alternate
    /// path. Returns an empty DB if neither file exists.
    pub fn load_default() -> Self {
        for p in ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"] {
            if let Ok(text) = std::fs::read_to_string(Path::new(p)) {
                return Self::parse(&text);
            }
        }
        Self::default()
    }

    /// Process-wide cached default DB. First call parses; subsequent
    /// calls return the cached reference. Plugins that don't need PCI
    /// lookup never trigger the parse.
    pub fn shared() -> &'static Self {
        static CELL: OnceLock<PciIdDb> = OnceLock::new();
        CELL.get_or_init(Self::load_default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/mini-pci.ids");

    #[test]
    fn parses_fixture_into_lookup_table() {
        let db = PciIdDb::parse(FIXTURE);
        assert_eq!(db.lookup(0x8086, 0xe223).as_deref(), Some("Battlemage [Arc B-series]"));
        assert_eq!(db.lookup(0x8086, 0xb0a0).as_deref(), Some("Arrow Lake-S iGPU"));
        assert_eq!(db.lookup(0x10de, 0x2782).as_deref(), Some("GeForce RTX 4070"));
    }

    #[test]
    fn lookup_misses_return_none() {
        let db = PciIdDb::parse(FIXTURE);
        assert!(db.lookup(0x8086, 0x0000).is_none());
        assert!(db.lookup(0xdead, 0xbeef).is_none());
    }

    #[test]
    fn vendor_name_lookup() {
        let db = PciIdDb::parse(FIXTURE);
        assert_eq!(db.vendor_name(0x8086).as_deref(), Some("Intel Corporation"));
        assert_eq!(db.vendor_name(0x10de).as_deref(), Some("NVIDIA Corporation"));
    }

    #[test]
    fn parse_skips_subdevice_lines() {
        let s = "8086  Intel\n\te223  Battlemage\n\t\t1234 5678  Some subsystem\n";
        let db = PciIdDb::parse(s);
        assert_eq!(db.lookup(0x8086, 0xe223).as_deref(), Some("Battlemage"));
    }
}
