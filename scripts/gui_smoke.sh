#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Headless GUI smoke test. Builds the linsight binary, runs it under
# `xvfb-run` with a short timeout, and asserts the daemon handshake
# completes. Exit codes follow the GNU automake test convention so a
# wrapping CI can distinguish pass / fail / skip:
#
#   0   pass — handshake-complete log line seen within the window
#   1   fail — log present, marker not seen (regression)
#   2   fail — log file not writable / xvfb died / unexpected error
#   77  skip — xvfb-run not installed; not a regression
#   99  fail — hard error (build failed, binary missing)
#
# This script is NOT part of `just ci` — Qt + Mesa want a real GPU
# even for the offscreen platform, so it runs locally / from a
# GPU-equipped runner only. Run from the repository root.

set -euo pipefail

if ! command -v xvfb-run >/dev/null 2>&1; then
    echo "[gui-smoke] xvfb-run not on PATH — skipping"
    exit 77
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[gui-smoke] building release binary"
if ! cargo build -p linsight --release >/dev/null; then
    echo "[gui-smoke] HARD FAIL: cargo build failed"
    exit 99
fi

BIN="$ROOT/target/release/linsight"
if [[ ! -x "$BIN" ]]; then
    echo "[gui-smoke] HARD FAIL: binary missing at $BIN after build"
    exit 99
fi

LOG=$(mktemp)
trap 'rm -f "$LOG"' EXIT

echo "[gui-smoke] launching under xvfb-run (12s window)"
# Do NOT swallow the exit code: distinguish a timeout (exit 124) from
# a normal exit (anything else). `timeout` returns 124 on its own
# timeout; we treat that as expected — we want the binary to keep
# running so the log accumulates — but we still record it.
TIMEOUT_RC=0
timeout --preserve-status 12 xvfb-run --auto-servernum --server-args="-screen 0 1280x720x24" \
    env LINSIGHT_LOG=info "$BIN" >"$LOG" 2>&1 || TIMEOUT_RC=$?

# Read the log first regardless of how the binary exited.
if grep -q "sensor catalogue cached" "$LOG"; then
    echo "[gui-smoke] OK: GUI reached handshake-complete state (binary rc=$TIMEOUT_RC)"
    exit 0
fi

if [[ "$TIMEOUT_RC" -eq 124 ]]; then
    echo "[gui-smoke] FAILED: 12s window elapsed without seeing 'sensor catalogue cached'."
    echo "[gui-smoke] This means the GUI was launched but never completed the daemon handshake."
elif [[ "$TIMEOUT_RC" -ne 0 ]]; then
    echo "[gui-smoke] FAILED: binary exited with code $TIMEOUT_RC before handshake."
else
    echo "[gui-smoke] FAILED: binary exited cleanly but did not log handshake."
fi

echo "[gui-smoke] --- last 200 log lines ---"
tail -n 200 "$LOG"
exit 1
