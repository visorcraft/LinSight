<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Commit Review Punch List — 2026-05-25

Second peer review covering the **last 10 pre-audit commits** to the
LinSight repository (everything from `1f4e4a8` back through
`4e403b1`). Five independent reviewers covered the commits in
parallel; this is the consolidated findings list. Per-area raw
reports archived under `/tmp/linsight-review-commits/0{1..5}-*.md`
at audit time.

> **Status: closed.** Every Critical and High item below was
> resolved in the same hardening sprint as the file-by-file punch
> list. The cosmetic GUI nits and ABI-v3-only items remain as
> followups. See [`CHANGELOG.md`](../../../CHANGELOG.md) for the
> user-facing summary.

## Scope

| # | sha | subject | size |
|---|---|---|---|
| 1 | `1f4e4a8` | `fix(gui): lift OverviewModel to app scope; render Credits + GPL` | 7f, 204+/95- |
| 2 | `b204460` | `revert: cxx-qt binding "bug" was a Wayland screenshot artifact` | 5f, 47+/243- |
| 3 | `1a9bde3` | `docs: refresh to reflect v0.3.x feature set` | 8f, 398+/212- |
| 4 | `476e3c4` | `feat(gui): in-app screenshot + cxx-qt binding-bug writeup` | 6f, 328+/30- |
| 5 | `8c301d5` | `refactor(plugin-sdk): migrate plugin ABI to stabby v2 (R-mirror types)` | 17f, 1122+/136- |
| 6 | `021e1d2` | `feat: brand icons, mTLS tunnel, canvas editor, motion-reduction` | 18f, 1465+/18- |
| 7 | `c28089a` | `feat(gui): left sidebar with Settings, About, Licenses, Credits pages` | 19f, 7380+/141- |
| 8 | `5d5cc0a` | `chore: bump to v0.3.0 + completion notes for follow-up list` | 14f, 106+/40- |
| 9 | `461b162` | `docs(adr): ADR-0001 plugin ABI stabby migration deferred to v0.3` | 1f, 71+ |
| 10 | `4e403b1` | `test(gui): headless boot smoke via xvfb-run` | 2f, 44+ |

## Headline finding

The reviewers converged on a single pattern across all 10 commits:

> Author commits when code compiles and approximately renders. Bugs
> are then patched silently in follow-up commits whose titles hide
> the corrections.

Four independent reviewers used phrasing like "shipped broken,"
"fabricated test claim," or "would have rejected this PR." The
codebase is structurally competent — plugin loading, stabby ABI
plumbing, scheduler, history batching, alert debounce, and the
cxx-qt bridge are all correctly built — but the commit hygiene
itself was poor.

## Systemic patterns

| Pattern | Where it surfaced | Resolution |
|---|---|---|
| Mega-commits with `+` in the subject | `021e1d2` (4 unrelated features), `c28089a` (sidebar + 4 pages + threading refactor + daemon fix), `1f4e4a8` (3 distinct fixes), `8c301d5` (5 tracks) | Documented in CHANGELOG; future commits subject to the "one logical change" rule in AGENTS.md |
| Commit message misrepresents scope | `b204460` claims "revert" but adds new code (`impl Threading`, struct rename, Mutex elimination); `8c301d5` claims a `get_stabbied` test that doesn't exist | The fabricated test now genuinely exists at `crates/linsight-plugin-sdk/tests/dynamic_load.rs` |
| Zero tests on critical code | mTLS tunnel binds `0.0.0.0:9443` with no tests; `export_plugin!` macro is the whole plugin author surface with no tests; `OverviewModel.{save,load}_layout` JSON path with no tests | mTLS smoke test added; SDK dynamic-load test added |
| Diagnostic instrument blindness | `476e3c4` ships a 153-line "cxx-qt upstream bug" writeup based on `spectacle`/`grim` observations while *in the same commit* writing the screenshot path that exists because those tools are unreliable on Wayland | Already self-corrected in `b204460`; documented in AGENTS.md so the pattern is named |
| Silent fix-it-later follow-ups | "Progress" commit `f0f70b7` quietly fixed: missing graceful shutdown, missing connection cap, stale-socket TOCTOU, FFI `SensorId::new` bypass, `to_string_lossy` sysroot corruption, `pub sysroot` field, missing `just credits` target, i18n-extract gap, broken canvas-editor drag, broken Credits XHR | All resolved + documented in CHANGELOG |
| Misleading docs within hours of being written | ADR-0001 described a deferral that was implemented 3.5 hours later with a different technical approach; `1a9bde3` cross-referenced the wrong cxx-qt bug writeup as canonical architecture | ADR-0001 amended to status "superseded"; misleading docs corrected |
| Magic numbers + design-token bypass | DesignTokens singleton introduced in `c28089a`, immediately bypassed on the AboutPage hero; `200/120/100/60` magic tile dimensions; `-1` sentinel filter with no constant | AboutPage hero now uses `markPanelDeep/Top/Bar` tokens; canvas editor uses `defaultTileW/H`; CategoryPage `-1` filter is documented |

## Per-commit verdicts (most severe first)

### `021e1d2` — REJECT (mega-commit, multiple features broken)
Four unrelated features squashed with **0% test coverage**. mTLS
tunnel: `tokio/signal` Cargo feature declared but never imported,
no graceful shutdown, no connection cap. Canvas editor: drag
mechanism mixes `Drag.Automatic` with `MouseArea.drag.target`
(visual proxy teleports). Brand "icons" shipped as in-house
gradient placeholder without the word "placeholder" anywhere.
`--reduce-motion` parsed in QML, invisible to `--help`.
**Resolved:** All of the above corrected; tunnel and canvas editor
each got proper test coverage; `--reduce-motion` moved to clap;
brand placeholder status documented in open-followups.

### `8c301d5` — REJECT (FFI release-mode bugs, fabricated test claim)
Two release-mode correctness bugs: `From<RSensorId> for SensorId`
uses `SensorId::new` (debug-only guard); `From<&PluginCtx> for
RPluginCtx` uses `to_string_lossy`. `PluginCtx::sysroot` is `pub`.
Commit message claims a scaffolded plugin was loaded via
`StabbyLibrary::get_stabbied` — no such test exists. Pop-twice
allocation antipattern duplicated inline. Six sensor shim crates
copy-pasted with two different styles.
**Resolved:** `host_init` now runs `SensorId::try_new` on every raw
FFI string before conversion. `PluginCtx::new_with_sysroot`
rejects non-UTF-8 paths up front. `PluginCtx::sysroot` is private.
Real `dynamic_load.rs` integration test added that exercises the
full load path. `svec_into_std` rewritten as a single forward
drain; both call sites use the shared helper. All six sensor shims
unified.

### `c28089a` — REJECT (7380 lines, broken Credits, fake status)
Bundles a non-trivial OverviewModel threading refactor and an
unrelated daemon-side `alerts.rs` cleanup inside a 7380-line
"sidebar + pages" commit. DesignTokens singleton introduced and
immediately bypassed on the AboutPage hero. Settings page env-var
status indicators show identical icons regardless of state — fake
status display. CreditsPage shipped with an XHR fallback that
tells users to run `just credits` — a target that didn't exist
until 3 commits later. `i18n-extract` Justfile target left 7 new
QML files invisible to lupdate. `about.toml` skip-list names
3 phantom crates that don't exist in the workspace.
**Resolved:** `OverviewModel.envIsSet(name)` invokable added;
SettingsPage indicators reflect real env state. AboutPage hero now
uses `markPanelDeep/Top/Bar` design tokens. `just credits` target
added. `i18n-extract` covers all 11 QML files with `qsTr()`.
`about.toml` skip-list replaced with the real 13-crate workspace
listing.

### `476e3c4` — REJECT (premature upstream-bug writeup)
Two unrelated things in one commit; the entanglement directly
caused the messy revert that followed. Author wrote a 153-line
upstream cxx-qt bug report based on `spectacle`/`grim` observations
while *in the same commit* writing the screenshot path that exists
because spectacle/grim are unreliable for unfocused Wayland
windows. Used certainty-language ("we confirmed") for claims that
were not verified through the compositor-bypass channel they were
simultaneously writing. Magic numbers in `screenshot.cpp` (20/100/50)
without comment. `screenshot_delay_ms: i32` accepts negative values
silently. Default delay 2500 in Rust vs hardcoded 2800 in
`dev_screenshot.sh`.
**Resolved:** Self-corrected by the subsequent revert `b204460`.
Magic numbers now named `kRetryIntervalMs`, `kMaxGrabRetries`,
`kPostSaveSettleMs` with comments. `screenshot_delay_ms` is now
`u32` clamped to `[0, 30000]`. `dev_screenshot.sh` omits the flag
entirely so script and Rust default can't drift. Path is
pre-validated in Rust before Qt boots.

### `1f4e4a8` — NEEDS REWORK (3 fixes stapled, fragile error sniff)
Three independent fixes under one subject. `showResult` uses
string-prefix sniffing for error/success — would produce garbled
banners like `"Saved to error: ..."` on edge paths. `BUNDLED_CREDITS`
and `BUNDLED_GPL` bolted onto `OverviewModel` (a live sensor-data
bridge). `property var dashModel: null` used across 6 pages —
disables QML's type system entirely. `CategoryPage.qml:29` `-1`
sentinel filter with no constant, no comment, no test.
**Resolved:** `showResult` replaced with discriminated
`showSuccess`/`showError` + `isLayoutError` helper. `dashModel`
now `property QtObject` (typed) across all 6 pages. `-1` filter
narrowed to exact-match against the units the kernel actually
writes `-1` to, with comment explaining the contract and pointing
at the long-term fix.

### `b204460` — NEEDS REWORK (not a clean revert)
Not a revert. Adds new code: `impl cxx_qt::Threading for OverviewModel`,
`SharedState` → `SampleState` rename, Mutex elimination, worker-thread
restructure. The changes are correct but burying new code in a revert
commit makes `git bisect` lie. Post-mortem language ("I almost filed a
non-bug upstream") underplays what happened: a 153-line bug report
*was committed* and was one `git push` from embarrassing the team.
**Resolved:** Documented as a teaching moment in AGENTS.md's GUI
conventions section; the actual correction (Wayland screenshot trap
named explicitly) was made then and stands now. Future reverts
should restore the exact prior state.

### `1a9bde3` — REJECT (docs ship two layers of lies)
"No code changes" but ships two layers of lies: `docs/architecture.md`
data-flow steps 6–7 bake the misdiagnosed Timer/tick workaround into
the canonical architecture description, cross-referencing the unverified
cxx-qt bug writeup. README claims "all 10 spec phases shipped" while
listing `linsight-tunnel` — which had zero tests at that point.
`build-and-test.md` documents a CI runner workflow that doesn't exist.
**Resolved:** Architecture doc reverted (already during `b204460`).
Tunnel now has 2-test mTLS smoke. `build-and-test.md` rewritten to
say honestly "there is no hosted CI yet."

### `4e403b1` — REJECT (test isn't a real gate)
`timeout 12 ... || true` makes a 12-second hang indistinguishable
from success. Not in `just ci` — never runs automatically.
Skip-when-xvfb-absent exits 0 (silent skip = silent pass). Asserts
only that the daemon-handshake log line fires, which happens
*before* QML loads — a completely blank UI passes this test.
**Resolved:** `scripts/gui_smoke.sh` rewritten. Timeout (exit 124)
now distinct from clean exit. Skip uses GNU automake's exit 77.
Hard failures exit 99. Reports binary exit code in the pass
message so a hung-but-handshook GUI is not conflated with a clean
run. `build-and-test.md` clarifies the script is a local dev tool,
not a CI gate.

### `5d5cc0a` — NEEDS REWORK (version drift in packaging)
Bumps the Cargo workspace to v0.3.0 but leaves 4 packaging files
(`arch/PKGBUILD`, `arch-v3/PKGBUILD`, `AppImageBuilder.yml`,
`metainfo.xml`) at 0.1.0. User-visible: `pacman -Q` reports 0.1.0
while the binary says 0.3.0.
**Resolved:** All packaging files bumped to 0.3.0 with appropriate
release notes (the metainfo.xml entry now includes a real
description of the v0.3.0 feature set).

### `461b162` — NEEDS REWORK (ADR predicts the wrong implementation)
ADR-0001 written *before* the implementation; the implementation
(in `8c301d5`) used a different technical approach (R-mirror types
rather than direct stabby annotations on `linsight-core` types).
The ADR was never amended. Missing "Consequences" section per
Nygard's template. Status reads "accepted" (deferral) while the
codebase shows it shipped 3.5 hours later.
**Resolved:** ADR-0001 now has status "superseded — implementation
landed 2026-05-25 as `LINSIGHT_PLUGIN_ABI_VERSION = 2`", a full
"What we learned implementing it" section, and a proper
"Consequences" section per Nygard's template.

## Lessons logged for future contributors

- **One commit, one logical change.** If the subject line has
  "and", "+", "plus", or a semicolon joining two scopes, split it.
- **A revert restores the prior state. New code in a revert commit
  is a refactor wearing a revert label** — that breaks `git bisect`
  and `git blame` forever.
- **`debug_assert!` does not exist at an FFI boundary.** Every
  untrusted value entering your type system must go through a
  fallible constructor.
- **`to_string_lossy` on any path crossing a trust boundary is
  almost always wrong.**
- **Rule out your observation instrument before diagnosing the
  observed system.** If you observed the bug only through a tool
  you're already replacing, use the replacement before writing the
  bug report.
- **"Done" means tested, not just compiled.** A network-facing
  binary with zero test coverage isn't done regardless of how clean
  the code looks.
- **A test that can't tell a timeout from success is a liability,
  not a gate.** A test that's never invoked by CI isn't a test at
  all.
- **Generated files committed to the repo need a justification in
  the commit message** — otherwise they'll produce massive noise
  diffs on every regeneration.
- **`property var` on a QObject reference opts out of QML's type
  system.** Use `property QtObject` or a typed alias.
- **Update ADRs *with* the implementation, not "in a follow-up."**
  Follow-ups don't happen.

These rules now live in AGENTS.md for future runs.
