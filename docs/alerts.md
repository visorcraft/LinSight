<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Alerts

LinSight can evaluate sensor values against user-defined rules and dispatch
notifications when a rule fires or clears. The alert engine is **off by
default**; enable it with the environment variable `LINSIGHT_ALERTS=1`.

## Enabling alerts

The systemd user unit (`linsight.service`) already gates the alert engine
via `Environment=LINSIGHT_ALERTS=1`. For manual or development launches:

```bash
LINSIGHT_ALERTS=1 linsightd
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
  this long before the rule fires. Suffixes: `s`, `ms`, `m`, `h`, `d`.
  Default: immediate.
- `cooldown` *(optional)* — flap-suppression window. After a rule fires,
  it cannot fire again until this duration elapses, even if the expression
  goes false and true again. Suffixes: `s`, `ms`, `m`, `h`, `d`.
  Default: no cooldown.
- `notify` — list of targets (see below).
- `enabled` — `true` or `false`. Disabled rules are skipped entirely.

### Notify targets

- `"desktop"` — libnotify popup via `notify-rust`.  
  **Caveat:** the daemon must have access to the session D-Bus. This works
  when the daemon is launched from a desktop session, but may fail when
  running under a systemd user unit without the `DISPLAY` / `WAYLAND_DISPLAY`
  and `XDG_RUNTIME_DIR` environment variables propagated. The systemd unit
  in `packaging/systemd/linsight.service` includes `Environment=DISPLAY=...`
  when those variables are present at install time; verify with
  `systemctl --user show-environment | grep DISPLAY`.

- `"exec:<argv>"` — execute a program directly. Tokens are split using
  POSIX-style quoting (single quotes, double quotes, backslash escapes) and
  passed to `execve()` **with no shell interposed**. Metacharacters like
  `;`, `|`, `&&`, and `$()` are passed as literal argv elements. Use a
  wrapper script if you need shell features.  
  Example: `notify = ["exec:/usr/bin/notify-send 'GPU alarm' 'temp > 85'"]`

## Event log

Every fire and clear is recorded in an in-memory ring buffer (capacity 512
events). The GUI's Alerts page shows the most recent events with a relative
timestamp (e.g. "2 minutes ago"). Events are also available via the CLI:

```bash
linsight-cli alert events
```

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

# Read a specific rule
linsight-cli alert read "GPU hot"

# Add or update a rule (creates the file if absent)
linsight-cli alert add --name "GPU hot" \
  --expr "xe.gpu0.temp_c > 85" \
  --for 30s \
  --cooldown 5m \
  --notify desktop \
  --enabled

# Delete a rule
linsight-cli alert delete "GPU hot"
```

## Security

The alerts config file lives in the user's home directory and is trusted
(same-user write access is assumed). The `exec:` target does not invoke a
shell, so a malicious `alerts.toml` cannot inject arbitrary commands through
unescaped metacharacters. See [`SECURITY.md`](SECURITY.md) for the full
threat model.
