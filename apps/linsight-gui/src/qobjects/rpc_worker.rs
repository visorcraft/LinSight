// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Shared worker-RPC helper for QObject invokables.
//!
//! `spawn_rpc` encapsulates the pattern of:
//!   1. spawning a background thread to perform a blocking RPC call,
//!   2. queuing the result back onto the Qt event loop, and
//!   3. discarding stale results via a `generation` counter.

use std::pin::Pin;
use std::thread;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading;

/// Spawn a background thread that runs `fetch`, then queues `apply` back
/// onto the Qt event loop for `obj`.
///
/// `generation` is compared inside `apply` to discard results from
/// superseded requests — callers bump `request_generation` before calling
/// this function and pass the resulting value here.
///
/// `R` is the result payload type.  The caller's `fetch` closure produces it
/// and the `apply` closure consumes it; `Result<String, String>` is the
/// common case but callers may choose any `Send + 'static` type.
pub(crate) fn spawn_rpc<Obj, Fetch, Apply, R>(
    qt_thread: CxxQtThread<Obj>,
    generation: u64,
    fetch: Fetch,
    apply: Apply,
) where
    Obj: Threading + 'static,
    Fetch: FnOnce() -> R + Send + 'static,
    Apply: FnOnce(Pin<&mut Obj>, u64, R) + Send + 'static,
    R: Send + 'static,
{
    thread::spawn(move || {
        let result = fetch();
        let _ = qt_thread.queue(move |pin| apply(pin, generation, result));
    });
}
