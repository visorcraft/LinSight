<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Plugin sandbox design

## Status

Groundwork / design. No implementation yet.

## Problem

Today every dynamic `.so` plugin is `dlopen`ed directly into `linsightd`
(`docs/SECURITY.md` § "Plugin trust"). A malicious or compromised plugin
runs with the daemon's full UID: it can read any file the user can read,
write to `$XDG_DATA_HOME`, open network connections, and execute arbitrary
syscalls. ABI v6 hardens the FFI seam against vtable drift and catches
panics, but it is **not a sandbox**.

## Goal

Run third-party dynamic plugins in a separate, short-lived worker process
that can:

- read `/sys`, `/proc`, and any path the plugin legitimately needs via an
  allow-list;
- return `Sample`s and `SensorDescriptor`s to the daemon over a local IPC
  channel;
- do little else.

In-tree plugins remain statically linked and run in-process unless an
operator opts them into the sandbox too. The trust boundary moves from
"same address space as the daemon" to "separate process with a seccomp
filter".

## Threat model

- **Daemon** is trusted. It owns the Unix socket, the scheduler, and the
  history database.
- **Distro-shipped plugins** in `/usr/lib/linsight/plugins/` are trusted
  by the packager; sandboxing them is defense-in-depth.
- **User-installed plugins** in `$XDG_DATA_HOME/linsight/plugins/` are
  untrusted and must be sandboxed by default once the feature ships.
- A buggy plugin must not be able to: escape the worker, exfiltrate data
  over the network, write to the daemon's data directory, or affect other
  plugins.

## Architecture

```
+----------------------------+           +----------------------------+
| linsightd                  |           | linsight-plugin-worker     |
|                            |  socket   | (one per dynamic plugin)   |
|  PluginHost                | <---------> |  dlopen libfoo.so          |
|  ├─ in-tree sensors        |  pair     |  seccomp-filtered          |
|  └─ remote: WorkerHandle   |           |  readonly /sys,/proc mounts|
+----------------------------+           +----------------------------+
```

1. **Daemon side**: `PluginHost` keeps in-tree plugins as `Arc<dyn
   LinsightPlugin>`. For dynamic plugins it spawns a `WorkerHandle` that
   owns the worker's stdin/stdout (or socket pair) and translates
   `init()`/`sample()`/`shutdown()` calls into framed IPC messages.

2. **Worker side**: a tiny binary `linsight-plugin-worker` receives a
   plugin path + config JSON on startup, calls `dlopen`, validates ABI
   version, and then enters its event loop. It responds to `Init`,
   `Sample`, and `Shutdown` requests with the same `RPluginManifest` /
   `RReading` types used today.

3. **IPC**: length-prefixed postcard over a Unix socket pair (or pipe).
   The message set is intentionally smaller than the GUI protocol:
   `WorkerReq { Init, Sample(SensorId), Shutdown }` and
   `WorkerResp { Manifest, Reading, Error, Panicked }`.

4. **Sandbox**: after `init()` succeeds, the worker installs a seccomp
   filter (via `seccomp`/`libseccomp` or raw BPF) that allows only:
   - file opens with `O_RDONLY` under allowed paths;
   - `read`, `pread64`, `close`, `lseek`, `fstat`, `newfstatat`;
   - `gettimeofday`, `clock_gettime`;
   - `exit`, `exit_group`;
   - IPC read/write on the socket pair;
   - `mmap`/`munmap`/`mprotect` (many Rust allocators need these).
   Network syscalls (`socket`, `connect`, `sendto`, `recvfrom`) and
   writeable file opens are denied.

5. **Filesystem**: use ` Landlock` or a private mount namespace to make
   `/sys` and `/proc` read-only and hide `$XDG_DATA_HOME/linsight`. If
   Landlock is unavailable, fall back to path validation in a small
   privileged launcher (setuid is undesirable) or rely on seccomp +
   `O_RDONLY` checks alone.

## ABI compatibility

The worker and daemon must be built from the same ABI version. The worker
reports its ABI version in the first handshake byte; a mismatch causes
`PluginHost` to reject the plugin before calling `init()`. The wire types
reuse `linsight-plugin-sdk::mirror` so no new conversion code is needed.

## Failure modes

- Worker panic: caught by the worker, reported as `WorkerResp::Panicked`,
  worker exits. The daemon removes the plugin's sensors and logs the
  event — same behavior as today's in-process panic isolation.
- Worker OOM/seccomp kill: daemon notices EOF on the socket, marks plugin
  degraded, and stops sampling.
- Slow sample: daemon applies the same timeout it uses for in-process
  plugins; a hanging worker is killed and the sensor is degraded.

## Performance budget

Target overhead per sample: ≤ 1 ms additional latency and ≤ 2 MB RSS per
worker. Most plugins read a few small sysfs files per sample, so IPC
round-trip dominates. If a plugin emits many sensors the daemon can batch
`Sample` requests.

## Rollout plan

1. **Phase 1** (this design): agree on process model, IPC schema, and
   seccomp policy. Add `linsight-plugin-worker` crate skeleton.
2. **Phase 2**: implement worker binary and daemon `WorkerHandle` behind
   `LINSIGHT_PLUGIN_SANDBOX=1`. In-tree plugins stay in-process.
3. **Phase 3**: make sandbox the default for user-installed dynamic
   plugins; distro plugins opt-in via packaging.
4. **Phase 4**: optional sandbox for in-tree plugins reading sensitive
   hardware (e.g. NVML, GPU PMU).

## Open questions

- Should the worker be a separate cargo package (`linsight-plugin-worker`)
  or a hidden subcommand of `linsightd`? A separate package is easier to
  seccomp and keeps the daemon binary smaller.
- How do we handle plugins that legitimately need `ioctl` (e.g. future
  NVMe direct passthrough)? Add a capabilities manifest field that the
  daemon prompts the user to approve on first load.
- Do we restrict CPU/time with cgroups? Out of scope for v1; seccomp +
  read-only filesystem is the first milestone.

## Related documents

- `docs/SECURITY.md` — current plugin trust model.
- `docs/adr/0001-plugin-abi-stabby-deferral.md` — why R-mirror types cross
  the boundary.
- `crates/linsight-plugin-sdk/` — trait and mirror types the worker will
  reuse.
