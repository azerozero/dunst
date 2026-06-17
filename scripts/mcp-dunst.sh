#!/usr/bin/env bash
# Development stdio MCP entrypoint for dunst-mcp.
#
# Keeps stdout reserved for JSON-RPC. Build output goes to stderr.
# Installed/user configs should prefer: dunst-mcp serve
#
# Environment:
#   DUNST_MCP_BIN=/path/to/dunst-mcp  use an explicit binary (must exist + be executable)
#   DUNST_MCP_MODE=fixture|live       fixture serves the deterministic Notes fixture; live attaches to a window
#   DUNST_MCP_APP=Chrome              in live mode, attach at startup to the largest window for an app
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/mcp-dunst.sh

Development wrapper for dunst-mcp stdio. stdout is reserved for JSON-RPC.

Environment:
  DUNST_MCP_BIN=/path/to/dunst-mcp  use an explicit binary (must be executable)
  DUNST_MCP_MODE=fixture|live       default: live
  DUNST_MCP_APP="Google Chrome"     live mode: target the largest window for this app
USAGE
}

main() {
  if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
  fi

  cd "$(dirname "$0")/.."

  case "${DUNST_MCP_MODE:-live}" in
    fixture|live) ;;
    *)
      echo "dunst-mcp wrapper: invalid DUNST_MCP_MODE='${DUNST_MCP_MODE}' (expected fixture|live)" >&2
      exit 2
      ;;
  esac

  local bin
  if [[ -n "${DUNST_MCP_BIN:-}" ]]; then
    bin="$DUNST_MCP_BIN"
    if [[ ! -x "$bin" ]]; then
      echo "dunst-mcp wrapper: DUNST_MCP_BIN is not executable: $bin" >&2
      exit 2
    fi
  else
    bin="target/debug/dunst-mcp"
    if [[ ! -x "$bin" ]]; then
      cargo build -q -p dunst-mcp >&2
    fi
    if [[ ! -x "$bin" ]]; then
      echo "dunst-mcp wrapper: build did not produce executable $bin" >&2
      exit 2
    fi
  fi

  if [[ "${DUNST_MCP_MODE:-live}" == "fixture" ]]; then
    exec "$bin" serve
  fi

  if [[ -n "${DUNST_MCP_APP:-}" ]]; then
    exec "$bin" serve --app "$DUNST_MCP_APP"
  fi

  exec "$bin" serve --live
}

main "$@"
