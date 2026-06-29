// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

.pragma library

function formatBytes(b) {
    const n = Number(b)
    if (isNaN(n) || n <= 0) return "0 B"
    const units = ["B", "KiB", "MiB", "GiB", "TiB"]
    let i = 0
    let val = n
    while (val >= 1024 && i < units.length - 1) {
        val /= 1024
        i++
    }
    return val.toFixed(2) + " " + units[i]
}

function formatByteRate(bytesPerSec) {
    const n = Number(bytesPerSec)
    if (isNaN(n) || n <= 0) return "0 B/s"
    const KIB = 1024
    const MIB = KIB * 1024
    const GIB = MIB * 1024
    const TIB = GIB * 1024
    if (n >= TIB) return (n / TIB).toFixed(2) + " TiB/s"
    if (n >= GIB) return (n / GIB).toFixed(2) + " GiB/s"
    if (n >= MIB) return (n / MIB).toFixed(2) + " MiB/s"
    if (n >= KIB) return (n / KIB).toFixed(2) + " KiB/s"
    return n.toFixed(1) + " B/s"
}

function sparklineVaries(pts) {
    if (!Array.isArray(pts) || pts.length < 2) return false
    let mn = pts[0]
    let mx = pts[0]
    for (let k = 1; k < pts.length; ++k) {
        if (pts[k] < mn) mn = pts[k]
        if (pts[k] > mx) mx = pts[k]
    }
    return mx > mn
}

/// Merge a delta array of tile objects into an existing id→tile map.
/// Mutates `tileById` and returns it. Unknown ids are added; existing ids
/// are overwritten so live values refresh without rebuilding the whole map.
function mergeTileUpdates(tileById, delta) {
    if (!Array.isArray(delta)) return tileById
    for (let i = 0; i < delta.length; ++i) {
        const t = delta[i]
        if (!t || !t.id) continue
        tileById[t.id] = t
    }
    return tileById
}

/// Merge a delta array of tile objects into an id→value map.
function mergeTileValues(valueById, delta) {
    if (!Array.isArray(delta)) return valueById
    for (let i = 0; i < delta.length; ++i) {
        const t = delta[i]
        if (!t || !t.id) continue
        valueById[t.id] = t.value
    }
    return valueById
}
