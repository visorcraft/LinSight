// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

pub mod frame;
pub mod messages;

pub use frame::{FrameError, FrameReader, FrameWriter, MAX_FRAME_BYTES};
pub use messages::*;

/// Wire-format protocol version. Bump only on breaking changes.
pub const PROTOCOL_VERSION: u32 = 2;

/// Minimum value accepted by `RequestOp::SetPumpIntervalMs`. Values
/// below 50 ms are wasteful — the daemon would wake faster than the
/// fastest sensor's native rate (2 Hz / 500 ms) without producing more
/// data, just burning CPU on the empty scheduler ticks.
pub const PUMP_INTERVAL_MIN_MS: u32 = 50;

/// Maximum value accepted by `RequestOp::SetPumpIntervalMs`. Above
/// 1000 ms the GUI's live tile updates feel stale (some sensors only
/// emit at 1 Hz, so a slower pump would drop their effective rate).
pub const PUMP_INTERVAL_MAX_MS: u32 = 1000;

/// Default pump-thread tick used by the daemon if a client never sends
/// `RequestOp::SetPumpIntervalMs`. 150 ms is the middle ground between
/// the historical 50 ms (smooth but ~3% idle CPU) and 250 ms (~0.3%
/// lower CPU but bursty sample arrival).
pub const PUMP_INTERVAL_DEFAULT_MS: u32 = 150;
