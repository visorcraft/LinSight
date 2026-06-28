// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Shared worker-RPC helper for QObject invokables.
//!
//! `spawn_rpc` encapsulates the pattern of:
//!   1. spawning a background thread to perform a blocking RPC call,
//!   2. queuing the result back onto the Qt event loop, and
//!   3. discarding stale results via a `generation` counter.

use std::sync::{Condvar, LazyLock, Mutex};
use std::thread;

use cxx_qt::CxxQtThread;
use cxx_qt::CxxQtType;
use cxx_qt::Threading;

/// Maximum number of RPC fetch closures executing concurrently. Excess
/// threads are spawned as before (the GUI thread never blocks on
/// `spawn_rpc`) but park on the semaphore until a slot frees, preventing
/// a burst of history-range clicks or host switches from spawning dozens
/// of simultaneous blocking RPC threads.
const MAX_CONCURRENT_RPC: usize = 16;

static RPC_SEMAPHORE: LazyLock<(Mutex<usize>, Condvar)> =
    LazyLock::new(|| (Mutex::new(0), Condvar::new()));

/// Acquire a concurrency slot. Called from inside the spawned thread so
/// the GUI thread is never blocked.
fn acquire_rpc_slot() {
    let (lock, cvar) = &*RPC_SEMAPHORE;
    let mut count = lock.lock().unwrap();
    while *count >= MAX_CONCURRENT_RPC {
        count = cvar.wait(count).unwrap();
    }
    *count += 1;
}

/// Release a concurrency slot and wake one waiting thread.
fn release_rpc_slot() {
    let (lock, cvar) = &*RPC_SEMAPHORE;
    let mut count = lock.lock().unwrap();
    *count -= 1;
    cvar.notify_one();
}

/// Staleness contract for QObjects that own a `request_generation` counter.
///
/// Implementing this on the Rust backing struct lets `spawn_rpc` own the
/// stale-completion guard so call sites do not repeat it.
pub(crate) trait RequestGenerated {
    fn request_generation(&self) -> u64;
    /// Increment the counter and return the new value.
    fn bump_request_generation(&mut self) -> u64;
}

/// Returns `true` when `rust`'s current generation equals the one captured
/// at dispatch — i.e. no newer request has superseded this one.
///
/// Factored out so it can be unit-tested without a Qt object.
pub(crate) fn is_current<G: RequestGenerated>(rust: &G, generation: u64) -> bool {
    rust.request_generation() == generation
}

/// Spawn a background thread that runs `fetch`, then queues `apply` back
/// onto the Qt event loop.
///
/// `generation` is the value produced by `bump_request_generation()` at the
/// call site.  `spawn_rpc` compares it inside the queued closure and silently
/// drops the result if a newer request has been dispatched — `apply` is only
/// called for fresh completions.
///
/// The `queue()` error is ignored: it only fails when the Qt object has been
/// destroyed (teardown), at which point there is nothing left to update.
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
    Obj: Threading + CxxQtType + 'static,
    Obj::Rust: RequestGenerated,
    Fetch: FnOnce() -> R + Send + 'static,
    Apply: FnOnce(std::pin::Pin<&mut Obj>, R) + Send + 'static,
    R: Send + 'static,
{
    thread::spawn(move || {
        acquire_rpc_slot();
        let result = fetch();
        release_rpc_slot();
        let _ = qt_thread.queue(move |mut pin| {
            if !is_current(pin.as_mut().rust(), generation) {
                return;
            }
            apply(pin, result);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockGenerated {
        generation: u64,
    }

    impl RequestGenerated for MockGenerated {
        fn request_generation(&self) -> u64 {
            self.generation
        }
        fn bump_request_generation(&mut self) -> u64 {
            self.generation += 1;
            self.generation
        }
    }

    #[test]
    fn is_current_matches_exact_generation() {
        let g = MockGenerated { generation: 3 };
        assert!(is_current(&g, 3));
    }

    #[test]
    fn is_current_rejects_stale_generation() {
        let g = MockGenerated { generation: 5 };
        assert!(!is_current(&g, 4));
        assert!(!is_current(&g, 6));
    }

    #[test]
    fn bump_returns_incremented_value() {
        let mut g = MockGenerated { generation: 0 };
        assert_eq!(g.bump_request_generation(), 1);
        assert_eq!(g.bump_request_generation(), 2);
        assert_eq!(g.request_generation(), 2);
    }
}
