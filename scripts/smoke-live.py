#!/usr/bin/env python3
"""Live end-to-end smoke test for the dunst stdio MCP server (WP-E).

Drives `dunst-mcp serve --pid <pid> --window <win>` against a real macOS
window and proves the full risk-gated loop **without** ever executing a
destructive action:

  1. initialize / tools/list
  2. refresh (live AX perceive)
  3. click a low-risk button (Nouvelle note)  -> executes
  4. type into the note body                  -> executes, verify_state confirms
  5. click a HIGH-risk menu item (Éteindre)   -> pending_approval (gate proven)
     ^ deliberately NOT approved: this script never runs an irreversible action.
  6. export_trace (audit) + summary

Exit code 0 only if every assertion holds. Safe to run repeatedly / from a loop.

Usage: scripts/smoke-live.py <bin> --app <app_name>
       scripts/smoke-live.py <bin> <pid> <window_id>
"""
import json
import subprocess
import sys
import time

if len(sys.argv) != 4:
    sys.exit("usage: smoke-live.py <dunst-mcp-bin> (--app <app_name> | <pid> <window_id>)")
BIN = sys.argv[1]
if sys.argv[2] == "--app":
    target_args = ["serve", "--app", sys.argv[3]]
else:
    target_args = ["serve", "--pid", sys.argv[2], "--window", sys.argv[3]]

NOTE_TEXT = "Dunst MCP — note de test live (WP-E), ecrite via type_into du serveur MCP."

proc = subprocess.Popen(
    [BIN, *target_args],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    text=True, bufsize=1,
)
_id = 0
failures = []


def call(method, params=None):
    global _id
    _id += 1
    req = {"jsonrpc": "2.0", "id": _id, "method": method}
    if params is not None:
        req["params"] = params
    proc.stdin.write(json.dumps(req) + "\n")
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())


def tool(name, **args):
    res = call("tools/call", {"name": name, "arguments": args}).get("result", {})
    txt = res.get("content", [{}])[0].get("text", "")
    try:
        val = json.loads(txt)
    except (ValueError, TypeError):
        val = txt
    return val, res.get("isError", False)


def check(cond, label):
    print(("  PASS " if cond else "  FAIL ") + label)
    if not cond:
        failures.append(label)


print("=== initialize ===")
info = call("initialize").get("result", {}).get("serverInfo", {})
check(info.get("name") == "dunst", f"server is dunst ({info})")
tool("refresh")

print("\n=== low-risk: create a note ===")
hits, _ = tool("find_element", query="Nouvelle note")
create = next((n for n in hits if n.get("role") == "button"), hits[0] if hits else None)
check(create is not None, "found 'Nouvelle note' button")
if create:
    out, err = tool("click_element", id=create["id"], reasoning="create a note for the live smoke test")
    res = out.get("result") if isinstance(out, dict) else out
    check(not err and res == "success", f"click {create['id']} -> {res}")
    time.sleep(0.6)
    tool("refresh")

print("\n=== low-risk: type the note body ===")
type_ids, _ = tool("query_affordances", action="type")
body_id = type_ids[0] if isinstance(type_ids, list) and type_ids else None
check(body_id is not None, f"found a type-capable element ({type_ids})")
if body_id:
    out, err = tool("type_into", id=body_id, text=NOTE_TEXT, reasoning="write the live test note body")
    res = out.get("result") if isinstance(out, dict) else out
    check(not err and res == "success", f"type_into {body_id} -> {res}")
    time.sleep(0.5)
    tool("refresh")
    vs, _ = tool("verify_state", id=body_id, field="value", expected=NOTE_TEXT)
    check(isinstance(vs, dict) and vs.get("matches") is True, f"verify_state value == note ({vs})")

print("\n=== HIGH-risk: Éteindre must be gated (NOT approved) ===")
hits, _ = tool("find_element", query="Éteindre")
hr = next((n for n in hits if (n.get("label") or "").strip().lower().startswith("éteindre")), hits[0] if hits else None)
check(hr is not None, "found 'Éteindre' menu item")
gated_id = None
if hr:
    # NB: find_element returns a SceneNode (no risk field); risk lives in the
    # affordance graph and in the action outcome. Assert via the gate result +
    # the audit trail below, not via the scene node.
    out, err = tool("click_element", id=hr["id"], reasoning="attempt shutdown (must be gated)")
    res = out.get("result") if isinstance(out, dict) else out
    check(not err and res == "pending_approval", f"click {hr['id']} -> {res} (gate holds)")
    gated_id = hr["id"]
    print("  (approve deliberately NOT called — destructive action left gated)")

print("\n=== audit trail ===")
trace, _ = tool("export_trace")
for e in (trace if isinstance(trace, list) else []):
    print("  %-6s %-26s risk=%-7s result=%s" % (
        e.get("action"), e.get("target_id"),
        (e.get("risk") or {}).get("level"), e.get("result")))
check(any(e.get("result") == "pending_approval" and (e.get("risk") or {}).get("level") == "high"
          and (gated_id is None or e.get("target_id") == gated_id)
          for e in (trace if isinstance(trace, list) else [])),
      "audit contains the gated HIGH-risk attempt (risk=high, pending_approval)")

proc.stdin.close()
try:
    proc.wait(timeout=5)
except subprocess.TimeoutExpired:
    proc.kill()

print(f"\n[smoke-live] {'OK' if not failures else 'FAILED: ' + ', '.join(failures)}")
sys.exit(1 if failures else 0)
