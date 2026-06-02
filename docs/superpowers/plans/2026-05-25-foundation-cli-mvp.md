<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# LinSight Foundation + CLI MVP — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the LinSight workspace, core types, wire protocol,
plugin SDK, a single CPU sensor backend, the daemon (transport +
scheduler + plugin host), and a CLI that can subscribe to live values
— end-to-end working software in the terminal.

**Architecture:** A Cargo workspace with crates for types, protocol,
plugin SDK, the CPU sensor (built-in plugin), the `linsightd`
daemon binary, and the `linsight-cli` CLI binary. The daemon hosts
plugins, schedules subscription-driven sampling, and serves clients
over a `postcard`-framed Unix socket. The CLI is the first
real client.

**Tech Stack:** Rust 2024 edition (latest stable toolchain),
`stabby` for plugin ABI, `postcard` for the wire format,
`polling` for the daemon event loop, `clap` for the CLI,
`tracing` for logging, `tempfile` + `assert_cmd` + `predicates` +
`proptest` for tests, `just` as the task runner.

**Reference spec:** [`../specs/2026-05-25-linsight-design.md`](../specs/2026-05-25-linsight-design.md)
**Reference roadmap:** [`./2026-05-25-phases-roadmap.md`](./2026-05-25-phases-roadmap.md)

---

## File structure for this plan

```
linsight/
├── Cargo.toml                                  ← workspace manifest (Task 1)
├── Cargo.lock                                  ← generated
├── rust-toolchain.toml                         ← Task 1
├── rustfmt.toml                                ← Task 1
├── deny.toml                                   ← Task 1
├── Justfile                                    ← Task 2
├── LICENSE                                     ← Task 1
├── README.md                                   ← Task 1
├── AGENTS.md                                   ← Task 2
├── CONTRIBUTING.md                             ← Task 2
├── .gitignore                                  ← Task 1
│
├── crates/
│   ├── linsight-core/
│   │   ├── Cargo.toml                          ← Task 3
│   │   └── src/
│   │       ├── lib.rs                          ← Task 3
│   │       ├── types.rs                        ← Tasks 4-7
│   │       └── error.rs                        ← Task 8
│   │
│   ├── linsight-protocol/
│   │   ├── Cargo.toml                          ← Task 9
│   │   └── src/
│   │       ├── lib.rs                          ← Task 9
│   │       ├── messages.rs                     ← Tasks 10-12
│   │       └── frame.rs                        ← Task 13
│   │
│   ├── linsight-plugin-sdk/
│   │   ├── Cargo.toml                          ← Task 14
│   │   └── src/
│   │       ├── lib.rs                          ← Task 14
│   │       ├── plugin.rs                       ← Task 15
│   │       ├── manifest.rs                     ← Task 16
│   │       └── export.rs                       ← Task 17
│   │
│   ├── linsight-sensors/
│   │   └── cpu/
│   │       ├── Cargo.toml                      ← Task 18
│   │       └── src/
│   │           ├── lib.rs                      ← Task 18
│   │           ├── proc_stat.rs                ← Tasks 19-21
│   │           └── plugin.rs                   ← Task 22
│   │
│   └── linsight-cli/
│       ├── Cargo.toml                          ← Task 30
│       └── src/
│           ├── main.rs                         ← Task 30
│           └── commands/
│               ├── mod.rs                      ← Task 31
│               ├── list.rs                     ← Task 32
│               └── read.rs                     ← Task 33
│
└── apps/
    └── linsightd/
        ├── Cargo.toml                          ← Task 23
        └── src/
            ├── main.rs                         ← Task 23
            ├── runtime.rs                      ← Task 24
            ├── scheduler.rs                    ← Tasks 25-26
            ├── plugin_host.rs                  ← Task 27
            └── transport/
                ├── mod.rs                      ← Task 28
                └── unix.rs                     ← Task 29
```

End state: `cargo test --workspace` is green, `just ci` is green,
and `linsight-cli read cpu.util` streams live CPU utilization
values.

---

## Conventions used throughout this plan

- **SPDX header on every source file** (Rust uses `//`, TOML uses `#`):
  ```rust
  // SPDX-FileCopyrightText: 2026 VisorCraft LLC
  // SPDX-License-Identifier: GPL-3.0-only
  ```
- **Every task is TDD**: failing test first, run to confirm failure,
  minimal implementation, run to confirm pass, commit.
- **Commit message format:** Conventional Commits
  (`feat:`, `test:`, `chore:`, `docs:`, `refactor:`).
- **`just ci` is the gate** before each commit unless the task
  explicitly says otherwise (e.g., during the workspace bootstrap
  before there are any tests).

---

## Task 1: Workspace bootstrap — Cargo + toolchain + lint config

**Files:**
- Create: `/work/repos/visorcraft/linsight/Cargo.toml`
- Create: `/work/repos/visorcraft/linsight/rust-toolchain.toml`
- Create: `/work/repos/visorcraft/linsight/rustfmt.toml`
- Create: `/work/repos/visorcraft/linsight/deny.toml`
- Create: `/work/repos/visorcraft/linsight/LICENSE`
- Create: `/work/repos/visorcraft/linsight/README.md`
- Create: `/work/repos/visorcraft/linsight/.gitignore`

- [ ] **Step 1: Initialize git repo**

```bash
cd /work/repos/visorcraft/linsight
git init
git branch -m main
```

- [ ] **Step 2: Determine latest stable Rust**

```bash
rustup update stable
rustc +stable --version
# Note the exact version (e.g., "rustc 1.95.0 (...)"). Use that for rust-toolchain.toml.
```

- [ ] **Step 3: Write `rust-toolchain.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[toolchain]
channel = "1.95"  # ← replace with exact latest stable from Step 2
components = ["rustfmt", "clippy"]
profile = "minimal"
```

- [ ] **Step 4: Write `rustfmt.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
edition = "2024"
max_width = 100
use_small_heuristics = "Max"
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
reorder_imports = true
```

- [ ] **Step 5: Write `Cargo.toml` (workspace manifest)**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[workspace]
members = [
    "apps/linsightd",
    "crates/linsight-core",
    "crates/linsight-protocol",
    "crates/linsight-plugin-sdk",
    "crates/linsight-sensors/cpu",
    "crates/linsight-cli",
]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "GPL-3.0-only"
repository = "https://github.com/visorcraft/linsight"
authors = ["VisorCraft LLC"]
rust-version = "1.95"

[workspace.dependencies]
anyhow = "1.0"
clap = { version = "4.5", features = ["derive"] }
postcard = { version = "1.0", features = ["alloc"] }
polling = "3.7"
serde = { version = "1.0", features = ["derive"] }
stabby = "36"
thiserror = "2.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
tempfile = "3.15"
assert_cmd = "2.0"
predicates = "3.1"
proptest = "1.7"

[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
opt-level = 3
```

- [ ] **Step 6: Write `deny.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[advisories]
yanked = "deny"
ignore = []

[licenses]
allow = [
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-3-Clause",
    "GPL-3.0-only",
    "ISC",
    "MIT",
    "Unicode-3.0",
    "Unlicense",
    "Zlib",
    "0BSD",
]
confidence-threshold = 0.93

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

- [ ] **Step 7: Write `LICENSE` (full GPL-3.0 text)**

```bash
curl -fsSL https://www.gnu.org/licenses/gpl-3.0.txt -o /work/repos/visorcraft/linsight/LICENSE
```

Verify the first line is `                    GNU GENERAL PUBLIC LICENSE`.

- [ ] **Step 8: Write `README.md`**

```markdown
<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# LinSight

A fast, beautiful, modular Linux system-monitoring dashboard with
multi-GPU support and a runtime plugin system.

**Status:** in development. See [`docs/superpowers/specs/2026-05-25-linsight-design.md`](docs/superpowers/specs/2026-05-25-linsight-design.md) for the v1 design.

## Build

```bash
just ci             # fmt-check + clippy + tests
just build          # debug
just build-release  # release
```

## License

GPL-3.0-only. See `LICENSE`.
```

- [ ] **Step 9: Write `.gitignore`**

```
/target
**/*.rs.bk
Cargo.lock.bak
/.superpowers
*.swp
.DS_Store
```

Note: `Cargo.lock` IS committed for binary workspaces.

- [ ] **Step 10: Copy spec + roadmap + this plan into the new repo's docs dir if not already there**

```bash
ls /work/repos/visorcraft/linsight/docs/superpowers/specs/2026-05-25-linsight-design.md
ls /work/repos/visorcraft/linsight/docs/superpowers/plans/2026-05-25-foundation-cli-mvp.md
ls /work/repos/visorcraft/linsight/docs/superpowers/plans/2026-05-25-phases-roadmap.md
```

Expected: all three exist (they were authored before this plan executed).

- [ ] **Step 11: Verify workspace parses (no members exist yet, so this should fail cleanly)**

```bash
cd /work/repos/visorcraft/linsight
cargo metadata --format-version 1 2>&1 | head -5
```

Expected: error mentioning missing member directories. That's fine —
we add them in subsequent tasks.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "chore: bootstrap workspace, toolchain, lint config, LICENSE"
```

---

## Task 2: Justfile + AGENTS.md + CONTRIBUTING.md

**Files:**
- Create: `/work/repos/visorcraft/linsight/Justfile`
- Create: `/work/repos/visorcraft/linsight/AGENTS.md`
- Create: `/work/repos/visorcraft/linsight/CONTRIBUTING.md`

- [ ] **Step 1: Write the Justfile**

```make
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Format every crate.
fmt:
    cargo fmt --all

# Check formatting without mutating the worktree.
fmt-check:
    cargo fmt --all -- --check

# Cargo build (debug) for the entire workspace.
build:
    cargo build --workspace

# Cargo build (release) for the entire workspace.
build-release:
    cargo build --workspace --release

# Cargo build (release) tuned for x86_64-v3 (CachyOS / modern systems).
build-release-v3:
    RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --workspace --release

# Run every test in the workspace.
test:
    cargo test --workspace

# Type-check without producing binaries.
check:
    cargo check --workspace

# Strict lint pass — same gate as CI.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# License + advisory check (requires `cargo install cargo-deny`).
deny:
    cargo deny --all-features check

# Recursive audit (requires `cargo install cargo-audit`).
audit:
    cargo audit

# Run the daemon standalone (useful for manual smoke tests).
run-daemon *args:
    cargo run -p linsightd -- {{args}}

# Run the CLI (passes args through).
run-cli *args:
    cargo run -p linsight-cli -- {{args}}

# Convenience target — everything CI does.
ci: fmt-check lint test
    @echo "ci preflight passed"

# Full pre-release gate.
preflight: ci deny audit
    @echo "preflight passed"
```

- [ ] **Step 2: Write `AGENTS.md`**

```markdown
<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# AGENTS.md

Guidelines for AI assistants working on this repository.

LinSight is a Linux system-monitoring dashboard with multi-GPU
support and a runtime plugin system. Spec:
`docs/superpowers/specs/2026-05-25-linsight-design.md`.

## Build commands

Everything goes through `just`. Direct `cargo` invocations are
equivalent if `just` isn't installed.

```bash
just ci             # fmt-check + clippy + tests — same gate CI runs
just build          # cargo build --workspace (debug)
just build-release  # release
just build-release-v3   # x86_64-v3 tuned release (CachyOS-friendly)
just test           # cargo test --workspace
just lint           # cargo clippy --workspace --all-targets -- -D warnings
just fmt            # cargo fmt --all
just deny           # cargo deny --all-features check
just audit          # cargo audit
just run-daemon ARGS    # cargo run -p linsightd -- ARGS
just run-cli ARGS       # cargo run -p linsight-cli -- ARGS
```

**CI parity:** `just ci` runs `fmt --check`, `clippy -D warnings`, and
`cargo test --workspace` in that order. A red local `just ci` is a red
CI run.

**Toolchain pinning:** `rust-toolchain.toml` pins stable Rust. Do not
bump without updating this file's prerequisites table.

## Workspace layout

```
linsight/
├── apps/
│   ├── linsight-gui/        ← Qt 6 / Kirigami GUI binary (later phases)
│   └── linsightd/           ← daemon binary (this phase)
├── crates/
│   ├── linsight-core/       ← shared types (no I/O)
│   ├── linsight-protocol/   ← postcard wire types + framing
│   ├── linsight-plugin-sdk/ ← stabby trait + export macro (public)
│   ├── linsight-sensors/    ← built-in sensors (in-tree plugins)
│   │   └── cpu/             ← first sensor
│   ├── linsight-cli/        ← CLI binary
│   └── linsight-i18n/       ← Fluent bundle (later phases)
```

## Code conventions

- **Edition 2024, latest stable Rust.** Do not require nightly.
- **No async runtime in the daemon hot path.** Sync + `polling` only.
- **SPDX REUSE headers required.** Every new source file gets:
  ```
  // SPDX-FileCopyrightText: 2026 VisorCraft LLC
  // SPDX-License-Identifier: GPL-3.0-only
  ```
- **GPL-3.0-only.** New deps must use a license in `deny.toml`'s
  allowlist.
- **Default to no comments.** Only add one when the WHY is non-obvious.
- **`tracing::*!` for structured logs.** Default filter is `info`;
  override via `LINSIGHT_LOG`.

## Tests

```bash
cargo test --workspace
```

Sensor crates test against synthetic `/sys` fixtures using
`tempfile::TempDir`. Hardware-dependent tests are `#[ignore]` by
default.

## Reporting

- Bugs / feature requests: GitHub issues at
  <https://github.com/visorcraft/linsight/issues>.
- Security: see `docs/security.md` (added in a later phase).
```

- [ ] **Step 3: Write `CONTRIBUTING.md`**

```markdown
<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Contributing to LinSight

Thanks for your interest. LinSight accepts:

1. Bug reports and feature requests via GitHub Issues.
2. Pull requests for code, docs, translations, and packaging.
3. Hardware plugins — see `docs/plugin-sdk.md` (added in a later phase).

## Dev loop

```bash
just ci   # must be green before opening a PR
```

## Commit messages

Conventional Commits: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`,
`test:`, `perf:`.

## License

By contributing, you agree your contribution is licensed under
GPL-3.0-only.
```

- [ ] **Step 4: Verify `just` is available**

```bash
just --version
```

If not present: `cargo install just` or install via the system package
manager.

- [ ] **Step 5: Run the (empty) CI target — should fail because no
crates exist yet**

```bash
cd /work/repos/visorcraft/linsight
just ci 2>&1 | head -20
```

Expected: failure during `cargo fmt --all -- --check` or `cargo
clippy` because there are no crates. That's fine. Subsequent tasks
add crates.

- [ ] **Step 6: Commit**

```bash
git add Justfile AGENTS.md CONTRIBUTING.md
git commit -m "chore: add Justfile + AGENTS.md + CONTRIBUTING.md"
```

---

## Task 3: linsight-core crate skeleton

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-core/Cargo.toml`
- Create: `/work/repos/visorcraft/linsight/crates/linsight-core/src/lib.rs`

- [ ] **Step 1: Write `crates/linsight-core/Cargo.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "linsight-core"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "LinSight shared types and pure-logic primitives."

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
```

- [ ] **Step 2: Write `crates/linsight-core/src/lib.rs`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2024_idioms)]

pub mod types;
pub mod error;

pub use error::{CoreError, CoreResult};
pub use types::*;
```

- [ ] **Step 3: Verify the crate compiles**

```bash
cd /work/repos/visorcraft/linsight
cargo check -p linsight-core 2>&1 | tail -10
```

Expected: error mentioning that `types` and `error` modules don't
exist yet. That's the next two tasks.

- [ ] **Step 4: Commit**

```bash
git add crates/linsight-core/Cargo.toml crates/linsight-core/src/lib.rs
git commit -m "feat(core): add linsight-core crate skeleton"
```

---

## Task 4: linsight-core — SensorId newtype

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-core/src/types.rs`

- [ ] **Step 1: Write the failing test**

Add this to a new file `crates/linsight-core/src/types.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_id_displays_as_its_string() {
        let id = SensorId::new("cpu.util");
        assert_eq!(id.to_string(), "cpu.util");
    }

    #[test]
    fn sensor_id_rejects_empty() {
        assert!(SensorId::try_new("").is_err());
    }

    #[test]
    fn sensor_id_rejects_whitespace() {
        assert!(SensorId::try_new("cpu util").is_err());
        assert!(SensorId::try_new("cpu\tutil").is_err());
    }

    #[test]
    fn sensor_id_orders_lexicographically() {
        let a = SensorId::new("cpu.util");
        let b = SensorId::new("mem.used");
        assert!(a < b);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd /work/repos/visorcraft/linsight
cargo test -p linsight-core 2>&1 | tail -15
```

Expected: compile error (`SensorId` is undefined).

- [ ] **Step 3: Write the minimal implementation**

Prepend to `crates/linsight-core/src/types.rs` (above the `#[cfg(test)]` block):

```rust
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::{CoreError, CoreResult};

/// Stable, human-readable sensor identifier.
///
/// Convention: dot-separated path of lowercase ASCII identifiers,
/// e.g., `"cpu.util"`, `"xe.gpu1.temp_c"`. The string never contains
/// whitespace and is never empty.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SensorId(String);

impl SensorId {
    /// Construct from a static-known good string.
    ///
    /// Panics in debug builds if the string violates the invariants
    /// (empty / contains whitespace). Use [`SensorId::try_new`] for
    /// runtime-derived strings.
    pub fn new(s: impl Into<String>) -> Self {
        let s = s.into();
        debug_assert!(
            !s.is_empty() && !s.chars().any(char::is_whitespace),
            "SensorId invariant violated: {s:?}"
        );
        Self(s)
    }

    /// Fallible constructor for runtime-derived strings.
    pub fn try_new(s: impl Into<String>) -> CoreResult<Self> {
        let s = s.into();
        if s.is_empty() {
            return Err(CoreError::InvalidSensorId("empty".into()));
        }
        if s.chars().any(char::is_whitespace) {
            return Err(CoreError::InvalidSensorId(format!("whitespace in {s:?}")));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SensorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SensorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SensorId({})", self.0)
    }
}
```

Also create the error module stub at `crates/linsight-core/src/error.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid sensor id: {0}")]
    InvalidSensorId(String),
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p linsight-core 2>&1 | tail -10
```

Expected: 4 tests passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-core/src/types.rs crates/linsight-core/src/error.rs
git commit -m "feat(core): add SensorId newtype with validated constructor"
```

---

## Task 5: linsight-core — Unit, Category, SensorKind enums

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-core/src/types.rs`

- [ ] **Step 1: Add the failing tests**

Append inside the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn unit_displays_with_symbol() {
        assert_eq!(Unit::Percent.symbol(), "%");
        assert_eq!(Unit::Celsius.symbol(), "°C");
        assert_eq!(Unit::Bytes.symbol(), "B");
        assert_eq!(Unit::BytesPerSec.symbol(), "B/s");
        assert_eq!(Unit::Hertz.symbol(), "Hz");
        assert_eq!(Unit::Watts.symbol(), "W");
        assert_eq!(Unit::Volts.symbol(), "V");
        assert_eq!(Unit::Rpm.symbol(), "rpm");
        assert_eq!(Unit::Count.symbol(), "");
        assert_eq!(Unit::Custom("foo".into()).symbol(), "foo");
    }

    #[test]
    fn category_round_trips_through_serde() {
        let original = Category::Gpu;
        let encoded = serde_json::to_string(&original).unwrap();
        let decoded: Category = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn sensor_kind_round_trips_through_serde() {
        for kind in [SensorKind::Scalar, SensorKind::Counter, SensorKind::Table, SensorKind::State] {
            let encoded = serde_json::to_string(&kind).unwrap();
            let decoded: SensorKind = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, kind);
        }
    }
```

The tests above use `serde_json` — add it as a `dev-dependency`:

```toml
# crates/linsight-core/Cargo.toml — under [dev-dependencies]
serde_json = "1.0"
```

Also add `serde_json` to the workspace dependencies (we'll need it
elsewhere) by editing the root `Cargo.toml`:

```toml
# Cargo.toml workspace [workspace.dependencies] section, alphabetical insertion:
serde_json = "1.0"
```

Then revise `crates/linsight-core/Cargo.toml`'s dev-dependencies:

```toml
[dev-dependencies]
proptest = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 2: Run to confirm tests fail to compile**

```bash
cargo test -p linsight-core 2>&1 | tail -15
```

Expected: errors about `Unit`, `Category`, `SensorKind` being undefined.

- [ ] **Step 3: Implement the enums**

Add to `crates/linsight-core/src/types.rs` (after the `SensorId` block,
before the test module):

```rust
/// Measurement unit for a sensor value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    Percent,
    Celsius,
    Bytes,
    BytesPerSec,
    Hertz,
    Watts,
    Volts,
    Rpm,
    Count,
    Custom(String),
}

impl Unit {
    pub fn symbol(&self) -> &str {
        match self {
            Unit::Percent => "%",
            Unit::Celsius => "°C",
            Unit::Bytes => "B",
            Unit::BytesPerSec => "B/s",
            Unit::Hertz => "Hz",
            Unit::Watts => "W",
            Unit::Volts => "V",
            Unit::Rpm => "rpm",
            Unit::Count => "",
            Unit::Custom(s) => s,
        }
    }
}

/// High-level grouping for the dashboard UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    Cpu,
    Gpu,
    Memory,
    Storage,
    Network,
    Custom,
}

/// Shape of values a sensor emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SensorKind {
    /// Continuous numeric value (utilization, temperature, etc.).
    Scalar,
    /// Monotonically increasing counter (bytes transferred, etc.).
    Counter,
    /// Tabular value (process list, etc.).
    Table,
    /// Discrete labeled state (power state, link status, etc.).
    State,
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p linsight-core 2>&1 | tail -10
```

Expected: 7 tests passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-core/Cargo.toml crates/linsight-core/src/types.rs Cargo.toml
git commit -m "feat(core): add Unit, Category, SensorKind enums"
```

---

## Task 6: linsight-core — Reading enum + TableRow

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-core/src/types.rs`

- [ ] **Step 1: Add the failing tests**

Append inside the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn reading_round_trips_scalar() {
        let r = Reading::Scalar(42.5);
        let encoded = serde_json::to_string(&r).unwrap();
        let decoded: Reading = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, r);
    }

    #[test]
    fn reading_round_trips_table() {
        let r = Reading::Table(vec![
            TableRow {
                cells: vec![
                    Cell::Text("firefox".into()),
                    Cell::Number(1234.0),
                    Cell::Bytes(50_000_000),
                ],
            },
        ]);
        let encoded = serde_json::to_string(&r).unwrap();
        let decoded: Reading = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, r);
    }

    #[test]
    fn reading_round_trips_state_and_counter() {
        for r in [Reading::Counter(999_999), Reading::State("P0".into())] {
            let encoded = serde_json::to_string(&r).unwrap();
            let decoded: Reading = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, r);
        }
    }
```

- [ ] **Step 2: Run to confirm tests fail to compile**

```bash
cargo test -p linsight-core 2>&1 | tail -10
```

Expected: errors about `Reading`, `TableRow`, `Cell` undefined.

- [ ] **Step 3: Implement**

Add to `crates/linsight-core/src/types.rs` (after the `SensorKind` block):

```rust
/// One sample value as emitted by a sensor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reading {
    Scalar(f64),
    Counter(u64),
    Table(Vec<TableRow>),
    State(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cell {
    Text(String),
    Number(f64),
    Bytes(u64),
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-core 2>&1 | tail -5
```

Expected: 10 tests passed (4 + 3 + 3).

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-core/src/types.rs
git commit -m "feat(core): add Reading enum with TableRow + Cell"
```

---

## Task 7: linsight-core — Sample struct (sensor + timestamp + reading)

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-core/src/types.rs`

- [ ] **Step 1: Add the failing test**

```rust
    #[test]
    fn sample_holds_id_ts_and_reading() {
        let s = Sample {
            sensor: SensorId::new("cpu.util"),
            ts_micros: 1_700_000_000_000_000,
            reading: Reading::Scalar(33.3),
        };
        assert_eq!(s.sensor.as_str(), "cpu.util");
        assert_eq!(s.ts_micros, 1_700_000_000_000_000);
        assert!(matches!(s.reading, Reading::Scalar(v) if v == 33.3));
    }
```

- [ ] **Step 2: Run, confirm failure**

```bash
cargo test -p linsight-core 2>&1 | tail -5
```

Expected: `Sample` undefined.

- [ ] **Step 3: Implement**

```rust
/// One sensor reading at a point in time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub sensor: SensorId,
    /// Microseconds since the Unix epoch (UTC).
    pub ts_micros: u64,
    pub reading: Reading,
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-core 2>&1 | tail -5
```

Expected: 11 passed.

- [ ] **Step 5: `just ci`**

```bash
cd /work/repos/visorcraft/linsight
just ci
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/linsight-core/src/types.rs
git commit -m "feat(core): add Sample struct"
```

---

## Task 8: linsight-core — finalise error types

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-core/src/error.rs`

- [ ] **Step 1: Add failing tests in `error.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_sensor_id_display() {
        let e = CoreError::InvalidSensorId("oops".into());
        assert_eq!(e.to_string(), "invalid sensor id: oops");
    }

    #[test]
    fn migration_error_display() {
        let e = CoreError::MigrationFailed { from: 1, to: 2 };
        assert_eq!(e.to_string(), "dashboard schema migration failed: v1 → v2");
    }
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-core 2>&1 | tail -5
```

Expected: `MigrationFailed` not yet defined.

- [ ] **Step 3: Expand the error enum**

Replace `crates/linsight-core/src/error.rs` with:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use thiserror::Error;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid sensor id: {0}")]
    InvalidSensorId(String),

    #[error("dashboard schema migration failed: v{from} → v{to}")]
    MigrationFailed { from: u32, to: u32 },
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-core 2>&1 | tail -5
```

Expected: 13 passed.

- [ ] **Step 5: `just ci`**

```bash
just ci
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/linsight-core/src/error.rs
git commit -m "feat(core): expand CoreError with MigrationFailed variant"
```

---

## Task 9: linsight-protocol crate skeleton

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-protocol/Cargo.toml`
- Create: `/work/repos/visorcraft/linsight/crates/linsight-protocol/src/lib.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "linsight-protocol"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "LinSight daemon ↔ client wire protocol (postcard-framed)."

[dependencies]
linsight-core = { path = "../linsight-core" }
postcard = { workspace = true }
serde = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Write `src/lib.rs`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2024_idioms)]

pub mod messages;
pub mod frame;

pub use messages::*;
pub use frame::{FrameError, FrameReader, FrameWriter};

/// Wire-format protocol version. Bump only on breaking changes.
pub const PROTOCOL_VERSION: u32 = 1;
```

- [ ] **Step 3: Confirm it compiles (will fail — modules empty)**

```bash
cargo check -p linsight-protocol 2>&1 | tail -10
```

Expected: errors about missing `messages` and `frame` modules. Next tasks.

- [ ] **Step 4: Commit**

```bash
git add crates/linsight-protocol/
git commit -m "feat(protocol): add linsight-protocol crate skeleton"
```

---

## Task 10: linsight-protocol — Hello / Welcome messages

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-protocol/src/messages.rs`

- [ ] **Step 1: Add failing tests**

Create `crates/linsight-protocol/src/messages.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use linsight_core::{SensorId, Category, SensorKind, Unit, Reading};
use serde::{Deserialize, Serialize};

// ... message definitions go in Step 3.

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: T) {
        let bytes = postcard::to_allocvec(&v).unwrap();
        let back: T = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn hello_round_trips() {
        round_trip(ClientMsg::Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            client_name: "test".into(),
        });
    }

    #[test]
    fn welcome_round_trips() {
        round_trip(ServerMsg::Welcome {
            protocol_version: crate::PROTOCOL_VERSION,
            daemon_version: "0.1.0".into(),
            plugins: vec![],
        });
    }
}
```

- [ ] **Step 2: Run, confirm failure**

```bash
cargo test -p linsight-protocol 2>&1 | tail -10
```

Expected: `ClientMsg` / `ServerMsg` undefined.

- [ ] **Step 3: Add the message definitions**

Insert in `crates/linsight-protocol/src/messages.rs` above the test
module:

```rust
/// A client → daemon message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message after socket connect. The daemon replies with Welcome or
    /// disconnects on protocol mismatch.
    Hello {
        protocol_version: u32,
        client_name: String,
    },
    /// Request the daemon's full sensor list.
    ListSensors,
    /// Subscribe to a set of sensors. `rate_hz = None` means "use the
    /// sensor's native rate."
    Subscribe {
        sensors: Vec<SensorId>,
        rate_hz: Option<f32>,
    },
    /// Stop receiving samples for the given sensors.
    Unsubscribe {
        sensors: Vec<SensorId>,
    },
    /// Polite shutdown signal so the daemon can release subscriptions
    /// immediately rather than waiting for socket close.
    Goodbye,
}

/// A daemon → client message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Reply to Hello.
    Welcome {
        protocol_version: u32,
        daemon_version: String,
        plugins: Vec<PluginInfo>,
    },
    /// Reply to ListSensors.
    SensorList(Vec<SensorInfo>),
    /// Pushed continuously while subscribed.
    Sample(linsight_core::Sample),
    /// A sensor has been degraded (e.g., plugin panic).
    SensorDegraded {
        sensor: SensorId,
        reason: String,
    },
    /// Daemon is going away (e.g., systemd stop).
    Bye {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
    pub sensor_count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorInfo {
    pub id: SensorId,
    pub display_name: String,
    pub unit: Unit,
    pub kind: SensorKind,
    pub category: Category,
    pub native_rate_hz: f32,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub device_id: Option<String>,
    pub plugin_id: String,
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-protocol 2>&1 | tail -5
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-protocol/src/messages.rs
git commit -m "feat(protocol): Hello / Welcome / Subscribe / Sample messages"
```

---

## Task 11: linsight-protocol — exhaustive message round-trip tests

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-protocol/src/messages.rs`

- [ ] **Step 1: Add tests for every message variant**

Append to the `#[cfg(test)] mod tests` block:

```rust
    use linsight_core::Sample;

    #[test]
    fn subscribe_round_trips() {
        round_trip(ClientMsg::Subscribe {
            sensors: vec![SensorId::new("cpu.util"), SensorId::new("mem.used")],
            rate_hz: Some(2.0),
        });
    }

    #[test]
    fn unsubscribe_round_trips() {
        round_trip(ClientMsg::Unsubscribe {
            sensors: vec![SensorId::new("cpu.util")],
        });
    }

    #[test]
    fn list_sensors_round_trips() {
        round_trip(ClientMsg::ListSensors);
    }

    #[test]
    fn goodbye_round_trips() {
        round_trip(ClientMsg::Goodbye);
    }

    #[test]
    fn sample_message_round_trips() {
        round_trip(ServerMsg::Sample(Sample {
            sensor: SensorId::new("cpu.util"),
            ts_micros: 1_700_000_000_000_000,
            reading: Reading::Scalar(42.0),
        }));
    }

    #[test]
    fn sensor_list_round_trips() {
        round_trip(ServerMsg::SensorList(vec![SensorInfo {
            id: SensorId::new("cpu.util"),
            display_name: "CPU utilization".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 1.0,
            min: Some(0.0),
            max: Some(100.0),
            device_id: None,
            plugin_id: "com.visorcraft.linsight.cpu".into(),
        }]));
    }

    #[test]
    fn degraded_round_trips() {
        round_trip(ServerMsg::SensorDegraded {
            sensor: SensorId::new("cpu.util"),
            reason: "panic in sample()".into(),
        });
    }

    #[test]
    fn bye_round_trips() {
        round_trip(ServerMsg::Bye { reason: "systemd stop".into() });
    }
```

- [ ] **Step 2: Run**

```bash
cargo test -p linsight-protocol 2>&1 | tail -5
```

Expected: 10 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/linsight-protocol/src/messages.rs
git commit -m "test(protocol): exhaustive round-trip coverage for every message"
```

---

## Task 12: linsight-protocol — version handshake helper

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-protocol/src/messages.rs`

- [ ] **Step 1: Add failing tests**

```rust
    #[test]
    fn handshake_accepts_matching_version() {
        let hello = ClientMsg::Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            client_name: "x".into(),
        };
        assert!(verify_hello(&hello).is_ok());
    }

    #[test]
    fn handshake_rejects_mismatched_version() {
        let hello = ClientMsg::Hello {
            protocol_version: 999,
            client_name: "x".into(),
        };
        assert!(matches!(verify_hello(&hello), Err(HandshakeError::VersionMismatch { client: 999, daemon: 1 })));
    }

    #[test]
    fn handshake_rejects_non_hello() {
        let bad = ClientMsg::ListSensors;
        assert!(matches!(verify_hello(&bad), Err(HandshakeError::NotHello)));
    }
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-protocol 2>&1 | tail -5
```

Expected: `verify_hello` / `HandshakeError` undefined.

- [ ] **Step 3: Implement**

Add to `crates/linsight-protocol/src/messages.rs`:

```rust
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum HandshakeError {
    #[error("first message must be Hello")]
    NotHello,
    #[error("protocol version mismatch: client={client} daemon={daemon}")]
    VersionMismatch { client: u32, daemon: u32 },
}

pub fn verify_hello(msg: &ClientMsg) -> Result<&str, HandshakeError> {
    match msg {
        ClientMsg::Hello { protocol_version, client_name } => {
            if *protocol_version != crate::PROTOCOL_VERSION {
                Err(HandshakeError::VersionMismatch {
                    client: *protocol_version,
                    daemon: crate::PROTOCOL_VERSION,
                })
            } else {
                Ok(client_name)
            }
        }
        _ => Err(HandshakeError::NotHello),
    }
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-protocol 2>&1 | tail -5
```

Expected: 13 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-protocol/src/messages.rs
git commit -m "feat(protocol): add verify_hello() handshake helper"
```

---

## Task 13: linsight-protocol — length-prefixed framing

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-protocol/src/frame.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/linsight-protocol/src/frame.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::io::{self, Read, Write};
use thiserror::Error;

use crate::messages::{ClientMsg, ServerMsg};

// ... implementation in Step 3.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROTOCOL_VERSION;
    use std::io::Cursor;

    #[test]
    fn write_then_read_client_msg() {
        let original = ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".into(),
        };

        let mut buf: Vec<u8> = Vec::new();
        FrameWriter::new(&mut buf).write_client(&original).unwrap();

        let mut reader = FrameReader::new(Cursor::new(buf));
        let back = reader.read_client().unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn write_then_read_server_msg() {
        let original = ServerMsg::Welcome {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: "0.1.0".into(),
            plugins: vec![],
        };
        let mut buf: Vec<u8> = Vec::new();
        FrameWriter::new(&mut buf).write_server(&original).unwrap();

        let mut reader = FrameReader::new(Cursor::new(buf));
        let back = reader.read_server().unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn read_rejects_oversized_frame() {
        let mut bad = vec![0xff, 0xff, 0xff, 0xff];   // length = u32::MAX
        bad.extend_from_slice(b"junk");
        let mut reader = FrameReader::new(Cursor::new(bad));
        let err = reader.read_client().unwrap_err();
        assert!(matches!(err, FrameError::Oversized(_)));
    }

    #[test]
    fn multiple_frames_in_one_buffer() {
        let m1 = ClientMsg::Goodbye;
        let m2 = ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "x".into(),
        };

        let mut buf: Vec<u8> = Vec::new();
        let mut w = FrameWriter::new(&mut buf);
        w.write_client(&m1).unwrap();
        w.write_client(&m2).unwrap();

        let mut r = FrameReader::new(Cursor::new(buf));
        assert_eq!(r.read_client().unwrap(), m1);
        assert_eq!(r.read_client().unwrap(), m2);
    }
}
```

- [ ] **Step 2: Run, confirm failure**

```bash
cargo test -p linsight-protocol 2>&1 | tail -10
```

Expected: `FrameReader` / `FrameWriter` / `FrameError` undefined.

- [ ] **Step 3: Implement**

Add to `crates/linsight-protocol/src/frame.rs` (above the test module):

```rust
/// Cap on a single frame's body. 1 MiB is far larger than any
/// realistic LinSight message; anything bigger is treated as
/// adversarial / corrupted.
pub const MAX_FRAME_BYTES: u32 = 1 << 20;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("decode: {0}")]
    Decode(#[from] postcard::Error),
    #[error("frame larger than MAX_FRAME_BYTES: {0} bytes")]
    Oversized(u32),
    #[error("connection closed")]
    Closed,
}

pub struct FrameReader<R: Read> { inner: R }

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self { Self { inner } }

    pub fn read_client(&mut self) -> Result<ClientMsg, FrameError> {
        let bytes = self.read_frame()?;
        Ok(postcard::from_bytes(&bytes)?)
    }

    pub fn read_server(&mut self) -> Result<ServerMsg, FrameError> {
        let bytes = self.read_frame()?;
        Ok(postcard::from_bytes(&bytes)?)
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, FrameError> {
        let mut len_buf = [0u8; 4];
        match self.inner.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
            Err(e) => return Err(FrameError::Io(e)),
        }
        let len = u32::from_le_bytes(len_buf);
        if len > MAX_FRAME_BYTES {
            return Err(FrameError::Oversized(len));
        }
        let mut body = vec![0u8; len as usize];
        self.inner.read_exact(&mut body)?;
        Ok(body)
    }
}

pub struct FrameWriter<W: Write> { inner: W }

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W) -> Self { Self { inner } }

    pub fn write_client(&mut self, msg: &ClientMsg) -> Result<(), FrameError> {
        let bytes = postcard::to_allocvec(msg)?;
        self.write_frame(&bytes)
    }

    pub fn write_server(&mut self, msg: &ServerMsg) -> Result<(), FrameError> {
        let bytes = postcard::to_allocvec(msg)?;
        self.write_frame(&bytes)
    }

    fn write_frame(&mut self, body: &[u8]) -> Result<(), FrameError> {
        let len = body.len() as u32;
        if len > MAX_FRAME_BYTES {
            return Err(FrameError::Oversized(len));
        }
        self.inner.write_all(&len.to_le_bytes())?;
        self.inner.write_all(body)?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-protocol 2>&1 | tail -5
```

Expected: 17 passed.

- [ ] **Step 5: `just ci`**

```bash
just ci
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/linsight-protocol/src/frame.rs
git commit -m "feat(protocol): add length-prefixed FrameReader / FrameWriter"
```

---

## Task 14: linsight-plugin-sdk crate skeleton

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-plugin-sdk/Cargo.toml`
- Create: `/work/repos/visorcraft/linsight/crates/linsight-plugin-sdk/src/lib.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "linsight-plugin-sdk"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "Public SDK for authoring LinSight sensor plugins."

[dependencies]
linsight-core = { path = "../linsight-core" }
stabby = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Write `src/lib.rs`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![deny(rust_2024_idioms)]

pub mod plugin;
pub mod manifest;
pub mod export;

pub use plugin::*;
pub use manifest::*;

/// Bump only on breaking changes to the plugin ABI. The daemon refuses
/// to load plugins whose returned abi version does not match this
/// constant.
pub const LINSIGHT_PLUGIN_ABI_VERSION: u32 = 1;
```

- [ ] **Step 3: Check compile (will fail — modules empty)**

```bash
cargo check -p linsight-plugin-sdk 2>&1 | tail -10
```

Expected: errors about missing modules.

- [ ] **Step 4: Commit**

```bash
git add crates/linsight-plugin-sdk/
git commit -m "feat(sdk): add linsight-plugin-sdk crate skeleton"
```

---

## Task 15: linsight-plugin-sdk — LinsightPlugin trait + supporting types

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-plugin-sdk/src/plugin.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/linsight-plugin-sdk/src/plugin.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use linsight_core::{SensorId, Reading};

// ... definitions in Step 3.

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::SensorId;

    struct NoopPlugin;

    impl LinsightPlugin for NoopPlugin {
        fn init(&self, _ctx: &PluginCtx) -> Result<crate::PluginManifest, PluginError> {
            Ok(crate::PluginManifest {
                plugin_id: "test".into(),
                display_name: "Test".into(),
                version: "0.0.1".into(),
                sensors: vec![],
            })
        }

        fn sample(&self, _sensor: SensorId) -> Result<Reading, PluginError> {
            Err(PluginError::Unsupported("no sensors".into()))
        }
    }

    #[test]
    fn noop_plugin_init_runs() {
        let p = NoopPlugin;
        let m = p.init(&PluginCtx::default()).unwrap();
        assert_eq!(m.plugin_id, "test");
    }

    #[test]
    fn noop_plugin_sample_errors() {
        let p = NoopPlugin;
        let err = p.sample(SensorId::new("foo")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }
}
```

- [ ] **Step 2: Run, confirm failure**

```bash
cargo test -p linsight-plugin-sdk 2>&1 | tail -10
```

Expected: undefined `LinsightPlugin`, `PluginCtx`, `PluginError`.

- [ ] **Step 3: Implement**

Add to `crates/linsight-plugin-sdk/src/plugin.rs`:

```rust
use crate::PluginManifest;
use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum PluginError {
    #[error("io: {0}")]
    Io(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("unsupported sensor: {0}")]
    Unsupported(String),
    #[error("transient: {0}")]
    Transient(String),
}

/// Read-only context passed to `init`. v1 has nothing in it; future
/// fields (logger handle, sysroot override, etc.) extend this struct.
#[derive(Default)]
pub struct PluginCtx {
    /// Override the filesystem root for sensor reads. `None` means use `/`.
    /// Tests use this to point plugins at a synthetic sysfs.
    pub sysroot: Option<std::path::PathBuf>,
}

pub trait LinsightPlugin: Send + Sync {
    fn init(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError>;
    fn sample(&self, sensor: SensorId) -> Result<Reading, PluginError>;
    fn shutdown(&self) {}
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-plugin-sdk 2>&1 | tail -5
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-plugin-sdk/src/plugin.rs
git commit -m "feat(sdk): LinsightPlugin trait + PluginCtx + PluginError"
```

---

## Task 16: linsight-plugin-sdk — Manifest + SensorDescriptor

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-plugin-sdk/src/manifest.rs`

- [ ] **Step 1: Add failing tests**

Create `crates/linsight-plugin-sdk/src/manifest.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use linsight_core::{Category, SensorId, SensorKind, Unit};

// ... defs in Step 3.

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::SensorId;

    #[test]
    fn sensor_descriptor_clamps_native_rate() {
        let d = SensorDescriptor {
            id: SensorId::new("foo"),
            display_name: "Foo".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 99.0,    // out of range; will be clamped at use site
            min: None,
            max: None,
            device_id: None,
        };
        assert_eq!(d.clamped_rate_hz(), 20.0);
    }

    #[test]
    fn sensor_descriptor_clamps_low_rate() {
        let d = SensorDescriptor {
            id: SensorId::new("foo"),
            display_name: "Foo".into(),
            unit: Unit::Percent,
            kind: SensorKind::Scalar,
            category: Category::Cpu,
            native_rate_hz: 0.001,
            min: None,
            max: None,
            device_id: None,
        };
        assert_eq!(d.clamped_rate_hz(), 0.1);
    }
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-plugin-sdk 2>&1 | tail -5
```

Expected: undefined `SensorDescriptor`.

- [ ] **Step 3: Implement**

Add above the test module:

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct PluginManifest {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
    pub sensors: Vec<SensorDescriptor>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensorDescriptor {
    pub id: SensorId,
    pub display_name: String,
    pub unit: Unit,
    pub kind: SensorKind,
    pub category: Category,
    pub native_rate_hz: f32,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub device_id: Option<String>,
}

impl SensorDescriptor {
    /// Native rate hint, clamped into the scheduler's accepted range.
    pub fn clamped_rate_hz(&self) -> f32 {
        self.native_rate_hz.clamp(0.1, 20.0)
    }
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-plugin-sdk 2>&1 | tail -5
```

Expected: 4 passed (2 from plugin.rs + 2 here).

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-plugin-sdk/src/manifest.rs
git commit -m "feat(sdk): PluginManifest + SensorDescriptor + rate clamp"
```

---

## Task 17: linsight-plugin-sdk — `export_plugin!` macro

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-plugin-sdk/src/export.rs`

For v1 we use a simple `extern "C"` registration with `Box<dyn LinsightPlugin>`
returned as a raw pointer. `stabby` integration tightens the ABI in
the dynamic-loading plan (Plan 5). In this plan, built-in sensors are
statically linked, so the macro just needs to be **defined** and to
work for symmetry; full FFI exercising happens later.

- [ ] **Step 1: Add failing test**

Create `crates/linsight-plugin-sdk/src/export.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

#[cfg(test)]
mod tests {
    use crate::{LinsightPlugin, PluginCtx, PluginError, PluginManifest};
    use linsight_core::{Reading, SensorId};

    struct EchoPlugin;

    impl LinsightPlugin for EchoPlugin {
        fn init(&self, _ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
            Ok(PluginManifest {
                plugin_id: "echo".into(),
                display_name: "Echo".into(),
                version: "0.0.1".into(),
                sensors: vec![],
            })
        }

        fn sample(&self, _: SensorId) -> Result<Reading, PluginError> {
            Ok(Reading::Scalar(1.0))
        }
    }

    crate::export_plugin!(EchoPlugin);

    #[test]
    fn macro_emits_abi_version_symbol() {
        assert_eq!(linsight_plugin_abi_version(), crate::LINSIGHT_PLUGIN_ABI_VERSION);
    }

    #[test]
    fn macro_emits_factory_symbol() {
        let plugin: Box<dyn LinsightPlugin> = unsafe { Box::from_raw(linsight_plugin_v1()) };
        let manifest = plugin.init(&PluginCtx::default()).unwrap();
        assert_eq!(manifest.plugin_id, "echo");
    }
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-plugin-sdk 2>&1 | tail -10
```

Expected: `export_plugin!` is not defined.

- [ ] **Step 3: Implement**

Add to `crates/linsight-plugin-sdk/src/export.rs` (above the test module):

```rust
/// Define the entry points an out-of-tree plugin's `cdylib` must export.
///
/// Pass the type that implements `LinsightPlugin`. The type must be
/// constructible with `Default::default()`.
#[macro_export]
macro_rules! export_plugin {
    ($ty:ty) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn linsight_plugin_abi_version() -> u32 {
            $crate::LINSIGHT_PLUGIN_ABI_VERSION
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn linsight_plugin_v1() -> *mut dyn $crate::LinsightPlugin {
            let boxed: Box<dyn $crate::LinsightPlugin> = Box::new(<$ty as Default>::default());
            Box::into_raw(boxed)
        }
    };
}
```

`EchoPlugin` in the test needs `Default`:

```rust
#[derive(Default)]
struct EchoPlugin;
```

(Update the test to include the derive.)

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-plugin-sdk 2>&1 | tail -5
```

Expected: 6 passed.

- [ ] **Step 5: `just ci`**

```bash
just ci
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/linsight-plugin-sdk/src/export.rs
git commit -m "feat(sdk): export_plugin! macro for cdylib registration"
```

---

## Task 18: linsight-sensors-cpu crate skeleton + plugin shell

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-sensors/cpu/Cargo.toml`
- Create: `/work/repos/visorcraft/linsight/crates/linsight-sensors/cpu/src/lib.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "linsight-sensors-cpu"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "CPU sensor backend for LinSight (built-in plugin)."

[dependencies]
linsight-core = { path = "../../linsight-core" }
linsight-plugin-sdk = { path = "../../linsight-plugin-sdk" }
thiserror = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Write skeleton `src/lib.rs`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2024_idioms)]

mod proc_stat;
mod plugin;

pub use plugin::CpuPlugin;
```

- [ ] **Step 3: Verify (will fail until modules added)**

```bash
cargo check -p linsight-sensors-cpu 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add crates/linsight-sensors/cpu/
git commit -m "feat(sensors-cpu): add crate skeleton"
```

---

## Task 19: linsight-sensors-cpu — parse /proc/stat aggregate line

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-sensors/cpu/src/proc_stat.rs`

- [ ] **Step 1: Write failing tests**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// ... defs in Step 3.

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
cpu  100 0 50 1000 0 0 0 0 0 0
cpu0 50 0 25 500 0 0 0 0 0 0
cpu1 50 0 25 500 0 0 0 0 0 0
intr 1234567
ctxt 8910
btime 1700000000
processes 5000
procs_running 1
procs_blocked 0
";

    #[test]
    fn parse_aggregate_line() {
        let s = parse_proc_stat(SAMPLE).unwrap();
        assert_eq!(s.user, 100);
        assert_eq!(s.system, 50);
        assert_eq!(s.idle, 1000);
        // total = user + nice + system + idle + iowait + irq + softirq + steal
        assert_eq!(s.total(), 1150);
        assert_eq!(s.busy(), 150);
    }

    #[test]
    fn parse_missing_aggregate_errors() {
        let s = "cpu0 50 0 25 500 0 0 0 0 0 0\n";
        assert!(parse_proc_stat(s).is_err());
    }

    #[test]
    fn parse_short_aggregate_errors() {
        let s = "cpu 100 0\n";
        assert!(parse_proc_stat(s).is_err());
    }
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-sensors-cpu 2>&1 | tail -5
```

Expected: `parse_proc_stat` / `Stat` undefined.

- [ ] **Step 3: Implement**

Add above the test module:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StatError {
    #[error("missing aggregate cpu line")]
    MissingAggregate,
    #[error("aggregate cpu line too short (need ≥ 8 fields)")]
    TooShort,
    #[error("non-numeric field in cpu line: {0}")]
    BadNumber(String),
}

/// Parsed counters from the aggregate `cpu` line of `/proc/stat`.
/// All values are clock ticks since boot (USER_HZ).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl Stat {
    pub fn total(self) -> u64 {
        self.user + self.nice + self.system + self.idle + self.iowait + self.irq + self.softirq + self.steal
    }

    pub fn busy(self) -> u64 {
        self.total() - self.idle - self.iowait
    }
}

pub fn parse_proc_stat(s: &str) -> Result<Stat, StatError> {
    let line = s.lines().next().ok_or(StatError::MissingAggregate)?;
    let mut it = line.split_whitespace();
    let first = it.next().ok_or(StatError::MissingAggregate)?;
    if first != "cpu" {
        return Err(StatError::MissingAggregate);
    }
    let parse_field = |it: &mut std::str::SplitWhitespace<'_>| -> Result<u64, StatError> {
        let tok = it.next().ok_or(StatError::TooShort)?;
        tok.parse::<u64>().map_err(|_| StatError::BadNumber(tok.into()))
    };
    Ok(Stat {
        user: parse_field(&mut it)?,
        nice: parse_field(&mut it)?,
        system: parse_field(&mut it)?,
        idle: parse_field(&mut it)?,
        iowait: parse_field(&mut it)?,
        irq: parse_field(&mut it)?,
        softirq: parse_field(&mut it)?,
        steal: parse_field(&mut it)?,
    })
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-sensors-cpu 2>&1 | tail -5
```

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-sensors/cpu/src/proc_stat.rs
git commit -m "feat(sensors-cpu): parse_proc_stat() for aggregate cpu line"
```

---

## Task 20: linsight-sensors-cpu — utilization between two stats

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-sensors/cpu/src/proc_stat.rs`

- [ ] **Step 1: Add failing tests**

Append:

```rust
    #[test]
    fn util_returns_percent_busy() {
        let a = Stat { user: 100, idle: 100, ..Stat::default() };
        let b = Stat { user: 200, idle: 100, ..Stat::default() };
        // Δbusy = 100, Δtotal = 100, util = 100%
        assert_eq!(util_between(a, b), 100.0);
    }

    #[test]
    fn util_clamped_when_idle_only() {
        let a = Stat { idle: 100, ..Stat::default() };
        let b = Stat { idle: 200, ..Stat::default() };
        // Δbusy = 0, Δtotal = 100, util = 0%
        assert_eq!(util_between(a, b), 0.0);
    }

    #[test]
    fn util_zero_when_no_time_elapsed() {
        let a = Stat { user: 10, idle: 10, ..Stat::default() };
        assert_eq!(util_between(a, a), 0.0);
    }
```

- [ ] **Step 2: Run, confirm fail**

Expected: `util_between` undefined.

- [ ] **Step 3: Implement**

Add to `proc_stat.rs`:

```rust
/// Compute CPU utilization (0..=100) between two `/proc/stat` samples.
pub fn util_between(a: Stat, b: Stat) -> f64 {
    let dt = b.total().saturating_sub(a.total());
    if dt == 0 { return 0.0; }
    let db = b.busy().saturating_sub(a.busy());
    100.0 * (db as f64) / (dt as f64)
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-sensors-cpu 2>&1 | tail -5
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-sensors/cpu/src/proc_stat.rs
git commit -m "feat(sensors-cpu): util_between() over two samples"
```

---

## Task 21: linsight-sensors-cpu — proc/stat reader that honors PluginCtx.sysroot

**Files:**
- Modify: `/work/repos/visorcraft/linsight/crates/linsight-sensors/cpu/src/proc_stat.rs`

- [ ] **Step 1: Failing test using tempfile**

Append:

```rust
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn read_with_sysroot_uses_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let proc_dir = dir.path().join("proc");
        fs::create_dir(&proc_dir).unwrap();
        fs::write(proc_dir.join("stat"), "cpu 1 2 3 4 5 6 7 8\n").unwrap();

        let stat = read_proc_stat(Some(dir.path())).unwrap();
        assert_eq!(stat.user, 1);
        assert_eq!(stat.system, 3);
    }

    #[test]
    fn read_with_no_sysroot_reads_real_proc() {
        // Smoke test: real /proc/stat must parse on the host.
        // (CI runs on Linux; this is acceptable.)
        let stat = read_proc_stat(None).unwrap();
        assert!(stat.total() > 0);
    }
```

- [ ] **Step 2: Run, confirm fail**

Expected: `read_proc_stat` undefined.

- [ ] **Step 3: Implement**

Add to `proc_stat.rs`:

```rust
use std::path::Path;

pub fn read_proc_stat(sysroot: Option<&Path>) -> Result<Stat, StatError> {
    let path = match sysroot {
        Some(root) => root.join("proc/stat"),
        None => Path::new("/proc/stat").to_path_buf(),
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|e| StatError::BadNumber(format!("io reading {}: {e}", path.display())))?;
    parse_proc_stat(&content)
}
```

(We're slightly abusing `StatError::BadNumber` for IO errors here for
brevity; the daemon-level error type will widen.)

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-sensors-cpu 2>&1 | tail -5
```

Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-sensors/cpu/src/proc_stat.rs
git commit -m "feat(sensors-cpu): read_proc_stat() with sysroot override"
```

---

## Task 22: linsight-sensors-cpu — CpuPlugin (LinsightPlugin impl)

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-sensors/cpu/src/plugin.rs`

The CPU plugin in this MVP exposes one sensor: `cpu.util`. It holds a
mutex-protected previous `Stat` and computes `util_between` on each
sample.

- [ ] **Step 1: Failing test**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Mutex;
use std::path::PathBuf;

use linsight_core::{Category, Reading, SensorId, SensorKind, Unit};
use linsight_plugin_sdk::{LinsightPlugin, PluginCtx, PluginError, PluginManifest, SensorDescriptor};

use crate::proc_stat::{read_proc_stat, util_between, Stat};

// ... impl in Step 3.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_sysroot(stat_content: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("proc")).unwrap();
        fs::write(dir.path().join("proc/stat"), stat_content).unwrap();
        dir
    }

    #[test]
    fn init_returns_one_sensor() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        let ctx = PluginCtx { sysroot: Some(dir.path().to_path_buf()) };
        let manifest = plugin.init(&ctx).unwrap();
        assert_eq!(manifest.sensors.len(), 1);
        assert_eq!(manifest.sensors[0].id.as_str(), "cpu.util");
    }

    #[test]
    fn first_sample_returns_zero() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 100 0 50 1000 0 0 0 0\n");
        let ctx = PluginCtx { sysroot: Some(dir.path().to_path_buf()) };
        plugin.init(&ctx).unwrap();
        let r = plugin.sample(SensorId::new("cpu.util")).unwrap();
        assert!(matches!(r, Reading::Scalar(v) if v == 0.0));
    }

    #[test]
    fn second_sample_reflects_busy_delta() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 100 0 50 1000 0 0 0 0\n");
        let ctx = PluginCtx { sysroot: Some(dir.path().to_path_buf()) };
        plugin.init(&ctx).unwrap();
        // First sample: primes the previous-stat cache.
        plugin.sample(SensorId::new("cpu.util")).unwrap();
        // Mutate the fake /proc/stat: idle didn't move, user went up by 100.
        std::fs::write(dir.path().join("proc/stat"), "cpu 200 0 50 1000 0 0 0 0\n").unwrap();
        let r = plugin.sample(SensorId::new("cpu.util")).unwrap();
        // Δbusy = 100, Δtotal = 100 → 100%
        assert!(matches!(r, Reading::Scalar(v) if v == 100.0));
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let plugin = CpuPlugin::default();
        let dir = fake_sysroot("cpu 1 2 3 4 5 6 7 8\n");
        let ctx = PluginCtx { sysroot: Some(dir.path().to_path_buf()) };
        plugin.init(&ctx).unwrap();
        let err = plugin.sample(SensorId::new("not.cpu")).unwrap_err();
        assert!(matches!(err, PluginError::Unsupported(_)));
    }
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-sensors-cpu 2>&1 | tail -10
```

Expected: `CpuPlugin` undefined.

- [ ] **Step 3: Implement**

Add to `crates/linsight-sensors/cpu/src/plugin.rs` above the test module:

```rust
#[derive(Default)]
pub struct CpuPlugin {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    sysroot: Option<PathBuf>,
    prev_stat: Option<Stat>,
}

impl LinsightPlugin for CpuPlugin {
    fn init(&self, ctx: &PluginCtx) -> Result<PluginManifest, PluginError> {
        let mut inner = self.inner.lock().expect("CpuPlugin inner poisoned");
        inner.sysroot = ctx.sysroot.clone();
        inner.prev_stat = None;
        Ok(PluginManifest {
            plugin_id: "com.visorcraft.linsight.cpu".into(),
            display_name: "CPU".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sensors: vec![SensorDescriptor {
                id: SensorId::new("cpu.util"),
                display_name: "CPU utilization".into(),
                unit: Unit::Percent,
                kind: SensorKind::Scalar,
                category: Category::Cpu,
                native_rate_hz: 1.0,
                min: Some(0.0),
                max: Some(100.0),
                device_id: None,
            }],
        })
    }

    fn sample(&self, sensor: SensorId) -> Result<Reading, PluginError> {
        if sensor.as_str() != "cpu.util" {
            return Err(PluginError::Unsupported(sensor.to_string()));
        }
        let mut inner = self.inner.lock().expect("CpuPlugin inner poisoned");
        let current = read_proc_stat(inner.sysroot.as_deref())
            .map_err(|e| PluginError::Io(e.to_string()))?;
        let util = match inner.prev_stat {
            None => 0.0,
            Some(prev) => util_between(prev, current),
        };
        inner.prev_stat = Some(current);
        Ok(Reading::Scalar(util))
    }
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-sensors-cpu 2>&1 | tail -10
```

Expected: 12 passed across crate.

- [ ] **Step 5: `just ci`**

```bash
just ci
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/linsight-sensors/cpu/src/plugin.rs
git commit -m "feat(sensors-cpu): CpuPlugin emitting cpu.util via /proc/stat deltas"
```

---

## Task 23: linsightd crate skeleton + main

**Files:**
- Create: `/work/repos/visorcraft/linsight/apps/linsightd/Cargo.toml`
- Create: `/work/repos/visorcraft/linsight/apps/linsightd/src/main.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "linsightd"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "LinSight daemon: hosts plugins, serves clients over a Unix socket."

[dependencies]
anyhow = { workspace = true }
clap = { workspace = true }
linsight-core = { path = "../../crates/linsight-core" }
linsight-protocol = { path = "../../crates/linsight-protocol" }
linsight-plugin-sdk = { path = "../../crates/linsight-plugin-sdk" }
linsight-sensors-cpu = { path = "../../crates/linsight-sensors/cpu" }
polling = { workspace = true }
postcard = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
```

- [ ] **Step 2: Skeleton `src/main.rs`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2024_idioms)]

mod plugin_host;
mod runtime;
mod scheduler;
mod transport;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about = "LinSight sensor daemon")]
struct Cli {
    /// Override the Unix socket path. Defaults to $XDG_RUNTIME_DIR/linsight.sock.
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("LINSIGHT_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(default_socket_path);
    runtime::run(socket)
}

fn default_socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    runtime_dir.join("linsight.sock")
}
```

- [ ] **Step 3: Compile-check (will fail — modules empty)**

```bash
cargo check -p linsightd 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add apps/linsightd/
git commit -m "feat(daemon): add linsightd binary skeleton"
```

---

## Task 24: linsightd — runtime module: socket bind + epoll loop entry

**Files:**
- Create: `/work/repos/visorcraft/linsight/apps/linsightd/src/runtime.rs`

The runtime owns the `polling::Poller`, accepts new clients, and
delegates connection handling to `transport::unix`.

- [ ] **Step 1: Failing integration test** (in
  `apps/linsightd/tests/`)

Create `apps/linsightd/tests/handshake.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use linsight_protocol::{ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, ServerMsg};
use std::os::unix::net::UnixStream;
use std::time::Duration;

mod harness;
use harness::DaemonHarness;

#[test]
fn daemon_accepts_hello_replies_welcome() {
    let harness = DaemonHarness::spawn();
    let stream = UnixStream::connect(&harness.socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

    let mut writer = FrameWriter::new(stream.try_clone().unwrap());
    let mut reader = FrameReader::new(stream);

    writer.write_client(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: "test".into(),
    }).unwrap();

    let welcome = reader.read_server().expect("welcome");
    match welcome {
        ServerMsg::Welcome { protocol_version, .. } => {
            assert_eq!(protocol_version, PROTOCOL_VERSION);
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
}
```

Create `apps/linsightd/tests/harness/mod.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub struct DaemonHarness {
    pub socket: PathBuf,
    _child: Child,
    _tmp: tempfile::TempDir,
}

impl DaemonHarness {
    pub fn spawn() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket = tmp.path().join("linsight.sock");
        let bin = env!("CARGO_BIN_EXE_linsightd");
        let child = Command::new(bin)
            .args(["--socket", socket.to_str().unwrap()])
            .env("LINSIGHT_LOG", "warn")
            .spawn()
            .expect("spawn linsightd");
        // Wait up to 2 s for the socket file to appear.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if socket.exists() { break; }
            sleep(Duration::from_millis(20));
        }
        Self { socket, _child: child, _tmp: tmp }
    }
}

impl Drop for DaemonHarness {
    fn drop(&mut self) {
        let _ = self._child.kill();
        let _ = self._child.wait();
    }
}
```

Add `tempfile` to `apps/linsightd/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsightd 2>&1 | tail -10
```

Expected: link errors / compile errors because `runtime::run` doesn't
exist yet.

- [ ] **Step 3: Implement runtime stub**

Create `apps/linsightd/src/runtime.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use anyhow::Context;
use tracing::info;

use crate::transport;
use crate::plugin_host;
use crate::scheduler;

pub fn run(socket: PathBuf) -> anyhow::Result<()> {
    if socket.exists() {
        std::fs::remove_file(&socket)
            .with_context(|| format!("removing stale socket at {}", socket.display()))?;
    }

    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("binding {}", socket.display()))?;
    info!(socket = %socket.display(), "linsightd listening");

    let host = plugin_host::PluginHost::with_builtins();
    let scheduler = scheduler::Scheduler::new(host);

    // Tear down the socket file on Drop.
    let _guard = SocketGuard(socket.clone());

    transport::unix::accept_loop(listener, scheduler)
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
```

- [ ] **Step 4: Run (will still fail — `PluginHost`, `Scheduler`, `transport::unix::accept_loop` undefined)**

```bash
cargo check -p linsightd 2>&1 | tail -10
```

That's expected; next tasks fill them in.

- [ ] **Step 5: Commit**

```bash
git add apps/linsightd/
git commit -m "feat(daemon): runtime::run() + SocketGuard skeleton"
```

---

## Task 25: linsightd — plugin host (built-in CpuPlugin registered)

**Files:**
- Create: `/work/repos/visorcraft/linsight/apps/linsightd/src/plugin_host.rs`

- [ ] **Step 1: Failing tests**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// ... defs in Step 3.

#[cfg(test)]
mod tests {
    use super::*;
    use linsight_core::SensorId;

    #[test]
    fn with_builtins_registers_cpu() {
        let host = PluginHost::with_builtins();
        let ids: Vec<_> = host.sensor_ids().collect();
        assert!(ids.iter().any(|s| s.as_str() == "cpu.util"));
    }

    #[test]
    fn sample_routes_to_owning_plugin() {
        let host = PluginHost::with_builtins();
        let id = SensorId::new("cpu.util");
        let _first = host.sample(&id).unwrap();
        // Second sample exercises the cached previous stat.
        let _second = host.sample(&id).unwrap();
    }

    #[test]
    fn sample_unknown_sensor_errors() {
        let host = PluginHost::with_builtins();
        let err = host.sample(&SensorId::new("nope.nope")).unwrap_err();
        assert!(err.to_string().contains("nope.nope"));
    }
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsightd 2>&1 | tail -10
```

Expected: `PluginHost` undefined.

- [ ] **Step 3: Implement**

```rust
use std::collections::HashMap;
use std::sync::Arc;

use linsight_core::{Reading, Sample, SensorId};
use linsight_plugin_sdk::{LinsightPlugin, PluginCtx, PluginError, SensorDescriptor};
use linsight_sensors_cpu::CpuPlugin;
use tracing::warn;

pub struct PluginHost {
    plugins: Vec<Arc<dyn LinsightPlugin>>,
    /// Maps every sensor id to (plugin index, descriptor).
    registry: HashMap<SensorId, (usize, SensorDescriptor)>,
}

impl PluginHost {
    pub fn with_builtins() -> Self {
        let mut host = Self { plugins: Vec::new(), registry: HashMap::new() };
        host.register(Arc::new(CpuPlugin::default()));
        host
    }

    fn register(&mut self, plugin: Arc<dyn LinsightPlugin>) {
        let ctx = PluginCtx::default();
        let manifest = match plugin.init(&ctx) {
            Ok(m) => m,
            Err(e) => {
                warn!(error = ?e, "plugin init failed; skipping");
                return;
            }
        };
        let idx = self.plugins.len();
        for desc in manifest.sensors {
            if self.registry.contains_key(&desc.id) {
                warn!(sensor = %desc.id, "duplicate sensor id, first registration wins");
                continue;
            }
            self.registry.insert(desc.id.clone(), (idx, desc));
        }
        self.plugins.push(plugin);
    }

    pub fn sensor_ids(&self) -> impl Iterator<Item = &SensorId> {
        self.registry.keys()
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &SensorDescriptor> {
        self.registry.values().map(|(_, d)| d)
    }

    pub fn sample(&self, id: &SensorId) -> Result<Reading, PluginError> {
        let (idx, _) = self.registry.get(id)
            .ok_or_else(|| PluginError::Unsupported(id.to_string()))?;
        self.plugins[*idx].sample(id.clone())
    }

    pub fn sample_to(&self, id: &SensorId, ts_micros: u64) -> Result<Sample, PluginError> {
        let reading = self.sample(id)?;
        Ok(Sample { sensor: id.clone(), ts_micros, reading })
    }
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsightd plugin_host 2>&1 | tail -10
```

Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add apps/linsightd/src/plugin_host.rs
git commit -m "feat(daemon): PluginHost::with_builtins() registers CpuPlugin"
```

---

## Task 26: linsightd — scheduler (subscription bookkeeping + sample dispatch)

**Files:**
- Create: `/work/repos/visorcraft/linsight/apps/linsightd/src/scheduler.rs`

The v1 scheduler maintains a map of `SensorId → (ref_count, next_due_at)`.
It exposes a `tick(now)` method that returns `(SensorId, Sample)` pairs
for every sensor whose `next_due_at <= now`. Subscriptions add/remove
references; reaching zero stops sampling.

Concurrency: clients are served by separate worker threads, but the
scheduler is single-owner (the runtime thread). Threading is handled
above by passing samples through a channel — out of scope here.

- [ ] **Step 1: Failing tests**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// ... defs in Step 3.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_host::PluginHost;
    use linsight_core::SensorId;

    #[test]
    fn subscribe_once_then_tick_yields_sample() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        let samples = sched.tick(1_000);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].sensor, id);
    }

    #[test]
    fn second_tick_within_period_yields_nothing() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        let _ = sched.tick(1_000_000);          // at 1 s
        let samples = sched.tick(1_500_000);     // 500 ms later, native_rate = 1 Hz
        assert!(samples.is_empty());
    }

    #[test]
    fn unsubscribe_stops_sampling() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        sched.unsubscribe(&id);
        let samples = sched.tick(10_000_000);
        assert!(samples.is_empty());
    }

    #[test]
    fn two_subscribers_increment_refcount() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, None).unwrap();
        sched.subscribe(&id, None).unwrap();
        sched.unsubscribe(&id);                  // refcount drops to 1, still sampling
        assert_eq!(sched.tick(10_000_000).len(), 1);
        sched.unsubscribe(&id);                  // refcount = 0
        assert!(sched.tick(20_000_000).is_empty());
    }

    #[test]
    fn requested_rate_caps_at_native() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let id = SensorId::new("cpu.util");
        sched.subscribe(&id, Some(99.0)).unwrap();       // request 99 Hz, native = 1 Hz
        let _ = sched.tick(0);
        let samples_at_500ms = sched.tick(500_000);
        assert!(samples_at_500ms.is_empty(), "should still be once per second");
    }

    #[test]
    fn unknown_sensor_rejects_subscribe() {
        let host = PluginHost::with_builtins();
        let mut sched = Scheduler::new(host);
        let err = sched.subscribe(&SensorId::new("ghost"), None).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsightd scheduler 2>&1 | tail -10
```

Expected: `Scheduler` undefined.

- [ ] **Step 3: Implement**

```rust
use std::collections::HashMap;

use linsight_core::{Sample, SensorId};
use linsight_plugin_sdk::PluginError;
use thiserror::Error;
use tracing::warn;

use crate::plugin_host::PluginHost;

#[derive(Debug, Error)]
pub enum SchedError {
    #[error("unknown sensor: {0}")]
    Unknown(String),
}

struct Entry {
    refcount: u32,
    period_micros: u64,
    next_due_at_micros: u64,
}

pub struct Scheduler {
    host: PluginHost,
    entries: HashMap<SensorId, Entry>,
}

impl Scheduler {
    pub fn new(host: PluginHost) -> Self {
        Self { host, entries: HashMap::new() }
    }

    pub fn subscribe(
        &mut self,
        id: &SensorId,
        requested_rate_hz: Option<f32>,
    ) -> Result<(), SchedError> {
        let descriptor = self
            .host
            .descriptors()
            .find(|d| &d.id == id)
            .ok_or_else(|| SchedError::Unknown(id.to_string()))?
            .clone();

        let native = descriptor.clamped_rate_hz();
        let effective = match requested_rate_hz {
            Some(r) => native.min(r.clamp(0.1, native)),
            None => native,
        };
        let period_micros = (1_000_000.0 / effective as f64) as u64;

        self.entries
            .entry(id.clone())
            .and_modify(|e| e.refcount += 1)
            .or_insert(Entry { refcount: 1, period_micros, next_due_at_micros: 0 });
        Ok(())
    }

    pub fn unsubscribe(&mut self, id: &SensorId) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.refcount = entry.refcount.saturating_sub(1);
            if entry.refcount == 0 {
                self.entries.remove(id);
            }
        }
    }

    pub fn tick(&mut self, now_micros: u64) -> Vec<Sample> {
        let mut out = Vec::new();
        for (id, entry) in self.entries.iter_mut() {
            if now_micros < entry.next_due_at_micros {
                continue;
            }
            match self.host.sample_to(id, now_micros) {
                Ok(sample) => out.push(sample),
                Err(PluginError::Unsupported(_)) => {
                    // The plugin no longer claims this sensor — degrade gracefully.
                    warn!(sensor = %id, "plugin no longer supports sensor; will retry");
                }
                Err(e) => warn!(sensor = %id, error = ?e, "sample failed"),
            }
            entry.next_due_at_micros = now_micros + entry.period_micros;
        }
        out
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &linsight_plugin_sdk::SensorDescriptor> {
        self.host.descriptors()
    }
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsightd scheduler 2>&1 | tail -10
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add apps/linsightd/src/scheduler.rs
git commit -m "feat(daemon): subscription-driven Scheduler with period-clamped rates"
```

---

## Task 27: linsightd — transport module skeleton

**Files:**
- Create: `/work/repos/visorcraft/linsight/apps/linsightd/src/transport/mod.rs`

- [ ] **Step 1: Add `transport::mod`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

pub mod unix;
```

- [ ] **Step 2: Commit (no test yet; transport::unix is next)**

```bash
git add apps/linsightd/src/transport/mod.rs
git commit -m "chore(daemon): add transport::mod"
```

---

## Task 28: linsightd — transport::unix accept loop + per-client thread

**Files:**
- Create: `/work/repos/visorcraft/linsight/apps/linsightd/src/transport/unix.rs`

Design choice for the v1 MVP: one OS thread per client connection.
Subscription bookkeeping is shared via `Arc<Mutex<Scheduler>>`. This
keeps the runtime model trivial. The `polling` crate becomes
load-bearing in Plan 7 when we need to sleep until the next sample
deadline across hundreds of clients; for the MVP the thread-per-client
budget is fine (you'll have at most ~3 clients in normal use).

- [ ] **Step 1: Make `Scheduler` `Send` and shareable; add `tick_sensor`
helper**

Add to `apps/linsightd/src/scheduler.rs`:

```rust
impl Scheduler {
    /// Tick exactly one sensor regardless of period — used by client
    /// threads to take a sample right after subscribe.
    pub fn sample_now(&self, id: &SensorId, now_micros: u64) -> Option<Sample> {
        self.host.sample_to(id, now_micros).ok()
    }
}
```

- [ ] **Step 2: Write the transport**

Create `apps/linsightd/src/transport/unix.rs`:

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use linsight_protocol::{
    verify_hello, ClientMsg, FrameError, FrameReader, FrameWriter, PluginInfo, ServerMsg,
    PROTOCOL_VERSION,
};
use tracing::{info, warn};

use crate::scheduler::Scheduler;

pub fn accept_loop(listener: UnixListener, scheduler: Scheduler) -> anyhow::Result<()> {
    let shared = Arc::new(Mutex::new(scheduler));
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let sched = Arc::clone(&shared);
                thread::spawn(move || {
                    if let Err(e) = serve(s, sched) {
                        warn!(error = ?e, "client session ended with error");
                    }
                });
            }
            Err(e) => warn!(error = ?e, "accept failed"),
        }
    }
    Ok(())
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

fn serve(stream: UnixStream, sched: Arc<Mutex<Scheduler>>) -> Result<(), FrameError> {
    let peer = stream.peer_addr().ok();
    info!(?peer, "client connected");
    stream.set_read_timeout(None).ok();
    let read_clone = stream.try_clone().map_err(FrameError::Io)?;
    let mut reader = FrameReader::new(read_clone);
    let writer = Arc::new(Mutex::new(FrameWriter::new(stream)));

    // 1) Handshake.
    let first = reader.read_client()?;
    let client_name = match verify_hello(&first) {
        Ok(name) => name.to_string(),
        Err(e) => {
            let _ = writer.lock().unwrap().write_server(&ServerMsg::Bye {
                reason: format!("handshake failed: {e}"),
            });
            return Ok(());
        }
    };
    info!(client = %client_name, "client said hello");
    let plugins = vec![PluginInfo {
        plugin_id: "com.visorcraft.linsight.cpu".into(),
        display_name: "CPU".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        sensor_count: 1,
    }];
    writer.lock().unwrap().write_server(&ServerMsg::Welcome {
        protocol_version: PROTOCOL_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").into(),
        plugins,
    })?;

    // 2) Spawn a sample-pumping thread tied to this client's subscriptions.
    let pump_writer = Arc::clone(&writer);
    let pump_sched = Arc::clone(&sched);
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let pump = thread::spawn(move || {
        let tick = Duration::from_millis(50);
        let start = Instant::now();
        loop {
            if stop_rx.recv_timeout(tick).is_ok() { break; }
            let now = now_micros();
            let samples = {
                let mut s = pump_sched.lock().unwrap();
                s.tick(now)
            };
            for sample in samples {
                let mut w = pump_writer.lock().unwrap();
                if w.write_server(&ServerMsg::Sample(sample)).is_err() {
                    return;
                }
            }
            let _ = start;
        }
    });

    // 3) Read loop.
    let result = (|| -> Result<(), FrameError> {
        loop {
            let msg = reader.read_client()?;
            match msg {
                ClientMsg::ListSensors => {
                    let s = sched.lock().unwrap();
                    let infos: Vec<_> = s.descriptors().map(|d| linsight_protocol::SensorInfo {
                        id: d.id.clone(),
                        display_name: d.display_name.clone(),
                        unit: d.unit.clone(),
                        kind: d.kind,
                        category: d.category,
                        native_rate_hz: d.native_rate_hz,
                        min: d.min,
                        max: d.max,
                        device_id: d.device_id.clone(),
                        plugin_id: "com.visorcraft.linsight.cpu".into(),
                    }).collect();
                    drop(s);
                    writer.lock().unwrap().write_server(&ServerMsg::SensorList(infos))?;
                }
                ClientMsg::Subscribe { sensors, rate_hz } => {
                    let mut s = sched.lock().unwrap();
                    for id in &sensors {
                        if let Err(e) = s.subscribe(id, rate_hz) {
                            warn!(error = ?e, "subscribe rejected");
                        }
                    }
                }
                ClientMsg::Unsubscribe { sensors } => {
                    let mut s = sched.lock().unwrap();
                    for id in &sensors {
                        s.unsubscribe(id);
                    }
                }
                ClientMsg::Hello { .. } => {
                    let _ = writer.lock().unwrap().write_server(&ServerMsg::Bye {
                        reason: "duplicate Hello".into(),
                    });
                    return Ok(());
                }
                ClientMsg::Goodbye => return Ok(()),
            }
        }
    })();

    let _ = stop_tx.send(());
    let _ = pump.join();
    result
}
```

- [ ] **Step 3: Run the harness test**

```bash
cargo test -p linsightd --test handshake 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 4: Commit**

```bash
git add apps/linsightd/src/transport/unix.rs apps/linsightd/src/scheduler.rs
git commit -m "feat(daemon): transport::unix accept loop + per-client handshake + sample pump"
```

---

## Task 29: linsightd — end-to-end subscribe & sample integration test

**Files:**
- Modify: `/work/repos/visorcraft/linsight/apps/linsightd/tests/handshake.rs`

- [ ] **Step 1: Add the failing test**

Append:

```rust
#[test]
fn subscribe_receives_at_least_one_sample() {
    let harness = DaemonHarness::spawn();
    let stream = UnixStream::connect(&harness.socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

    let mut writer = FrameWriter::new(stream.try_clone().unwrap());
    let mut reader = FrameReader::new(stream);

    writer.write_client(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: "test".into(),
    }).unwrap();
    let _ = reader.read_server().unwrap();   // Welcome

    writer.write_client(&ClientMsg::Subscribe {
        sensors: vec![linsight_core::SensorId::new("cpu.util")],
        rate_hz: None,
    }).unwrap();

    // Drain frames for up to 3 seconds, expecting at least one Sample.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut got_sample = false;
    while std::time::Instant::now() < deadline {
        match reader.read_server() {
            Ok(ServerMsg::Sample(s)) if s.sensor.as_str() == "cpu.util" => {
                got_sample = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(got_sample, "expected a cpu.util sample within 3 seconds");

    writer.write_client(&ClientMsg::Goodbye).unwrap();
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p linsightd --test handshake 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 3: `just ci`**

```bash
just ci
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add apps/linsightd/tests/handshake.rs
git commit -m "test(daemon): end-to-end subscribe yields cpu.util sample"
```

---

## Task 30: linsight-cli crate skeleton + clap entry

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-cli/Cargo.toml`
- Create: `/work/repos/visorcraft/linsight/crates/linsight-cli/src/main.rs`

- [ ] **Step 1: Write `Cargo.toml`**

```toml
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
[package]
name = "linsight-cli"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
description = "LinSight CLI (talks to linsightd)."

[[bin]]
name = "linsight-cli"
path = "src/main.rs"

[dependencies]
anyhow = { workspace = true }
clap = { workspace = true }
linsight-core = { path = "../linsight-core" }
linsight-protocol = { path = "../linsight-protocol" }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 2: Write `src/main.rs`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2024_idioms)]

mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about = "LinSight command-line client")]
struct Cli {
    /// Override the Unix socket path. Defaults to $XDG_RUNTIME_DIR/linsight.sock.
    #[arg(long, global = true)]
    socket: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print every sensor the daemon advertises.
    List,
    /// Subscribe to a single sensor and print samples until Ctrl+C.
    Read {
        sensor: String,
        /// Stop after N samples.
        #[arg(long)]
        count: Option<u64>,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("LINSIGHT_LOG")
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")))
        .init();
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(default_socket_path);
    match cli.command {
        Cmd::List => commands::list::run(&socket),
        Cmd::Read { sensor, count } => commands::read::run(&socket, &sensor, count),
    }
}

fn default_socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    runtime_dir.join("linsight.sock")
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/linsight-cli/
git commit -m "feat(cli): scaffold linsight-cli with clap subcommands"
```

---

## Task 31: linsight-cli — commands::mod + shared client helpers

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-cli/src/commands/mod.rs`

- [ ] **Step 1: Implement shared helpers**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

pub mod list;
pub mod read;

use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result};
use linsight_protocol::{ClientMsg, FrameReader, FrameWriter, PROTOCOL_VERSION, ServerMsg};

pub(crate) struct Session {
    pub reader: FrameReader<UnixStream>,
    pub writer: FrameWriter<UnixStream>,
}

pub(crate) fn connect_and_hello(socket: &Path) -> Result<Session> {
    let stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let read_stream = stream.try_clone().context("cloning stream")?;
    let mut reader = FrameReader::new(read_stream);
    let mut writer = FrameWriter::new(stream);
    writer.write_client(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_name: "linsight-cli".into(),
    }).context("writing hello")?;
    match reader.read_server().context("reading welcome")? {
        ServerMsg::Welcome { protocol_version, .. } if protocol_version == PROTOCOL_VERSION => {}
        ServerMsg::Welcome { protocol_version, .. } => {
            anyhow::bail!("protocol mismatch: daemon={protocol_version} cli={PROTOCOL_VERSION}");
        }
        ServerMsg::Bye { reason } => anyhow::bail!("daemon refused: {reason}"),
        other => anyhow::bail!("unexpected first message from daemon: {other:?}"),
    }
    Ok(Session { reader, writer })
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/linsight-cli/src/commands/mod.rs
git commit -m "feat(cli): connect_and_hello() shared session bootstrap"
```

---

## Task 32: linsight-cli — `list` subcommand

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-cli/src/commands/list.rs`
- Create: `/work/repos/visorcraft/linsight/crates/linsight-cli/tests/list.rs`

- [ ] **Step 1: Failing integration test**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use assert_cmd::Command;
use predicates::str::contains;
use std::process::{Child, Command as StdCommand};
use std::time::{Duration, Instant};

fn spawn_daemon(socket: &std::path::Path) -> Child {
    StdCommand::new(env!("CARGO_BIN_EXE_linsightd"))
        .args(["--socket", socket.to_str().unwrap()])
        .env("LINSIGHT_LOG", "warn")
        .spawn()
        .expect("spawn daemon")
}

fn wait_for_socket(p: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if p.exists() { return; }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("daemon did not bind socket at {}", p.display());
}

#[test]
fn list_prints_cpu_sensor() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    Command::cargo_bin("linsight-cli").unwrap()
        .args(["--socket", socket.to_str().unwrap(), "list"])
        .assert()
        .success()
        .stdout(contains("cpu.util"));

    let _ = daemon.kill();
    let _ = daemon.wait();
}
```

Note: `linsight-cli`'s test crate must take a dev-dependency on
`linsightd` so `CARGO_BIN_EXE_linsightd` is set:

```toml
# crates/linsight-cli/Cargo.toml — extend [dev-dependencies]:
linsightd = { path = "../../apps/linsightd" }
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-cli 2>&1 | tail -10
```

Expected: `commands::list::run` undefined.

- [ ] **Step 3: Implement `commands::list::run`**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use linsight_protocol::{ClientMsg, ServerMsg};

use crate::commands::connect_and_hello;

pub fn run(socket: &Path) -> Result<()> {
    let mut session = connect_and_hello(socket)?;
    session.writer.write_client(&ClientMsg::ListSensors)?;
    match session.reader.read_server()? {
        ServerMsg::SensorList(infos) => {
            for s in infos {
                println!("{:<24}  {:<24}  {} {}", s.id, s.display_name, s.unit.symbol(), match s.kind {
                    linsight_core::SensorKind::Scalar => "scalar",
                    linsight_core::SensorKind::Counter => "counter",
                    linsight_core::SensorKind::Table => "table",
                    linsight_core::SensorKind::State => "state",
                });
            }
        }
        other => anyhow::bail!("expected SensorList, got {other:?}"),
    }
    Ok(())
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-cli --test list 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/linsight-cli/src/commands/list.rs crates/linsight-cli/tests/list.rs crates/linsight-cli/Cargo.toml
git commit -m "feat(cli): list subcommand"
```

---

## Task 33: linsight-cli — `read` subcommand

**Files:**
- Create: `/work/repos/visorcraft/linsight/crates/linsight-cli/src/commands/read.rs`
- Create: `/work/repos/visorcraft/linsight/crates/linsight-cli/tests/read.rs`

- [ ] **Step 1: Failing integration test**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use assert_cmd::Command;
use predicates::str::is_match;
use std::process::{Child, Command as StdCommand};
use std::time::{Duration, Instant};

fn spawn_daemon(socket: &std::path::Path) -> Child {
    StdCommand::new(env!("CARGO_BIN_EXE_linsightd"))
        .args(["--socket", socket.to_str().unwrap()])
        .env("LINSIGHT_LOG", "warn")
        .spawn()
        .expect("spawn daemon")
}

fn wait_for_socket(p: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if p.exists() { return; }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no socket at {}", p.display());
}

#[test]
fn read_streams_two_samples_then_exits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let socket = tmp.path().join("linsight.sock");
    let mut daemon = spawn_daemon(&socket);
    wait_for_socket(&socket);

    Command::cargo_bin("linsight-cli").unwrap()
        .args(["--socket", socket.to_str().unwrap(), "read", "cpu.util", "--count", "2"])
        .timeout(Duration::from_secs(5))
        .assert()
        .success()
        // Each line should be: cpu.util\t<float>%
        .stdout(is_match(r"(?m)^cpu\.util\s+\d+(\.\d+)?%$").unwrap());

    let _ = daemon.kill();
    let _ = daemon.wait();
}
```

- [ ] **Step 2: Run, confirm fail**

```bash
cargo test -p linsight-cli --test read 2>&1 | tail -10
```

Expected: undefined `commands::read::run`.

- [ ] **Step 3: Implement**

```rust
// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use linsight_core::{Reading, SensorId};
use linsight_protocol::{ClientMsg, ServerMsg};

use crate::commands::connect_and_hello;

pub fn run(socket: &Path, sensor: &str, count: Option<u64>) -> Result<()> {
    let sensor_id = SensorId::try_new(sensor)?;
    let mut session = connect_and_hello(socket)?;
    session.writer.write_client(&ClientMsg::Subscribe {
        sensors: vec![sensor_id.clone()],
        rate_hz: None,
    })?;
    let mut printed = 0u64;
    loop {
        match session.reader.read_server()? {
            ServerMsg::Sample(s) if s.sensor == sensor_id => {
                let line = format_sample(&s.reading);
                println!("{}\t{}", s.sensor, line);
                printed += 1;
                if let Some(max) = count {
                    if printed >= max {
                        break;
                    }
                }
            }
            ServerMsg::SensorDegraded { sensor: id, reason } if id == sensor_id => {
                anyhow::bail!("{sensor_id} degraded: {reason}");
            }
            _ => continue,
        }
    }
    session.writer.write_client(&ClientMsg::Unsubscribe { sensors: vec![sensor_id] }).ok();
    session.writer.write_client(&ClientMsg::Goodbye).ok();
    Ok(())
}

fn format_sample(r: &Reading) -> String {
    match r {
        Reading::Scalar(v) => format!("{v:.1}%"),
        Reading::Counter(v) => format!("{v}"),
        Reading::State(s) => s.clone(),
        Reading::Table(rows) => format!("<{} rows>", rows.len()),
    }
}
```

- [ ] **Step 4: Run**

```bash
cargo test -p linsight-cli --test read 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 5: `just ci`**

```bash
just ci
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/linsight-cli/src/commands/read.rs crates/linsight-cli/tests/read.rs
git commit -m "feat(cli): read subcommand streams samples for one sensor"
```

---

## Task 34: End-to-end smoke

**Files:** none (verification only)

- [ ] **Step 1: Start the daemon in one terminal**

```bash
cd /work/repos/visorcraft/linsight
just run-daemon
```

Expect: `linsightd listening` on `$XDG_RUNTIME_DIR/linsight.sock`.

- [ ] **Step 2: From a second terminal, list sensors**

```bash
just run-cli list
```

Expect output:

```
cpu.util                  CPU utilization           % scalar
```

- [ ] **Step 3: Stream three samples**

```bash
just run-cli read cpu.util --count 3
```

Expect three lines like:

```
cpu.util	2.4%
cpu.util	1.7%
cpu.util	3.0%
```

- [ ] **Step 4: Verify daemon shuts down on Ctrl+C**

In the daemon terminal, press Ctrl+C; the socket file is removed.
Re-check:

```bash
ls -la $XDG_RUNTIME_DIR/linsight.sock
```

Expect: file not found.

---

## Task 35: Final preflight + README badge sweep

**Files:**
- Modify: `/work/repos/visorcraft/linsight/README.md`

- [ ] **Step 1: Run the full preflight**

```bash
cargo install cargo-deny cargo-audit   # if not already installed
just preflight
```

Expected: all three (fmt, clippy, tests, deny, audit) green.

- [ ] **Step 2: Update README with status + first-run instructions**

Replace the body of `README.md` after the heading with:

```markdown
A fast, beautiful, modular Linux system-monitoring dashboard with
multi-GPU support and a runtime plugin system.

**Status:** Phase 1 (Foundation + CLI MVP) — `linsight-cli` streams
live CPU utilization from `linsightd` over a Unix socket. The Qt /
Kirigami GUI lands in Phase 2.

See [`docs/superpowers/specs/2026-05-25-linsight-design.md`](docs/superpowers/specs/2026-05-25-linsight-design.md)
for the full v1 design and [`docs/superpowers/plans/2026-05-25-phases-roadmap.md`](docs/superpowers/plans/2026-05-25-phases-roadmap.md)
for the 10-phase rollout.

## Try it (Phase 1 MVP)

```bash
just run-daemon     # terminal 1
just run-cli list   # terminal 2 — see "cpu.util"
just run-cli read cpu.util --count 5
```

## Build

```bash
just ci              # fmt-check + clippy + tests
just build-release   # release binary
just build-release-v3   # x86_64-v3 tuned (CachyOS / modern systems)
```

## License

GPL-3.0-only. See `LICENSE`.
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: README first-run for Phase 1 MVP"
```

- [ ] **Step 4: Tag**

```bash
git tag -a v0.1.0 -m "Phase 1: Foundation + CLI MVP"
```

---

## Self-review

### Spec coverage

| Spec section | Covered by |
|---|---|
| Workspace layout | Task 1 (Cargo.toml), Tasks 3-33 add crates following the spec layout |
| Demand-driven daemon | Task 24 (`runtime::run`), Task 28 (`transport::unix`) |
| Subscription-driven sampling | Task 26 (`Scheduler`) — `tick()` only emits subscribed sensors |
| Plugin SDK trait | Tasks 14-17 |
| Built-in sensors as in-tree plugins | Task 22 (`CpuPlugin` uses the same SDK) |
| Wire protocol (postcard, length-prefixed) | Tasks 9-13 |
| Version handshake | Task 12 |
| `linsight-cli` first client | Tasks 30-33 |
| Justfile + CI parity + perf flags | Task 2 + Task 1 (release profile) |
| x86_64-v3 build variant | Task 2 (`just build-release-v3`) |

Items deliberately deferred to later phases per the roadmap:

- Qt / Kirigami GUI (Plan 2)
- NVML, xe, NVMe, network sensors (Plans 3-4)
- Runtime `.so` plugin loading (Plan 5)
- Custom-canvas dashboard (Plan 6)
- Always-on mode (Plan 7)
- Multi-window + remote (Plan 8)
- Theming / i18n / a11y (Plan 9)
- Packaging beyond `just arch` placeholders (Plan 10)

### Placeholder scan

No "TBD" / "TODO" / "implement later" markers found.

### Type consistency

- `SensorId`, `Reading`, `Sample`, `Unit`, `SensorKind`, `Category` are
  defined once in `linsight-core` and re-used everywhere else.
- `PluginManifest`, `SensorDescriptor`, `PluginError`, `PluginCtx`,
  `LinsightPlugin` are defined once in `linsight-plugin-sdk`.
- Wire messages defined once in `linsight-protocol::messages`; framing
  in `linsight-protocol::frame`.
- Scheduler's `Entry` and `SchedError` are private to `scheduler.rs`.
- `PluginHost::sample_to` (Task 25) and `Scheduler::sample_now`
  (Task 28) are consistent: both return `Sample` (or option of) at a
  caller-supplied timestamp.
