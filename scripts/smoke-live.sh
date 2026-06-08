#!/usr/bin/env bash
# Live end-to-end smoke test for visualops-mcp against the macOS Notes app.
# Builds the server, ensures Notes is running, then runs scripts/smoke-live.py.
# Safe & idempotent: it writes a throwaway test note and proves the risk gate
# blocks destructive actions — it never approves or executes one.
#
# Usage: scripts/smoke-live.sh [app_name]   (default app: Notes)
set -euo pipefail

cd "$(dirname "$0")/.."

APP="${1:-Notes}"

echo "==> building visualops-mcp"
cargo build -q -p visualops-mcp

BIN="target/debug/visualops-mcp"
[[ -x "$BIN" ]] || { echo "error: $BIN not found after build" >&2; exit 1; }

echo "==> ensuring $APP is running"
open -a "$APP" || true
sleep 1
PID="$(pgrep -x "$APP" | head -1 || true)"
[[ -n "$PID" ]] || { echo "error: could not find pid for $APP" >&2; exit 1; }
echo "    $APP pid=$PID"

echo "==> driving live MCP smoke (window 0 -> AXMainWindow)"
exec python3 scripts/smoke-live.py "$BIN" "$PID" 0
