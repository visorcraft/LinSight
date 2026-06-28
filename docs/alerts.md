<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Alerts

LinSight can evaluate sensor values against user-defined rules and dispatch
notifications when a rule fires or clears. The alert engine is **off by
default**; enable it with the environment variable `LINSIGHT_ALERTS=1`.

## Enabling alerts

For manual or development launches:

```bash
LINSIGHT_ALERTS=1 linsightd
```

When running under the systemd user unit, add the variable via
`systemctl --user edit linsight`:

```ini
[Service]
Environment=LINSIGHT_ALERTS=1
```

Rules are read from `$XDG_CONFIG_HOME/linsight/alerts.toml` (usually
`~/.config/linsight/alerts.toml`). The file is created automatically when
you add a rule through the GUI or CLI.

## Rule format

Each rule has a name, an expression, optional debounce and cooldown durations,
and a list of notify targets:

```toml
[[rule]]
name = "GPU hot"
expr = "xe.gpu0.temp_c > 85"
for = "30s"
cooldown = "5m"
notify = ["desktop"]
enabled = true
```

- `name` — human-readable identifier, shown in the GUI and notifications.
- `expr` — an [`evalexpr`](https://github.com/ISibboI/evalexpr) boolean
  expression. Sensor IDs are written with dots (`cpu.util`,
  `disk.nvme0n1.temp_c`) and are bound to the latest scalar value.
- `for` *(optional)* — debounce window. The expression must stay true for
  this long before the rule fires. Suffixes: `s`, `ms`, `m`, `h`.
  Default: immediate.
- `cooldown` *(optional)* — flap-suppression window. After a rule fires,
  it cannot fire again until this duration elapses, even if the expression
  goes false and true again. Suffixes: `s`, `ms`, `m`, `h`.
  Default: no cooldown.
- `notify` — list of targets (see below).
- `enabled` — `true` or `false`. Disabled rules are skipped entirely.

### Notify targets

- `"desktop"` — libnotify popup via `notify-rust`.  
  **Caveat:** the daemon must have access to the session D-Bus. This works
  when the daemon is launched from a desktop session, but may fail when
  running under a systemd user unit without `DISPLAY`, `WAYLAND_DISPLAY`,
  and `XDG_RUNTIME_DIR` propagated. If desktop notifications do not appear,
  check that those variables are present in the service environment:
  `systemctl --user show-environment | grep DISPLAY`.

- `"exec:<argv>"` — execute a program directly. Tokens are split using
  POSIX-style quoting (single quotes, double quotes, backslash escapes) and
  passed to `execve()` **with no shell interposed**. Metacharacters like
  `;`, `|`, `&&`, and `$()` are passed as literal argv elements. Use a
  wrapper script if you need shell features.  
  Example: `notify = ["exec:/usr/bin/notify-send 'GPU alarm' 'temp > 85'"]`

- `"webhook:<url>"` — HTTP POST to an external URL. The URL must use
  `http://` or `https://`. Loopback, link-local, private, and unspecified
  addresses are rejected, as are obfuscated numeric IPs (e.g. `2130706433`
  for `127.0.0.1`). Redirects are not followed. The POST body is a JSON
  object with `name`, `expr`, and `source` fields.

## Disk health alerts

SMART sensors are available for ATA and NVMe drives when udisks2 is on the
system bus (NVMe SMART requires udisks2 ≥ 2.10). The sensor IDs follow the
`disk.<name>.<metric>` pattern:

- `disk.nvme0n1.smart_temp_c` — temperature in °C
- `disk.nvme0n1.smart_health` — `ok` or `failing` (state sensor)
- `disk.nvme0n1.smart_power_on_hours` — power-on hours
- `disk.nvme0n1.smart_wear_pct` — wear percentage (NVMe only)
- `disk.nvme0n1.smart_realloc_sectors` — reallocated sector count (ATA only)

Example rules:

```toml
[[rule]]
name = "NVMe temperature"
expr = "disk.nvme0n1.smart_temp_c > 70"
for = "1m"
cooldown = "15m"
notify = ["desktop"]
enabled = true

[[rule]]
name = "NVMe health"
expr = "disk.nvme0n1.smart_health == \"failing\""
for = "30s"
notify = ["desktop"]
enabled = true
```

## Event log

Every fire and clear is recorded in an in-memory ring buffer (capacity 512
events). The GUI's Alerts page shows the most recent events with a relative
timestamp (e.g. "2 minutes ago").

Events are lost when the daemon restarts; they are not persisted to disk.

## Managing rules

### GUI

Open the **Alerts** page in the sidebar. Rules are listed with their current
state (firing / normal). Click a rule to edit its expression, debounce,
cooldown, notify targets, and enabled state. A "Desktop notification"
checkbox toggles the `"desktop"` target without editing the TOML manually.

### CLI

```bash
# List all rules
linsight-cli alert list

# Add or update a rule (creates the file if absent)
linsight-cli alert add "GPU hot" "xe.gpu0.temp_c > 85" \
  --for-duration 30s \
  --cooldown 5m \
  --notify desktop

# Delete a rule
linsight-cli alert remove "GPU hot"
```

## Security

The alerts config file lives in the user's home directory and is trusted
(same-user write access is assumed). The `exec:` target does not invoke a
shell, so a malicious `alerts.toml` cannot inject arbitrary commands through
unescaped metacharacters. See [`SECURITY.md`](SECURITY.md) for the full
threat model.
