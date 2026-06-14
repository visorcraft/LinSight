<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Security model

LinSight is a system-monitoring tool: the daemon reads `/sys`,
`/proc`, NVML, and `/dev` to gather telemetry, and the daemon
loads third-party `.so` plugins. This document spells out what
trust boundaries exist today.

## Plugin trust

Loaded `.so` plugins run in-process inside `linsightd`. They have
the daemon's full filesystem and network access. There is no
sandbox today.

ABI v6 (R-mirror types — stabby-marked structs over `#[repr(u8)]`
discriminants) hardens the FFI seam against vtable-layout drift
between rustc releases. A mis-built plugin fails loudly at load
time: the `linsight_plugin_v6` symbol is missing on an older `.so`, and
stabby's `_stabbied_v3_report` type check rejects any shape
mismatch on a v6 `.so` built against an incompatible SDK. ADR-0001
records why we moved off stabby's tagged-enum encoding for the
payload-bearing mirrors (release-mode `match_owned` misdispatch on
`#[repr(stabby)]` enums; see ADR-0001 § "What we learned at v3").
The audit-driven hardening sprint also added two integrity guards
at the boundary:

- `host_init` runs every plugin-returned sensor ID through
  `SensorId::try_new` *before* the From-conversion calls the
  infallible `SensorId::new`. A release-mode plugin emitting
  whitespace-bearing or empty IDs is rejected with
  `PluginError::Parse` rather than poisoning the registry.
- `PluginCtx::new_with_sysroot(PathBuf)` rejects non-UTF-8 paths
  up front so the FFI mirror's UTF-8 contract holds. The
  `to_string_lossy` corruption hazard previously present at the
  conversion site is gone.

These are integrity guarantees, not a sandbox; a malicious plugin
still runs with the daemon's full capabilities.

Since ABI v6 the daemon also catches panics that unwind out of a
plugin's `init`/`sample`/`shutdown` (the trait methods are
`extern "C-unwind"` and the release profile is `panic = "unwind"`),
so a buggy plugin is isolated and dropped instead of taking the whole
daemon down — robustness hardening, still not a security sandbox.

- Plugins from `/usr/lib/linsight/plugins/` are distro-trust —
  they were vetted by the packager.
- Plugins from `$XDG_DATA_HOME/linsight/plugins/` are user-trust —
  the same level of trust as anything else in that user's home
  directory.
- A first-launch acknowledgement dialog (planned, not yet shipped)
  will list third-party plugins before they're loaded the first
  time.

A future sandbox pass will run each plugin in a seccomp-filtered
worker process with read-only `/sys` and `/proc` mounts. The trait
+ wire protocol stay the same; the trust boundary moves from
in-process to inter-process. See [`docs/plugin-sandbox.md`](docs/plugin-sandbox.md)
for the concrete design and rollout plan.

## Network surface

- The Unix socket at `$XDG_RUNTIME_DIR/linsight.sock` is the only
  always-listening socket. It's a per-user socket (`0700` runtime
  dir) so other local users can't reach it without root.
- The Prometheus exporter is **opt-in** via `LINSIGHT_PROM_BIND`.
  It binds wherever you tell it to. The default in
  `packaging/systemd/linsight.service` is `127.0.0.1:9777` —
  loopback only.
- Remote dashboards use SSH-forwarded local sockets. No network
  protocol is invented; `ssh -L` is the wire. This is the
  recommended path for most users.
- `linsight-tunnel` ships an mTLS bridge for non-SSH topologies
  (see [`apps/linsight-tunnel/README.md`](../apps/linsight-tunnel/README.md)).
  Both `server` and `client` modes require a cert + key + CA at
  the CLI; the daemon never sees the TLS layer —
  `linsight-tunnel` is a transparent byte pipe between
  TCP-with-mTLS and the daemon's Unix socket. The default bind is
  `127.0.0.1:9443` (loopback only); pass `--bind 0.0.0.0:9443`
  explicitly to expose to the network. Both modes cap concurrent
  connections via `--max-connections` (default 64) so a peer with
  a valid client cert can't burst-open enough sessions to exhaust
  the daemon's resources. The tunnel does **not** authenticate
  users beyond the cert chain you configure; treat it like a
  system-level secret-channel where presence of a valid client
  cert is the access decision. **Important:** the
  `WebPkiClientVerifier` enforces CA-chain validity. The server can
  also apply per-client-cert filters with `--allow-cn <pattern>` and
  `--allow-san <pattern>`; if neither is supplied, any client certificate
  chained to the configured CA is accepted. The configured CA bundle is
  therefore a full-daemon-access trust boundary unless you add CN/SAN
  filters; rotate or constrain it carefully.

## Sensor surface

- `/proc/stat`, `/proc/meminfo`, `/proc/net/dev` — readable by any
  user, no privilege.
- `/sys/class/drm/card*/device/...` — readable by any user.
- `/sys/class/nvme/...` and `/sys/class/block/.../stat` — readable
  by any user.
- NVML — needs the NVIDIA kernel module loaded; no special
  privilege after that.
- `intel_gpu_top` PMU events — would need CAP_PERFMON or root, but
  we don't use them today; the xe sensor reads sysfs only.

## Always-on mode + secrets

- `$XDG_DATA_HOME/linsight/history.db` is mode `0600` by virtue of
  the XDG_DATA_HOME default permissions. Don't put it on a shared
  filesystem.
- `~/.config/linsight/alerts.toml` may reference notify targets
  that exec a program. The `exec:<argv>` target argv-splits with
  POSIX-style quoting (single quotes, double quotes, backslash
  escapes) and `execve()`s the result directly — there is **no
  shell interposed**. `;`, `|`, `&&`, `$()`, etc. are passed as
  literal argv tokens. (An earlier `shell:<cmd>` target passed the
  raw config string to `sh -c` and was an RCE for anyone able to
  write the file; it was removed in the audit-driven hardening
  sprint and is now a no-op with a warning if referenced.)
- API keys for cloud notification providers (future work) will
  live in the Secret Service (KWallet / GNOME Keyring) under the
  `com.visorcraft.LinSight` service, never on disk.

## Reporting a vulnerability

**Do not file a public GitHub issue, discussion, or pull request for
security problems.** Report privately through **GitHub's private
vulnerability reporting**:

1. Go to the repository's **Security** tab.
2. Click **Report a vulnerability**.
3. Fill in the advisory form with the details below.

This keeps the report confidential between you and the maintainers
until a fix is ready. Please include as much as you can:

- a description of the issue and its impact,
- step-by-step reproduction steps,
- the LinSight version and your Linux distribution / kernel,
- the relevant configuration, logs, or a proof-of-concept,
- a suggested fix or mitigation, if you have one.

### What to expect

- **Acknowledgement** of your report within a few days.
- An initial assessment and, where confirmed, a remediation plan.
- Progress updates through the private advisory thread until the
  issue is resolved.
- Credit for your responsible disclosure in the advisory, unless you
  prefer to remain anonymous.

We ask that you give us a reasonable opportunity to ship a fix before
any public disclosure.
