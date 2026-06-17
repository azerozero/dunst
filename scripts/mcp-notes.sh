#!/usr/bin/env bash
# Compatibility wrapper for the historical Notes-focused MCP entrypoint.
#
# Prefer scripts/mcp-dunst.sh for new config. This shim maps VO_APP to the
# canonical DUNST_MCP_APP contract and delegates.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/mcp-notes.sh

Compatibility wrapper for Notes-oriented local sessions.

Environment:
  VO_APP=Notes                  legacy target app name
  DUNST_MCP_APP="Google Chrome" canonical target app name
  DUNST_MCP_BIN=/path/to/dunst-mcp
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

cd "$(dirname "$0")/.."

export DUNST_MCP_MODE="${DUNST_MCP_MODE:-live}"
export DUNST_MCP_APP="${DUNST_MCP_APP:-${VO_APP:-Notes}}"

exec scripts/mcp-dunst.sh
