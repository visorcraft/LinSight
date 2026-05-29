<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Advanced Sensors via pkexec — Design Spec

**Status:** draft 2026-05-26.
**Author:** Claude (paired with VisorCraft).
**Implementation plan:** TBD; this spec defines the design for a
follow-up sprint.

## Problem

Several useful sensor values live behind root-only system surfaces
that the unprivileged daemon can't read:

| Datum | Source | Why root-only |
|---|---|---|
| Memory speed (MT/s, e.g. DDR5-9600) | DMI Type 17 (Memory Device) | `/sys/firmware/dmi/{tables/DMI,entries/17-*/raw}` is `r--------` |
| Per-DIMM manufacturer + part number | DMI Type 17 | same |
| BIOS / motherboard version detail beyond `/sys/class/dmi/id` | DMI Type 0, 2 | most fields restricted to root |
| SMART attributes per drive (temp, wear, hours) | `smartctl -A /dev/nvme0` | needs `CAP_SYS_RAWIO` or sudo |
| PCIe link speed / generation per device | some sysfs files require root | varies by kernel |
| Chassis / motherboard fan speeds | ACPI, EC, or vendor-specific drivers | varies |

The user-facing motivation is straightforward: Mission Center, neofetch,
and inxi all show memory speed on the same hardware. LinSight should
match — without requiring the daemon to run as root permanently.

## Non-goals

- Running the LinSight daemon as root. The whole architecture
  assumes `linsightd` is user-scope. Privileged data flows through
  a narrow, audited helper invoked on demand.
- Reading `/dev/mem` or `/proc/kcore`. Memory speed and DIMM
  identity don't need this; SMART data doesn't either. If a future
  sensor genuinely requires it, that's a separate design.
- Continuous root access. The helper runs once per "refresh
  advanced sensors" action and exits. The daemon never holds a
  privileged file descriptor or a persistent capability.
- Cross-platform polkit / authorization frameworks beyond Linux.
  PolKit is the only authentication path; macOS / BSD / Windows
  are out of scope (the daemon is Linux-only anyway).

## High-level shape

A tiny privileged helper binary, `linsight-privileged-probe`,
performs the root-only reads and writes the results to a
user-readable JSON cache. The daemon reads the cache like any
other sysfs file; sensor plugins query it through a shared
`linsight-core::privileged_cache` module.

The helper is invoked via `pkexec` (PolicyKit) when the user opts
in through the GUI's Settings page. The GUI surfaces a single
"Enable advanced sensors" toggle plus a "Refresh now" button; both
trigger a `pkexec linsight-privileged-probe` call that prompts for
the user's password (or whichever PolKit method is configured).

```
┌─────────────────┐         pkexec           ┌────────────────────────┐
│ LinSight GUI    │ ─────────────────────▶  │ linsight-privileged-   │
│ (user)          │                          │ probe (root, exits     │
│                 │                          │ immediately after work)│
└─────────────────┘                          └────────┬───────────────┘
                                                      │ writes
                                                      ▼
                                       ~/.config/linsight/advanced.json
                                                      │ reads
                                                      ▼
                                  ┌─────────────────────────────────┐
                                  │ linsightd (user, never elevated)│
                                  │ mem plugin reads cache;          │
                                  │ surfaces mem.speed_mts etc.      │
                                  └─────────────────────────────────┘
```

## Security model

The privileged helper has a small, deterministic job:

1. Read every file under `/sys/firmware/dmi/entries/`.
2. Parse DMI types we care about (0, 2, 17, ...).
3. Read any other sources we add (e.g. SMART via direct ioctl).
4. Write a JSON cache to `$HOME/.config/linsight/advanced.json`
   owned by the calling user (not root).
5. Exit.

Specifically what the helper does NOT do:

- Read arbitrary paths passed on the command line. Sources are
  hardcoded in the helper.
- Take any input from the daemon (the daemon does not invoke the
  helper directly — the GUI does, via pkexec).
- Hold open file descriptors, listen on sockets, or fork. It's a
  pure read-parse-write-exit binary.
- Modify any system state (no writes to /sys, /proc, or device
  files).

Attack surface analysis:

- **Helper code itself.** The whole binary is < 500 lines of Rust;
  reviewed once at write time, then frozen. No external user
  input. Any future change goes through code review against the
  hardening checklist documented in this spec.
- **pkexec invocation.** A PolKit `.policy` file ships with the
  package. The policy declares the action ID
  `io.visorcraft.linsight.refresh-advanced-sensors`, requires
  `auth_admin` (admin password), and points at the helper's
  installed path. PolKit handles the password prompt; LinSight
  never sees or stores the password.
- **Cache file ownership.** Helper writes the JSON owned by the
  invoking user (`SUDO_UID`/`PKEXEC_UID` environment) with mode
  0600. The daemon reads it through the same user context;
  nothing else on the system has access.
- **Cache staleness.** The cache includes a `captured_at`
  timestamp. The mem plugin surfaces the values as-is — DIMM
  speed doesn't change at runtime on any hardware LinSight runs
  on. If the user replaces RAM, they re-run the refresh.

What we explicitly accept:

- **The user runs an untrusted privileged binary.** Same risk as
  installing any system package. Mitigated by code review +
  signing the binary in distro packages.
- **PolKit prompt fatigue.** Auth is needed once per refresh, not
  per metric read. The cache lasts indefinitely (no expiry); the
  user only re-auths if they want fresh data (e.g. after a BIOS
  update changes memory speed).

## Data model

`~/.config/linsight/advanced.json` schema, version 1:

```json
{
  "schema_version": 1,
  "captured_at": "2026-05-26T15:30:00Z",
  "memory": {
    "modules": [
      {
        "locator": "Controller0-ChannelA",
        "type": "LPDDR5",
        "size_bytes": 8589934592,
        "speed_mts": 9600,
        "configured_speed_mts": 9600,
        "manufacturer": "Micron Technology",
        "part_number": "MT62F4G32D8DV-020 WT",
        "serial": null
      }
    ],
    "total_capacity_bytes": 68719476736,
    "slot_count": 8,
    "occupied_slots": 8,
    "fastest_module_mts": 9600
  },
  "smart": {
    "drives": [
      {
        "device": "/dev/nvme0",
        "model": "Samsung SSD 990 PRO 4TB",
        "temperature_c": 42,
        "power_on_hours": 1234,
        "percentage_used": 2
      }
    ]
  }
}
```

Schema rules:

- Unknown keys parse cleanly via `#[serde(default)]` so an older
  daemon doesn't choke on a newer cache that added fields.
- `schema_version` is bumped only on shape-breaking changes; the
  daemon refuses caches whose `schema_version` it doesn't
  recognize and logs the mismatch (no crash).
- `null` fields are valid — some DMI implementations don't
  populate serial / part numbers; we surface what we have.

## Sensors unlocked

New sensors the mem and nvme plugins expose when `advanced.json`
exists:

| Sensor ID | Source | Notes |
|---|---|---|
| `mem.speed_mts` | `fastest_module_mts` | Headline number; shown in Overview's 4th tile when present |
| `mem.module_count` | `slot_count` / `occupied_slots` | A `Table` reading with one row per DIMM |
| `mem.modules` | full module list | A `Table` for the Hardware page |
| `nvme.<id>.temp_smart_c` | SMART temperature | Distinct from the existing hwmon-based `nvme.<id>.temp_c` — SMART has different averaging |
| `nvme.<id>.wear_pct` | SMART `percentage_used` | Drive lifetime indicator |
| `nvme.<id>.power_on_hours` | SMART | Drive longevity |

Without `advanced.json`, all these sensors return
`PluginError::Unsupported` and disappear from the catalogue.

## Plugin SDK changes

None required. `advanced.json` reads happen inside existing
plugins (`mem` and `nvme`) via a shared
`linsight-core::privileged_cache::AdvancedSensors` helper that
loads + caches the JSON. The plugin SDK ABI stays at v4.

## Helper binary

New crate: `apps/linsight-privileged-probe/`. Builds to
`/usr/lib/linsight/linsight-privileged-probe` (out of `$PATH` so
nobody runs it directly by accident).

```rust
fn main() -> Result<()> {
    require_root_uid()?;             // refuse to run as non-root
    let uid = pkexec_origin_uid()?;  // PKEXEC_UID env or SUDO_UID
    let memory = read_dmi_memory()?; // /sys/firmware/dmi/entries/17-*/raw
    let smart = read_smart()?;       // ioctl on each /dev/nvme*
    let payload = AdvancedSensors {
        schema_version: 1,
        captured_at: SystemTime::now(),
        memory,
        smart,
    };
    write_user_cache(uid, &payload)?;
    Ok(())
}
```

Key invariants enforced at startup:

- `geteuid() == 0` or the helper exits with code 78 (`EX_NOPERM`).
- `PKEXEC_UID` or `SUDO_UID` is present and parses as `u32`; the
  helper drops privileges before writing the cache so the file is
  owned by the invoking user.
- No command-line args are accepted; `--help` is the only
  recognized flag and emits a one-line description.

## PolKit policy

Ships at `/usr/share/polkit-1/actions/io.visorcraft.linsight.policy`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE policyconfig PUBLIC ...>
<policyconfig>
  <action id="io.visorcraft.linsight.refresh-advanced-sensors">
    <description>Refresh LinSight's advanced hardware sensors</description>
    <message>LinSight needs administrator access to read memory speed and SMART data.</message>
    <icon_name>io.visorcraft.LinSight</icon_name>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
    <annotate key="org.freedesktop.policykit.exec.path">/usr/lib/linsight/linsight-privileged-probe</annotate>
  </action>
</policyconfig>
```

- `auth_admin_keep`: while the desktop session is active, a
  successful auth is cached for ~5 minutes (PolKit's default) so
  consecutive refreshes don't re-prompt.
- `auth_admin`: an inactive or remote session requires a fresh
  password. Reasonable default; users running over SSH /
  Wayland-without-seat still get one prompt per refresh.

## GUI flow

### Settings page

A new section "Advanced sensors" with:

- **Toggle**: "Read privileged hardware data" (off by default).
  Turning it ON triggers a pkexec invocation immediately.
- **Status line**: "Last refreshed: 2026-05-26 15:30" or "Never
  refreshed — toggle to enable".
- **Refresh button**: re-runs the pkexec call on demand.

Toggle behaviour:

- OFF → ON: GUI shows a modal explaining what data will be read
  and why root is needed, then runs `pkexec
  linsight-privileged-probe`. On success, the toggle stays ON
  and the status line updates. On user cancel or auth failure,
  the toggle reverts to OFF and an error banner appears.
- ON → OFF: GUI deletes `~/.config/linsight/advanced.json` and
  the daemon's mem/nvme plugins re-emit their `Unsupported`
  fallbacks within ~1 second (the next sample). No pkexec needed.

### Modal copy

When the user first toggles ON:

> **LinSight needs administrator access** to read advanced
> hardware sensors — memory speed, DIMM details, and SMART data
> for your NVMe drives. These come from the system's DMI tables
> and SMART logs, which are protected from non-administrator
> programs.
>
> You'll see a password prompt next. LinSight never sees your
> password; the system handles it through PolicyKit.
>
> The advanced data is cached locally in
> `~/.config/linsight/advanced.json` (owned by you, mode 0600).
> You can refresh or disable it any time from this page.
>
> [Cancel] [Continue]

### Overview tile change

When `advanced.json` exists with `memory.fastest_module_mts`,
the Overview page's 4th tile becomes a user-settable choice:

- "CPU frequency" (default, current)
- "Memory speed" (new, only when advanced data is available)

A small dropdown next to the tile title lets the user switch. The
choice persists in `preferences.json`.

### Hardware page additions

The memory section of the Hardware page (currently absent — RAM
is a non-goal of the existing spec) becomes available when
advanced data exists:

```
Memory
  Total: 64 GiB across 8 slots (8 occupied)
  Speed: 9600 MT/s (LPDDR5)
  Modules:
    Controller0-ChannelA: 8 GiB LPDDR5-9600  Micron MT62F4G32D8DV-020 WT
    Controller0-ChannelB: 8 GiB LPDDR5-9600  Micron MT62F4G32D8DV-020 WT
    ...
```

Without advanced data, the Hardware page shows a small
"Enable advanced sensors in Settings to see memory details" hint
where the memory card would go.

## Daemon plumbing

Adds `apps/linsightd/src/advanced_cache.rs`:

```rust
pub struct AdvancedCache {
    path: PathBuf,
    last_loaded: Mutex<Option<(SystemTime, AdvancedSensors)>>,
}

impl AdvancedCache {
    /// Returns the parsed cache, re-reading from disk if the file's
    /// mtime has changed since the last load. `None` means the file
    /// doesn't exist (advanced sensors are disabled).
    pub fn current(&self) -> Option<AdvancedSensors>;
}
```

`AdvancedCache` is shared between the mem and nvme plugins via the
existing plugin context. The cache re-reads on mtime change so a
fresh `pkexec` writes a new cache and plugins pick it up at the
next sample.

## Test plan

- **Unit:** `dmi::parse_memory_devices` against a small fixture
  binary blob (committed under `apps/linsight-privileged-probe/
  tests/fixtures/dmi-blade16.bin`). Asserts the LPDDR5-9600
  modules round-trip.
- **Unit:** `AdvancedCache::current` re-reads on mtime bump;
  returns `None` when the file is absent; falls back to defaults
  + renames to `.bad` on parse failure.
- **Unit:** `mem.speed_mts` sensor returns Unsupported when the
  cache is absent, the value when it exists, and re-reads when
  the cache mtime changes.
- **Integration:** spawn the daemon with a synthetic
  `XDG_CONFIG_HOME/linsight/advanced.json`, send a Subscribe,
  assert `mem.speed_mts` samples carry the right value.
- **Manual:** install the polkit policy, run the GUI on the user's
  Razer Blade 16, toggle "Enable advanced sensors" — confirm the
  password prompt, the cache file appears with mode 0600, and the
  Overview tile dropdown shows "Memory speed" with `9600 MT/s`.

## Packaging

Files added to every distro packaging recipe (Arch, Debian,
Fedora, Flatpak):

| Path | Owner | Mode | Notes |
|---|---|---|---|
| `/usr/lib/linsight/linsight-privileged-probe` | root | 0755 | Helper binary; NOT setuid (pkexec invokes it) |
| `/usr/share/polkit-1/actions/io.visorcraft.linsight.policy` | root | 0644 | PolKit action |
| `/usr/share/dbus-1/system.d/io.visorcraft.linsight.conf` | — | — | None — the helper doesn't speak DBus |

Flatpak constraint: the sandboxed flatpak version CANNOT install
a polkit policy or run pkexec. Flatpak users see the toggle
greyed out with a tooltip explaining that advanced sensors
require a system-package install. (Or: Flatpak users could
manually run `flatpak-spawn --host pkexec ...` from a wrapper
script, but that adds complexity for a small audience.)

## Migration

- First daemon start with no advanced cache: every advanced sensor
  returns Unsupported, the Hardware page shows the
  "enable in Settings" hint, the Overview tile dropdown only
  shows "CPU frequency". No changes from current behavior.
- After user enables: cache file appears, the daemon picks it up
  on the next sample (within ~1 s), new sensors become available.
- After user disables: cache file is deleted, sensors revert
  within ~1 s.
- Reboot: cache file persists. Advanced sensors stay available.
  (DIMM speed doesn't change across boots unless the user replaces
  RAM, in which case they re-toggle.)

## Risks / open questions

- **Cache staleness across BIOS / RAM changes.** No automated
  detection; users have to know to re-refresh. Acceptable for v1.
- **PolKit on minimal / headless systems.** Some server distros
  don't ship polkit; the helper would need a fallback (e.g.
  `sudo`) for non-desktop deployments. Documented but not
  implemented in v1.
- **NVMe SMART read permissions.** The exact ioctl on Linux 7+
  needs verification; some kernels expose it via `/dev/nvme0`
  open + ioctl, others gate it harder. Confirm during
  implementation; punt to v2 if it's painful.
- **Wayland session authentication.** PolKit needs a session
  agent (KDE / GNOME / lxqt-policykit). On bare Sway / X11 with
  no session agent, the prompt won't appear. Document the
  requirement.

## Out of scope / future work

- Live monitoring of memory speed (it doesn't change, so polling
  is wasteful — read once and cache).
- Per-DIMM ECC error counters (those ARE live but require a
  separate kernel sysfs path — separate spec).
- DBus-based privileged service (long-running daemon with PolKit
  actions per metric). More flexible but much heavier; we'd only
  consider this if a non-DMI metric needs continuous root access.
- Cross-vendor SMART abstraction (SATA + NVMe + USB-storage).
  v1 ships NVMe only.
- GPU BIOS-level details (vendor-specific, ranges from
  nvidia-smi to vendor tools). Out of scope.
