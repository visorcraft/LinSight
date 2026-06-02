# Storage Nested Mount-Points Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the Storage page, nest each mount-point section inside the physical disk it resides on (e.g. `btrfs (/home)` as an inset card within the **Samsung 990 Pro** section).

**Architecture:** The daemon's `fs` plugin resolves each mount's backing block device up to the physical-device id the disk/nvme plugins use, and tags the mount's sensors with `parent:<id>` (riding the existing, already-plumbed `tags` channel — no new protocol/FFI field). The GUI extracts that tag into a `parentDevice` field on each tile's JSON, then renders the Storage page as disk cards with inset mount sub-cards.

**Tech Stack:** Rust (workspace crates `linsight-sensors-fs`, `apps/linsight-gui` cxx-qt bridge), QML (Qt 6 / Kirigami), `tempfile` for sysroot test fixtures.

---

## Why a tag, not a new field

`SensorInfo.tags: Vec<String>` is already carried end-to-end: `SensorDescriptor.tags` → stabby FFI mirror (`crates/linsight-plugin-sdk/src/manifest.rs:89`) → `build_sensor_info_list` copies `tags: d.tags.clone()` (`apps/linsightd/src/transport/unix.rs:253`) → `overview_model.rs:256` already reads `info.tags`. The field's own comment says *"for filtering and grouping in the UI."* Riding it avoids editing `HardwareDevice`, `SensorDescriptor`, `SensorInfo`, the protocol, and the FFI mirror.

## File Structure

- `crates/linsight-sensors/fs/src/plugin.rs` — add resolver fns + tests; extend `read_mtab` to capture the source device; tag fs sensors with `parent:<id>`.
- `apps/linsight-gui/src/qobjects/overview_model.rs` — add `parent_device` to `TileJson` (serialized `parentDevice`), a `parent_device_for(info)` helper + test, and set it at every tile construction/refresh site.
- `apps/linsight-gui/qml/CategoryPage.qml` — add `parseBytes()` + `storageSections` nested model + switch the page body between the existing flat grid and the new nested view.
- `apps/linsight-gui/qml/StorageSectionView.qml` — **new** — renders disk cards with inset mount sub-cards.

---

## Task 1: fs resolver functions (`resolve_parent_device` + helpers)

**Files:**
- Modify: `crates/linsight-sensors/fs/src/plugin.rs` (add fns after `read_mtab`, ~line 102; add tests into the existing `#[cfg(test)] mod tests` at line 282)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests { ... }` block in `crates/linsight-sensors/fs/src/plugin.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p linsight-sensors-fs resolve 2>&1 | tail -20; cargo test -p linsight-sensors-fs nvme 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'resolve_parent_device'` / `'nvme_controller'`.

- [ ] **Step 3: Implement the resolver**

Insert these functions in `crates/linsight-sensors/fs/src/plugin.rs` immediately after `read_mtab` (after its closing `}`, ~line 102):

```rust
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
    if let Some(rest) = disk.strip_prefix("nvme") {
        if let Some(npos) = rest.find('n') {
            let ctrl = &rest[..npos];
            if !ctrl.is_empty() && ctrl.chars().all(|c| c.is_ascii_digit()) {
                return format!("nvme{ctrl}");
            }
        }
    }
    disk.to_owned()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p linsight-sensors-fs 2>&1 | tail -20`
Expected: PASS — all five new tests green, existing fs tests still green.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-sensors/fs/src/plugin.rs
git commit -m "fs: resolve a mount's backing block device to its physical disk id"
```

---

## Task 2: Tag fs sensors with their parent device

**Files:**
- Modify: `crates/linsight-sensors/fs/src/plugin.rs` — `read_mtab` (line 65), `init_inner` loop (lines 120-202)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block:

```rust
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
```

Note: `PluginCtx` is already imported in the plugin module; if the test block lacks it, add `use linsight_plugin_sdk::PluginCtx;` inside `mod tests`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p linsight-sensors-fs fs_sensors_carry_parent_tag 2>&1 | tail -20`
Expected: FAIL — `home.tags` is empty (`tags: vec![]`).

- [ ] **Step 3: Capture the source device in `read_mtab`**

Change `read_mtab`'s signature and body in `crates/linsight-sensors/fs/src/plugin.rs`. Replace the function header (line 65) and the push (the `out.push((f[1].to_owned(), f[2].to_owned()))` line) so it returns `(source, mountpoint, fstype)`:

```rust
fn read_mtab(sysroot: Option<&Path>) -> Vec<(String, String, String)> {
```

and the push line becomes:

```rust
        out.push((f[0].to_owned(), f[1].to_owned(), f[2].to_owned()));
```

- [ ] **Step 4: Compute and attach the parent tag in `init_inner`**

In `init_inner`, the loop header currently reads:

```rust
        for (mtab_idx, (mountpoint, fstype)) in mtab.iter().enumerate() {
```

Replace it with (now destructuring the source and computing the tag once per mount):

```rust
        for (mtab_idx, (source, mountpoint, fstype)) in mtab.iter().enumerate() {
            let parent_tags: Vec<String> =
                resolve_parent_device(source, inner.sysroot.as_deref())
                    .map(|p| vec![format!("parent:{p}")])
                    .unwrap_or_default();
```

(Keep the existing `let base = mount_safekey(mountpoint);` etc. that follow.)

Then change each of the **five** `SensorDescriptor { ... tags: vec![], }` literals (ids `total_bytes`, `used_bytes`, `avail_bytes`, `inodes_total`, `inodes_used`) so the `tags` line reads:

```rust
                tags: parent_tags.clone(),
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p linsight-sensors-fs 2>&1 | tail -25`
Expected: PASS — new test green; existing fs tests green. If an existing `read_mtab` test exists and now fails on the tuple arity, update its expected tuples to 3-element `(source, mountpoint, fstype)`.

- [ ] **Step 6: Commit**

```bash
git add crates/linsight-sensors/fs/src/plugin.rs
git commit -m "fs: tag each mount's sensors with parent:<disk-id>"
```

---

## Task 3: Surface `parentDevice` on the tile JSON

**Files:**
- Modify: `apps/linsight-gui/src/qobjects/overview_model.rs` — `TileJson` struct (lines 114-132), tile construction (~line 265), refresh site (~line 344), add `parent_device_for` near `device_label_for` (~line 640), add a test in the tests module.

- [ ] **Step 1: Write the failing test**

Add to the test module at the bottom of `apps/linsight-gui/src/qobjects/overview_model.rs` (the module that contains `tile_json_carries_device_label_as_separate_line_not_concatenated`):

```rust
    #[test]
    fn parent_device_extracted_from_parent_tag() {
        let info = linsight_protocol::SensorInfo {
            id: SensorId::new("fs.home.used_bytes"),
            display_name: "Filesystem used".into(),
            unit: Unit::Bytes,
            kind: SensorKind::Scalar,
            category: Category::Storage,
            native_rate_hz: 1.0,
            min: Some(0.0),
            max: None,
            device_id: Some("home".into()),
            plugin_id: "com.visorcraft.linsight.fs".into(),
            device_key: Some("fs:home".into()),
            device_label: Some("btrfs (/home)".into()),
            tags: vec!["parent:nvme0".into()],
        };
        assert_eq!(super::parent_device_for(&info), Some("nvme0".to_owned()));

        let mut no_parent = info.clone();
        no_parent.tags = vec!["static".into()];
        assert_eq!(super::parent_device_for(&no_parent), None);
    }
```

If the test module's imports don't already cover `SensorId`, `Unit`, `SensorKind`, `Category`, add the matching `use` lines that the sibling test uses (copy them from the existing `tile_json_carries_device_label_*` test's scope).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p linsight --lib parent_device 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'parent_device_for'`.

- [ ] **Step 3: Add the helper**

Immediately after `device_label_for` (the function ending ~line 645) in `apps/linsight-gui/src/qobjects/overview_model.rs`, add:

```rust
/// Extract the backing physical-disk id from a sensor's `parent:<id>` tag
/// (set by the fs plugin). `None` for sensors with no such tag.
fn parent_device_for(info: &linsight_protocol::SensorInfo) -> Option<String> {
    info.tags.iter().find_map(|t| t.strip_prefix("parent:").map(|s| s.to_owned()))
}
```

- [ ] **Step 4: Add the field to `TileJson`**

In the `struct TileJson` definition, add this field immediately after the `device_label` field (line ~124):

```rust
    #[serde(rename = "parentDevice", skip_serializing_if = "Option::is_none")]
    parent_device: Option<String>,
```

- [ ] **Step 5: Set the field at every construction + refresh site**

Run `grep -n 'TileJson {' apps/linsight-gui/src/qobjects/overview_model.rs` to list construction sites. In the main build loop (the `TileJson { id, category, device: info.device_id.clone(), ... }` literal, ~line 265), add:

```rust
                    parent_device: parent_device_for(info),
```

Then find the catalogue-refresh site (~line 344, where existing tiles are updated via `tile.name = info.display_name.clone(); tile.device_label = device_label_for(info);`) and add alongside it:

```rust
                                tile.parent_device = parent_device_for(info);
```

If `grep` reveals any other non-test `TileJson { ... }` literal, add `parent_device: parent_device_for(info),` there too (or `parent_device: None,` if no `info` is in scope, e.g. a synthetic tile).

- [ ] **Step 6: Run the test + a serialization smoke check**

Run: `cargo test -p linsight --lib parent_device 2>&1 | tail -20`
Expected: PASS.
Then `cargo build -p linsight 2>&1 | tail -5` — expected: no errors (all `TileJson` literals updated).

- [ ] **Step 7: Commit**

```bash
git add apps/linsight-gui/src/qobjects/overview_model.rs
git commit -m "gui(model): expose parentDevice on tile JSON from the parent: tag"
```

---

## Task 4: Build the nested storage model in QML

**Files:**
- Modify: `apps/linsight-gui/qml/CategoryPage.qml` — add `parseBytes()` (near `isUnknownSentinel`, ~line 170) and `storageSections` (after `tilesArray`, ~line 168)

- [ ] **Step 1: Add a byte-parse helper**

In `apps/linsight-gui/qml/CategoryPage.qml`, next to `function isUnknownSentinel(v) { ... }`, add:

```qml
    // Parse a formatted capacity string ("2 TB", "512 GB") to a GB-scaled
    // number for ordering. Returns -1 when unparseable.
    function parseBytes(v) {
        if (typeof v !== "string") return -1
        const m = v.match(/^(\d+(\.\d+)?)\s*(TB|TiB|GB|GiB|MB|MiB|KB|KiB|B)$/)
        if (!m) return -1
        let val = parseFloat(m[1]); const u = m[3]
        if (u === "TB" || u === "TiB") val *= 1024
        else if (u === "GB" || u === "GiB") val *= 1
        else if (u === "MB" || u === "MiB") val /= 1024
        else if (u === "KB" || u === "KiB") val /= (1024 * 1024)
        else if (u === "B") val /= (1024 * 1024 * 1024)
        return val
    }
```

- [ ] **Step 2: Add the `storageSections` computed property**

After the `readonly property var tilesArray: { ... }` block (closing `}` ~line 168), add:

```qml
    // Nested model for the Storage page: physical-disk sections (capacity
    // desc), each with its own sensors plus inset mount sub-sections; mounts
    // with no resolved disk become their own top-level "orphan" sections.
    readonly property var storageSections: {
        if (page.category !== "storage" || !page.dashModel) return []
        try {
            const raw = JSON.parse(page.dashModel.tilesJson || "[]")
            const tiles = raw.filter(t => t.category === "storage"
                && !(typeof t.value === "string" && page.isUnknownSentinel(t.value)))

            const disks = new Map()    // device -> { device, label, capacity, ownTiles: [] }
            const fsTiles = []
            for (const t of tiles) {
                if (t.id && String(t.id).indexOf("fs.") === 0) { fsTiles.push(t); continue }
                const dev = t.device || ""
                if (dev === "") continue
                if (!disks.has(dev)) disks.set(dev, { device: dev, label: t.deviceLabel || dev, capacity: -1, ownTiles: [] })
                const d = disks.get(dev)
                d.ownTiles.push(t)
                if (t.id && String(t.id).endsWith("capacity_bytes")) d.capacity = page.parseBytes(t.value)
            }

            const diskMounts = new Map()   // device -> Map(mountLabel -> { label, tiles: [] })
            const orphanGroups = new Map() // mountLabel -> { label, tiles: [] }
            for (const t of fsTiles) {
                const parent = t.parentDevice || ""
                const mountLabel = t.deviceLabel || ""
                if (parent !== "" && disks.has(parent)) {
                    if (!diskMounts.has(parent)) diskMounts.set(parent, new Map())
                    const mm = diskMounts.get(parent)
                    if (!mm.has(mountLabel)) mm.set(mountLabel, { label: mountLabel, tiles: [] })
                    mm.get(mountLabel).tiles.push(t)
                } else {
                    if (!orphanGroups.has(mountLabel)) orphanGroups.set(mountLabel, { label: mountLabel, tiles: [] })
                    orphanGroups.get(mountLabel).tiles.push(t)
                }
            }

            const byName = (a, b) => (a.name || "").localeCompare(b.name || "")

            const diskSections = Array.from(disks.values()).map(d => {
                const mm = diskMounts.get(d.device) || new Map()
                const mounts = Array.from(mm.values())
                    .sort((a, b) => a.label.localeCompare(b.label))
                    .map(m => ({ label: m.label, tiles: m.tiles.slice().sort(byName) }))
                return { kind: "disk", device: d.device, label: d.label,
                         capacity: d.capacity, ownTiles: d.ownTiles.slice().sort(byName), mounts: mounts }
            })
            diskSections.sort((a, b) => a.capacity !== b.capacity ? b.capacity - a.capacity
                                                                   : a.label.localeCompare(b.label))

            const orphanSections = Array.from(orphanGroups.values())
                .sort((a, b) => a.label.localeCompare(b.label))
                .map(o => ({ kind: "orphan", device: "", label: o.label, capacity: -1,
                             ownTiles: o.tiles.slice().sort(byName), mounts: [] }))

            return diskSections.concat(orphanSections)
        } catch (e) {
            return []
        }
    }

    readonly property bool nested: page.category === "storage"
```

- [ ] **Step 3: Verify it parses (build the GUI)**

Run: `cargo build -p linsight 2>&1 | tail -5`
Expected: builds (QML is compiled by cxx-qt-build; a syntax error here fails the build). Full visual check happens in Task 6.

- [ ] **Step 4: Commit**

```bash
git add apps/linsight-gui/qml/CategoryPage.qml
git commit -m "gui(storage): compute nested disk/mount section model"
```

---

## Task 5: Render nested sections + wire the page body

**Files:**
- Create: `apps/linsight-gui/qml/StorageSectionView.qml`
- Modify: `apps/linsight-gui/qml/CategoryPage.qml` — replace the `Controls.ScrollView { GridLayout {...} }` body (lines 218-293) with a loader that switches flat vs. nested

- [ ] **Step 1: Create `StorageSectionView.qml`**

Create `apps/linsight-gui/qml/StorageSectionView.qml` with exactly:

```qml
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// Scrollable column of physical-disk cards. Each disk card shows the disk's
// own sensors, then inset cards for the mounts that live on it. Orphan
// sections render as a plain card (no disk chrome, no mounts).
Controls.ScrollView {
    id: view
    property var sections: []
    clip: true
    contentWidth: availableWidth

    ColumnLayout {
        width: view.availableWidth
        spacing: app.tokens.spaceL

        Repeater {
            model: view.sections
            delegate: Rectangle {
                required property var modelData
                Layout.fillWidth: true
                Layout.preferredHeight: card.implicitHeight + app.tokens.spaceL * 2
                radius: app.tokens.radiusCard
                color: app.tokens.surface1

                ColumnLayout {
                    id: card
                    anchors.fill: parent
                    anchors.margins: app.tokens.spaceL
                    spacing: app.tokens.spaceM

                    RowLayout {
                        Layout.fillWidth: true
                        Controls.Label {
                            text: modelData.label
                            font.pixelSize: app.tokens.textBody
                            font.weight: app.tokens.weightBold
                            font.family: app.tokens.sansFamily
                        }
                        Item { Layout.fillWidth: true }
                        Controls.Label {
                            visible: modelData.kind === "disk"
                            text: {
                                var cap = ""
                                for (var i = 0; i < modelData.ownTiles.length; i++) {
                                    var t = modelData.ownTiles[i]
                                    if (t.id && String(t.id).endsWith("capacity_bytes")) { cap = t.value; break }
                                }
                                return cap
                            }
                            opacity: 0.6
                            font.pixelSize: app.tokens.textCaption
                        }
                    }

                    GridLayout {
                        Layout.fillWidth: true
                        columns: Math.max(1, Math.floor(view.availableWidth / 240))
                        rowSpacing: app.tokens.spaceM
                        columnSpacing: app.tokens.spaceM
                        Repeater {
                            model: modelData.ownTiles
                            delegate: SensorTile {
                                required property var modelData
                                Layout.fillWidth: true
                                Layout.preferredHeight: 156
                                tileName: modelData.name
                                tileDeviceLabel: ""
                                tileValue: modelData.value
                                tileKind: modelData.kind || "scalar"
                                tileRows: modelData.rows || []
                            }
                        }
                    }

                    Repeater {
                        model: modelData.mounts
                        delegate: Rectangle {
                            required property var modelData
                            Layout.fillWidth: true
                            Layout.leftMargin: app.tokens.spaceL
                            Layout.preferredHeight: mountCard.implicitHeight + app.tokens.spaceM * 2
                            radius: app.tokens.radiusCard
                            color: app.tokens.surface0
                            border.color: app.tokens.separator
                            border.width: 1

                            ColumnLayout {
                                id: mountCard
                                anchors.fill: parent
                                anchors.margins: app.tokens.spaceM
                                spacing: app.tokens.spaceM

                                Controls.Label {
                                    text: modelData.label
                                    font.pixelSize: app.tokens.textCaption
                                    font.weight: app.tokens.weightBold
                                    opacity: 0.7
                                }
                                GridLayout {
                                    Layout.fillWidth: true
                                    columns: Math.max(1, Math.floor((view.availableWidth - app.tokens.spaceL) / 240))
                                    rowSpacing: app.tokens.spaceM
                                    columnSpacing: app.tokens.spaceM
                                    Repeater {
                                        model: modelData.tiles
                                        delegate: SensorTile {
                                            required property var modelData
                                            Layout.fillWidth: true
                                            Layout.preferredHeight: 156
                                            tileName: modelData.name
                                            tileDeviceLabel: ""
                                            tileValue: modelData.value
                                            tileKind: modelData.kind || "scalar"
                                            tileRows: modelData.rows || []
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Switch the page body between flat and nested**

In `apps/linsight-gui/qml/CategoryPage.qml`, replace the entire body block from `Controls.ScrollView {` (line 218) through its matching closing `}` (line 293) with a `Loader` that picks the renderer, wrapping the existing flat grid into a `Component`:

```qml
    Loader {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.leftMargin: app.tokens.spaceXL
        anchors.rightMargin: app.tokens.spaceXL
        anchors.topMargin: app.tokens.spaceL
        anchors.bottomMargin: app.tokens.spaceL
        sourceComponent: page.nested ? nestedView : flatView
    }

    Component {
        id: nestedView
        StorageSectionView { sections: page.storageSections }
    }

    Component {
        id: flatView
        Controls.ScrollView {
            clip: true
            contentWidth: availableWidth

            GridLayout {
                id: grid
                width: parent.width
                columns: Math.max(1, Math.floor(parent.width / 240))
                rowSpacing: app.tokens.spaceM
                columnSpacing: app.tokens.spaceM

                Repeater {
                    model: page.tilesArray
                    delegate: Loader {
                        id: cellLoader
                        sourceComponent: modelData.type === "header" ? headerComponent : tileComponent
                        Layout.columnSpan: modelData.type === "header" ? grid.columns : 1
                        Layout.fillWidth: true
                        Layout.preferredHeight: modelData.type === "header" ? 32 : (modelData.kind === "table" && modelData.rows && modelData.rows.length > 0 ? 280 : 156)
                        onLoaded: {
                            item.anchors.fill = cellLoader
                            if (modelData.type === "header") {
                                item.label = modelData.label
                            } else {
                                item.tileName = modelData.name
                                item.tileDeviceLabel = modelData.deviceLabel || ""
                                item.tileValue = modelData.value
                                item.tileKind = modelData.kind || "scalar"
                                item.tileRows = modelData.rows || []
                            }
                        }
                    }
                }

                Component {
                    id: headerComponent
                    Rectangle {
                        property string label: ""
                        Layout.fillWidth: true
                        color: app.tokens.surface0

                        Rectangle {
                            anchors.bottom: parent.bottom
                            width: parent.width
                            height: 1
                            color: app.tokens.separator
                        }

                        Controls.Label {
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            anchors.leftMargin: app.tokens.spaceM
                            text: label
                            font.pixelSize: app.tokens.textCaption
                            font.weight: app.tokens.weightBold
                            opacity: 0.6
                        }
                    }
                }

                Component {
                    id: tileComponent
                    SensorTile {
                        Layout.fillWidth: true
                    }
                }
            }
        }
    }
```

Leave the trailing empty-state `Controls.Label { ... "No %1 sensors detected" ... }` (lines 295-301) unchanged.

- [ ] **Step 3: Register the new QML file (if the module lists files explicitly)**

Run: `grep -rn "CategoryPage.qml\|SensorTile.qml\|QML_FILES\|qml_files\|qt_add_qml_module\|qml.qrc" apps/linsight-gui --include='*.rs' --include='*.toml' --include='CMakeLists.txt' --include='build.rs' | head`
If QML files are enumerated in a build manifest (e.g. `build.rs`, a `.qrc`, or `Cargo.toml` metadata), add `StorageSectionView.qml` next to `SensorTile.qml` in that list. If QML is loaded from a directory/glob, no change is needed.

- [ ] **Step 4: Build**

Run: `cargo build -p linsight 2>&1 | tail -8`
Expected: builds cleanly. Fix any QML compile errors reported by cxx-qt-build (e.g. an unknown design token — replace with one that exists in `DesignTokens.qml`).

- [ ] **Step 5: Commit**

```bash
git add apps/linsight-gui/qml/StorageSectionView.qml apps/linsight-gui/qml/CategoryPage.qml
git commit -m "gui(storage): render mounts as inset cards within their disk"
```

---

## Task 6: Full build, tests, and manual verification

**Files:** none (verification)

- [ ] **Step 1: Build the workspace**

Run: `just build 2>&1 | tail -8`
Expected: `Finished` with no errors.

- [ ] **Step 2: Run the full test suite**

Run: `just test 2>&1 | grep -E 'test result|FAILED|error\[' | tail -30`
Expected: every `test result: ok`, zero `FAILED`.

- [ ] **Step 3: Launch and verify on the real machine**

Run (no stale daemon): `pkill -x linsightd 2>/dev/null; ./target/debug/linsight > /tmp/linsight-verify.log 2>&1 &`
Then on the **Storage** page, confirm:
- Each physical disk is a card, ordered by capacity (largest first).
- Each disk's own sensors appear first, then inset mount cards (e.g. `btrfs (/)`, `btrfs (/home)`) for mounts that live on it.
- Network/zram mounts (if any) appear as their own top-level cards after the disks.
Check `/tmp/linsight-verify.log` for QML warnings (binding loops, undefined props) and fix any.

- [ ] **Step 4: Final commit (if any fixes were made in Step 3)**

```bash
git add -A
git commit -m "gui(storage): fix nested-render issues found in manual verification"
```

---

## Notes / non-goals

- LVM/dm/md/loop/zram sources resolve to `None` (orphan top-level) by design — slave resolution to underlying disks is out of scope.
- The existing inline byte-parsing in the GPU/storage branches of `tilesArray` is left as-is; `parseBytes()` is added only for the new code (a later DRY pass could converge them).
- `tilesArray` still computes for storage (used by the header's "%1 sensors" count and the empty-state check); the nested view reads `storageSections`.
