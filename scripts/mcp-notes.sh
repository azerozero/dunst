#!/usr/bin/env bash
# MCP stdio entrypoint for visualops-mcp, wired for Claude Code (.mcp.json).
#
# Resolves the target app's pid at launch (it changes every run) and execs the
# stdio MCP server against its live AX window. Falls back to the deterministic
# Notes fixture when the app isn't running, so the server always comes up and
# its tool surface is introspectable.
#
# Override the target app with VO_APP (default: Notes).
set -euo pipefail

cd "$(dirname "$0")/.."

APP="${VO_APP:-Notes}"
BIN="target/debug/visualops-mcp"

# Build once if needed. Send cargo's chatter to stderr so stdout stays a clean
# JSON-RPC channel for the MCP client.
[[ -x "$BIN" ]] || cargo build -q -p visualops-mcp >&2

PID="$(pgrep -x "$APP" | head -1 || true)"
if [[ -n "$PID" ]]; then
  exec "$BIN" serve --pid "$PID" --window 0   # live: window 0 -> AXMainWindow
else
  exec "$BIN" serve                           # fixture fallback
fi
