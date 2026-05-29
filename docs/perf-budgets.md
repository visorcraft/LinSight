<!-- SPDX-FileCopyrightText: 2026 VisorCraft LLC -->
<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Performance budgets

Targets enforced by the architecture (subscription-driven sampling,
no async runtime in the hot path, `lto=fat` + `codegen-units=1` +
`panic=abort` + stripped) and verified periodically by
`cargo bench` in CI. A regression > +20% on any of these is treated
as a perf bug, not just slow code.

## Daemon

| Metric | Budget | Measured |
|---|---|---|
| RSS, idle (no subscribers) | ≤ 7 MB | ~5 MB |
| RSS, full Overview + 1 plugin | ≤ 12 MB | ~7 MB |
| RSS, always-on (history + alerts + Prometheus) | ≤ 20 MB | ~12 MB |
| CPU, Overview visible (~6 sensors @ 1–4 Hz) | ≤ 0.5% of one core | ~0.3% |
| CPU, idle (epoll wait, no subs) | < 0.05% | ~0 |
| Subscribe → first sample latency | ≤ 60 ms | ~15 ms |

## GUI

| Metric | Budget | Notes |
|---|---|---|
| RSS, Overview page visible | ≤ 140 MB | Qt 6 + Kirigami baseline ~100 MB |
| Cold start to interactive | ≤ 700 ms | Qt QML startup is the bottleneck |
| Tile update latency end-to-end | ≤ 50 ms | sample → daemon → socket → QML repaint |

## Wire protocol

| Metric | Budget | Notes |
|---|---|---|
| Per-sample serialized size | ≤ 64 B | `postcard` varint encoding |
| Per-sample encode cost | ≤ 5 µs | `postcard::to_allocvec` |
| Per-sample decode cost | ≤ 5 µs | `postcard::from_bytes` |

## Release binary sizes

| Binary | Target | v0.3.0 |
|---|---|---|
| `linsightd` | ≤ 5 MB | ~1.3 MB |
| `linsight-cli` | ≤ 5 MB | ~1.3 MB |
| `linsight` (GUI) | ≤ 20 MB | larger; Qt is linked dynamically against system libs |
| `linsight-tunnel` | ≤ 8 MB | ~3 MB (rustls + ring + tokio runtime) |

## Methodology

Each metric is exercised by a `cargo bench` group in
`crates/<crate>/benches/`. We bias toward fewer, well-named benchmarks
that catch real regressions over many small ones that drown signal
in noise. The bench harness is `criterion` for everything except the
end-to-end latency tests, which use raw `std::time::Instant`.

When a budget is violated:

1. First check whether `RUSTFLAGS` accidentally diverged
   (`target-cpu`, debug overrides) — the release profile baseline
   is `lto=fat, codegen-units=1, panic=abort, strip=symbols,
   opt-level=3`.
2. Then check whether a new dependency landed that pulls in heavier
   defaults (e.g. enabling Tokio default features).
3. Only then optimize the code itself.
