#!/usr/bin/env bash
# Live end-to-end smoke test for dunst-mcp against the macOS Notes app.
# Builds the server, ensures Notes is running, then runs scripts/smoke-live.py.
# Safe & idempotent: it writes a throwaway test note and proves the risk gate
# blocks destructive actions — it never approves or executes one.
#
# Usage: scripts/smoke-live.sh [app_name]   (default app: Notes)
set -euo pipefail

cd "$(dirname "$0")/.."

usage() {
  cat <<'USAGE'
Usage: scripts/smoke-live.sh [app_name]

Build dunst-mcp, ensure the target app is running, then execute the live
stdio smoke test. Default app: Notes.
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

app="${1:-Notes}"

echo "==> building dunst-mcp"
cargo build -q -p dunst-mcp

bin="target/debug/dunst-mcp"
[[ -x "$bin" ]] || { echo "error: $bin not found after build" >&2; exit 1; }

echo "==> ensuring $app is running"
if ! open -a "$app"; then
  echo "error: failed to launch app: $app" >&2
  exit 1
fi
sleep 1
pid="$(pgrep -x "$app" | head -1 || true)"
[[ -n "$pid" ]] || { echo "error: could not find pid for $app" >&2; exit 1; }
echo "    $app pid=$pid"

echo "==> driving live MCP smoke (window 0 -> AXMainWindow)"
exec python3 scripts/smoke-live.py "$bin" "$pid" 0
