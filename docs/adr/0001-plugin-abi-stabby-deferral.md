<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# ADR-0001 — Plugin ABI: stabby migration

**Status:** superseded — implementation landed 2026-05-25 as
`LINSIGHT_PLUGIN_ABI_VERSION = 2`, then bumped to v3 the same day
after a release-mode correctness bug in stabby's tagged-enum
matcher was discovered. **Current shipped revision is v3.** The
migration uses **R-mirror types** in `linsight-plugin-sdk::mirror`
(`RUnit`, `RSensorKind`, `RCategory`, `RReading`, `RTableRow`,
`RCell`) with `From`/`Into` adapters back to the host's std-typed
`linsight-core` values. The `LinsightPlugin` trait is
`#[stabby::stabby]`; every method is `extern "C" fn`; the export
macro emits a `#[stabby::export]`-annotated factory returning
`stabby::dynptr!(stabby::boxed::Box<dyn LinsightPlugin>)`. The
host loads via `StabbyLibrary::get_stabbied`. The original deferral
context, the v2 implementation notes, and the v3 amendment that
explains why we restructured the payload-bearing mirrors are
preserved below for the record.

---

## Original context (now historical)

**Original status:** accepted, 2026-05-25
**Context:** v0.2 shipped a runtime `.so` plugin loader. The export
macro returned `*mut dyn LinsightPlugin` — a Rust fat pointer that's
strictly speaking not FFI-safe across compiler versions.

### Decision

Keep the raw-fat-pointer factory for v0.2. Defer the
`stabby::DynPtr` migration to v0.3 (or whenever there's a real
cross-compiler plugin ecosystem to worry about).

### Why ship the raw form first

* It worked end-to-end. `linsight-cli plugin new` → `cargo build` →
  `plugin install` → daemon load → live sample was verified on the
  dev machine.
* The realistic v0.2 plugin author rebuilt their plugin whenever
  the LinSight workspace bumped (the SDK wasn't on crates.io yet).
  Same rustc on both sides ⇒ identical trait layout ⇒ no ABI drift
  in practice.
* `LINSIGHT_PLUGIN_ABI_VERSION` gave us an explicit kill-switch:
  the daemon refused any plugin whose reported version didn't match.

### Why we'd do stabby eventually

* Once the SDK is on crates.io, plugins start arriving compiled
  with different rustc minor versions. `*mut dyn Trait`'s vtable
  layout is stable within a rustc release but not across releases.
* `stabby` provides an FFI-safe vtable + a typed factory return.
* The cost was a coordinated edit across the SDK, every in-tree
  sensor, the `linsight-cli plugin new` scaffold, and the daemon's
  loader.

## What we learned implementing it

The original recipe ("swap `Vec`/`String`/`Option` for
`RVec`/`RString`/`ROption` in `PluginManifest`/`SensorDescriptor`/
`PluginError`, slap `#[stabby::stabby]` on the trait") did not
compile against stabby 36.2.2:

* `#[stabby::stabby]` requires every field of a stabbified struct
  to implement `IStable`. `SensorDescriptor` referenced
  `linsight-core` types (`SensorId(String)`, `Unit::Custom(String)`,
  `SensorKind`, `Category`); none are `IStable`. Same trap for
  `Reading::Table(Vec<TableRow>)` and `Reading::State(String)`.
* Stabbifying `linsight-core` directly would propagate through
  `linsight-protocol::SensorInfo` and into the GUI — too wide.
* Stabby 36.x path/type names also differ from the original ADR
  pseudocode: it's `stabby::string::String` / `stabby::vec::Vec` /
  `stabby::option::Option` / `stabby::result::Result`, not
  `stabby::abi::R*`. The dyn-trait factory uses
  `stabby::dynptr!(SBox<dyn Trait>)`, not a top-level `DynPtr`.
  Stabby's `Box`/`Arc` (`stabby::boxed::Box`, `stabby::sync::Arc`)
  replace std equivalents; `library.get(b"…\0")` becomes
  `StabbyLibrary::get_stabbied`.
* `extern "C" fn` is mandatory on every stabbified trait method
  (stabby requires a stable calling convention).
* Stabby's proc-macro chain is heavy — the trait + manifest
  expansion took ~11 s alone on the dev machine.

## What we shipped

R-mirror types in `linsight-plugin-sdk::mirror` cross the FFI
vtable; std-typed `linsight-core` values live on the host side; the
SDK provides `host_init` / `host_sample` convenience wrappers so
the daemon doesn't repeat the conversion at every call site. A tiny
`DynBoxPlugin` adapter in `apps/linsightd/src/plugin_host.rs` wraps
the stabby dynptr so the rest of the host keeps treating builtins
and dynamic plugins through one `Arc<dyn LinsightPlugin>` shape.
Plugins now need a direct `stabby = "36"` dep because stabby's
proc-macros resolve `stabby` from the call-site crate's `Cargo.toml`
via `proc_macro_crate`; the `linsight-cli plugin new` template adds
it.

## Mitigation in the meantime (now obsolete)

The old export macro's `#[allow(improper_ctypes_definitions)]` is
gone — the ABI v2 factory returns a fully stabby-typed dynptr and
is `improper_ctypes`-clean.

## Consequences

Accepted as part of shipping v2:

* **Plugin authors must take a direct `stabby` dep.** The
  `linsight-cli plugin new` template adds it; documented in the
  scaffold README. Removing it is not possible — `#[stabby::export]`
  resolves `stabby` from the call-site crate.
* **R-mirror types are duplicated.** Every `linsight-core` value
  type with a non-`IStable` field (`SensorId(String)`,
  `Reading::State(String)`, `Reading::Table(Vec<…>)`,
  `Unit::Custom(String)`, etc.) has a parallel `R*` form in
  `linsight-plugin-sdk::mirror` with `From` conversions in both
  directions. Adding a new variant to a core type means editing two
  enums plus two `From` impls. The host wrapper functions
  (`host_init` / `host_sample`) keep this cost out of the daemon
  call sites.
* **Plugins compile against the workspace SDK by path until it
  publishes.** The kill-switch (`LINSIGHT_PLUGIN_ABI_VERSION` plus
  stabby's reflection check in `get_stabbied`) catches the rebuild
  miss at load time rather than at handshake time.
* **The `to_string_lossy` and debug-only `SensorId::new` hazards at
  the FFI seam were identified post-merge** in the 2026-05-25 audit
  and corrected: `PluginCtx::new_with_sysroot` now rejects non-UTF-8
  paths up front, and `host_init` validates every plugin-returned
  sensor ID with `SensorId::try_new`. See
  `docs/superpowers/plans/2026-05-25-code-review-punch-list.md`
  CR-3 and CR-4.

## What we learned at v3 — the stabby release-mode matcher bug

The v2 release binary shipped a quietly broken plugin ABI. A
manual launch of `target/release/linsight` rendered `cpu.util`
with unit `°C` instead of `%`, and NVIDIA GPU utilization with
`°C` instead of `%`. Tracing it found a stabby code-gen bug, not
an issue in our conversion code.

### The bug

`#[stabby::stabby] #[repr(stabby)]` on an enum produces a
discriminated union encoded as a nested
`Result<v0, Result<v1, Result<v2, ...>>>` tree. The macro emits
`match_owned` / `match_ref` / `match_mut` dispatchers that walk
this tree recursively, invoking the user-supplied closure for
whichever leaf matches. The user closures are passed in source
declaration order:

```rust
r.match_owned(
    || Unit::Percent,        // closure for variant 0
    || Unit::Celsius,        // closure for variant 1
    ...
    |s| Unit::Custom(s.as_str().to_owned()),  // closure for the payload variant
)
```

This works correctly in debug builds. At `opt-level >= 1` (i.e.
every release build) the recursive `match_owned` body misroutes
closures by one variant: `Percent` round-trips to `Celsius`,
`Scalar(42.5)` to `Counter(<f64 bit pattern>)`, etc. The bug is
reproducible:

* `cargo test -p linsight-plugin-sdk unit_round_trips` → pass.
* `cargo test --release -p linsight-plugin-sdk unit_round_trips`
  → `left: Percent, right: Celsius`.
* Reproduces at `opt-level = 1`; passes at `opt-level = 0`.
* Reproduces with `lto = false, codegen-units = 16` — not an
  LTO interaction, it's the optimizer.

`#[repr(u8)]` unit-only enums (`RSensorKind`, `RCategory`,
`RPluginErrorKind`) are unaffected because their `From`/`Into`
impls use plain Rust `match` on the discriminant, not stabby's
recursive matcher. `stabby::Option` is also unaffected — the
tests in `tests/dynamic_load.rs` and `mirror.rs` continue to
exercise `SOption<SString>` round-trips and pass in release.

### Why the audit missed it

`cargo test` defaults to debug. The `mTLS smoke`, dynamic-load,
and sensor sample-path tests added in the 2026-05-25 hardening
sprint all ran debug. The bug only surfaces in release binaries
— exactly the binaries `just build-release`, `just arch-pkg`,
and the Flatpak builder produce.

### The v3 fix

Every former `#[repr(stabby)]` enum was restructured into a
`(kind: <Repr>Kind, payload_fields)` struct. The discriminant
moves into an explicit `#[repr(u8)]` unit-only enum (which is
unaffected); the variant payloads become plain struct fields
that the active variant reads. `From`/`Into` impls dispatch via
trivial Rust `match` on `kind` — no stabby-generated matcher
involved. The wire shape changed:

| Type | v2 encoding | v3 encoding |
|---|---|---|
| `RUnit` | `enum { Percent, …, Custom(SString) }` | `struct { kind: RUnitKind, custom: SOption<SString> }` |
| `RReading` | `enum { Scalar(f64), Counter(u64), Table(SVec<…>), State(SString) }` | `struct { kind: RReadingKind, scalar: f64, counter: u64, state: SOption<SString>, table: SVec<…> }` |
| `RCell` | `enum { Text(SString), Number(f64), Bytes(u64) }` | `struct { kind: RCellKind, text: SOption<SString>, number: f64, bytes: u64 }` |

The struct encoding is wider on the wire (inactive payload
fields carry defaults) but trades a few bytes per descriptor
for correctness. The cost is invisible in practice: a
`SensorDescriptor` is sent once per `init()` and a `Reading` is
~32 bytes either way after `state` and `table` defaults.

### Wire compatibility

v2→v3 is a hard ABI break. The export symbol was renamed
`linsight_plugin_v2 → linsight_plugin_v3` so a v2 `.so` fails
the symbol lookup at load time (a clean error in the daemon
log) rather than silently exchanging incompatible mirror shapes
with the host. `LINSIGHT_PLUGIN_ABI_VERSION` bumped to `3`; the
fast-path version check rejects any v2 plugin before stabby's
reflection ever runs.

### Upstream reporting

The bug should be filed against stabby; we have a minimal repro
(`crates/linsight-plugin-sdk/src/mirror.rs::tests::unit_round_trips`
at v2-equivalent shape, run `cargo test --release`). Tracked in
the open follow-ups as "file stabby `match_owned` opt-level bug
upstream."

### Consequences of the v3 shape

* **Payload-bearing R-mirror types are wider.** Inactive payload
  fields are always serialized as zero / `None` / empty. For
  `RReading` that's an extra `u64 + f64 + SOption<SString> +
  SVec<…>` per sample. Acceptable at our sample rates; bench
  before the inevitable v4 if rates climb.
* **Adding a new `Reading` variant requires adding the
  corresponding payload field to `RReading`.** Document the
  fields-must-correlate-with-kinds contract in the mirror
  module's prose so a new variant isn't accidentally encoded
  without its companion payload field.
* **Sensor-author ergonomics unchanged.** The plugin-facing API
  (`Unit::Percent.into()`, `Reading::Scalar(v).into()`) is
  identical. Only mirror-internal conversion code changed.
