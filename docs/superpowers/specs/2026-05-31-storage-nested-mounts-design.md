# Storage page — nested mount-points under physical disks

**Status:** Approved design (2026-05-31)
**Area:** `linsight-sensors-fs` (daemon), `apps/linsight-gui` (QML Storage page)

## Goal

On the Storage page, nest each mount-point section **inside** the physical
disk it resides on. Example: if `/home` lives on the Samsung 990 Pro, the
`btrfs (/home)` section becomes a child (inset) sub-section within the
**Samsung 990 Pro** section — rather than a sibling, flat section as today.

## Why this needs a daemon change (not QML-only)

The link does not exist in the data. The `fs` plugin reads `/proc/mounts`
but keeps only `(mountpoint, fstype)` — it discards column 0 (the source
device, e.g. `/dev/nvme0n1p2`). So:

- fs tiles are keyed by **mountpoint** (`deviceLabel: "btrfs (/home)"`, `device: "home"`).
- disk/nvme tiles are keyed by **physical device** (`device: "sda"` / `"nvme0"`).
- Nothing connects a mount to its backing disk.

The daemon must resolve each mount's backing block device up to a physical
device id that **matches what the disk/nvme plugins emit**, and expose it on
the fs tiles. The QML then joins on it.

## Decisions (from brainstorming)

1. **Linkage resolved in the daemon** (robust), not guessed in QML.
2. **Orphan mounts** (no resolvable shown disk — NFS/CIFS, zram/LVM/loop/dm)
   stay as their **own top-level sections**, after all disk sections.
3. **Rendering: inset-card** — mounts render as cards visually inset within
   the disk's card (the fuller option, not just an indented header band).
4. **Within a disk:** the disk's own sensors come first, then the mount
   sub-sections.

## Data layer (Rust — `linsight-sensors-fs`)

Capture `/proc/mounts` column 0 (already read today only to skip
`tmpfs`/`none`) and, per mount, compute a `parent_device: Option<String>`
whose value equals the disk/nvme plugin's `device_id` for that disk:

- **SATA/SCSI:** partition → parent disk via sysfs, e.g. `sda3` → `sda`.
  Resolution uses sysfs topology (`/sys/class/block/<dev>` has a `partition`
  file ⇒ it is a partition; parent disk = basename of the parent sysfs dir),
  **not** string-stripping of trailing digits.
- **NVMe:** partition → namespace → **controller**, e.g.
  `nvme0n1p2` → `nvme0n1` → **`nvme0`**. The extra namespace→controller step
  is required because the nvme plugin keys its disk by the controller name
  (`dev.name == "nvme0"`), not the namespace.
- **Whole-disk source** (`/dev/sda` mounted directly): maps to itself.
- **Unresolvable** (source is not a real block device, or resolves to a
  device the UI does not show — zram/dm/loop/md, network sources):
  `parent_device = None`.

Expose the resolved value as a new tile-JSON field `parentDevice`
(`#[serde(rename = "parentDevice", skip_serializing_if = "Option::is_none")]`)
carried only on fs tiles. Disk/nvme tiles are unchanged — they already carry
`device` (= `device_id`), which is the join target.

**Plumbing:** `parent_device` rides from the fs plugin's per-mount metadata
through `SensorInfo` to `TileJson` in `overview_model.rs`, mirroring how
`device_label` already flows.

## Grouping (QML — `CategoryPage.tilesArray`, storage branch)

Replace the flat storage grouping with a nested model. Storage produces a
structured list of **sections**, each either a disk or an orphan:

```
section = {
  kind: "disk" | "orphan",
  device,            // join id ("sda"/"nvme0") for disks; mount key for orphans
  label,             // header text: deviceLabel ("Samsung 990 Pro")
  capacityBytes,     // for ordering; from the disk's *capacity_bytes tile
  ownTiles: [...],   // disk's own sensors, alphabetical (empty for orphan)
  mounts: [          // child sub-sections (empty for orphan)
    { label, mountpoint, tiles: [...] }   // tiles alphabetical
  ]
}
```

Build rules:

1. Disk sections come from disk/nvme tiles, grouped by `device` (id), label
   = `deviceLabel`.
2. Each fs tile whose `parentDevice` matches an existing disk section nests
   into that disk's `mounts` (grouped by the fs tile's `deviceLabel` /
   mountpoint). An fs tile whose `parentDevice` is absent **or** matches no
   shown disk section becomes an **orphan** top-level section.
3. **Ordering:**
   - Top level: disks by capacity desc (existing logic; disks with no
     capacity sensor last), then orphan sections alpha by label.
   - `ownTiles`: alphabetical by sensor name.
   - `mounts`: alphabetical by mountpoint.
   - tiles within a mount: alphabetical by sensor name.

Non-storage pages (GPU, etc.) keep the existing flat `tilesArray` path
unchanged.

## Rendering (QML)

The current single flat `GridLayout` cannot express inset nesting. Introduce
a dedicated nested renderer (e.g. `StorageSectionView.qml`) that `CategoryPage`
delegates to for the nested (storage) case, leaving the existing flat
GridLayout path intact for GPU/other pages:

- Outer `ColumnLayout` of **disk cards**.
- Each disk card: header (label + capacity) → a `GridLayout` of `ownTiles`
  → a `ColumnLayout` of **inset mount cards**.
- Each mount card: sub-header (mountpoint/fstype label) → a `GridLayout` of
  its tiles, visually inset (inner margin / surface tint / border) within the
  disk card.
- Orphan sections render as top-level cards with no disk chrome.

Reuse existing `SensorTile`, design tokens, and the column-count math
(`Math.floor(width / 240)`) within each card's grid.

## Edge cases

- **btrfs subvolumes** (`/`, `/home`, `/.snapshots` all on one partition):
  all share one `parentDevice`; each mountpoint is a separate mount card under
  the one disk. ✓
- **Disk with no mounts:** disk card with only `ownTiles`. ✓
- **Mount resolves to a non-shown disk** (e.g. zram-backed): no matching disk
  section ⇒ orphan. ✓
- **Disk with no `capacity_bytes` sensor:** sorts last among disks (existing
  rule). ✓

## Testing

- **Rust unit test** for the resolver: a table of `/proc/mounts` lines against
  a fake sysroot (`/sys/class/block/...` fixtures) → expected `parent_device`,
  covering: SATA partition (`sda3`→`sda`), NVMe partition
  (`nvme0n1p2`→`nvme0`), whole-disk source, and unresolvable sources
  (network, zram, `none`).
- Existing `fs` / `disk` / `nvme` / `overview_model` tests stay green.
- Manual: launch GUI; Storage page shows mounts nested under their disks with
  inset cards; orphan mounts at the bottom.

## Non-goals (YAGNI)

- LVM/dm/md slave resolution to underlying physical disks (treated as orphan
  for now).
- Generalizing nested rendering to the GPU or other pages.
- Per-mount capacity-based ordering (mounts ordered alphabetically).

## Affected files (anticipated)

- `crates/linsight-sensors/fs/src/plugin.rs` — capture source device, resolver, `parent_device`.
- (resolver helper, possibly a new module in the fs crate.)
- `apps/linsight-gui/src/qobjects/overview_model.rs` — carry `parentDevice` into `TileJson`.
- `apps/linsight-gui/qml/CategoryPage.qml` — nested storage model + delegate to nested view.
- `apps/linsight-gui/qml/StorageSectionView.qml` — new nested renderer.
