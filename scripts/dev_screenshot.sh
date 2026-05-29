#!/usr/bin/env bash
# Iterative dev screenshot helper. Drives LinSight's own
# QQuickWindow::grabWindow path via `--screenshot <path>` so the
# resulting PNG reflects whatever the QML scene rendered most
# recently — independent of compositor focus or Wayland surface
# caching. Reduce-motion is enabled so animations don't bleed into
# captured frames.
#
# Usage:  ./scripts/dev_screenshot.sh [page] [out]
#   page  one of: overview gpus storage network hardware editor settings
#         about licenses credits  (default: overview)
#   out   destination PNG path (default: /tmp/linsight-shot.png)
set -uo pipefail
PAGE="${1:-overview}"
OUT="${2:-/tmp/linsight-shot.png}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Kill only the GUI binary, not the daemon (pkill -x is exact-match).
pkill -TERM -x linsight 2>/dev/null || true
sleep 0.3
pkill -KILL -x linsight 2>/dev/null || true
sleep 0.2

# Ensure the daemon is running.
if ! pgrep -x linsightd >/dev/null; then
    rm -f "${XDG_RUNTIME_DIR:-/tmp}/linsight.sock"
    target/debug/linsightd --socket "${XDG_RUNTIME_DIR:-/tmp}/linsight.sock" \
        >/tmp/linsightd.log 2>&1 &
    sleep 1.0
fi

cargo build -p linsight 2>&1 | tail -1 >&2

rm -f "$OUT"

# LinSight's --screenshot path arms a QTimer that calls
# QQuickWindow::grabWindow() after --screenshot-delay ms, writes PNG,
# and exits the event loop with code 0 on success. We omit the
# `--screenshot-delay` flag entirely so the binary uses its compiled-in
# default (`DEFAULT_SCREENSHOT_DELAY_MS` in main.rs); keeping the value
# in one place stops the script and the Rust default from drifting.
target/debug/linsight \
    "$PAGE" \
    --reduce-motion \
    --screenshot "$OUT" \
    >/tmp/linsight-shot.log 2>&1
rc=$?

if [ -f "$OUT" ] && [ $rc -eq 0 ]; then
    echo "$OUT"
else
    echo "FAILED: linsight exited rc=$rc, see /tmp/linsight-shot.log" >&2
    tail -20 /tmp/linsight-shot.log >&2
    exit 1
fi
