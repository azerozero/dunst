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
#   DUNST_MCP_ENABLE_APPROVE_TOOL=1   expose the operator-side `approve` tool so gated raw input
#                                     (clicks/keys/scroll) can be approved in-band. Defaults to 1 in
#                                     this local dev wrapper (a controlled single-operator session);
#                                     set to 0 to keep raw input hard-gated.
set -euo pipefail

# Local dev wrapper runs as a controlled single-operator session: expose the
# approve escape hatch by default so pending_approval raw input is reachable.
# Overridable by exporting DUNST_MCP_ENABLE_APPROVE_TOOL=0 before launch.
export DUNST_MCP_ENABLE_APPROVE_TOOL="${DUNST_MCP_ENABLE_APPROVE_TOOL:-1}"

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
    if [[ ! -x "$bin" ]] || [[ -n "$(find Cargo.toml Cargo.lock crates -type f \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'build.rs' \) -newer "$bin" -print -quit)" ]]; then
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
