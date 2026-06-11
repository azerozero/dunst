#!/bin/bash
# visualops MCP wrapper targeting the on-screen Google Chrome window.
# Self-discovers the window via --app (robust to Chrome recreating windows).
exec /Users/ludwig/workspace/viewcontrolermcp/target/debug/visualops-mcp serve --app "Google Chrome"
