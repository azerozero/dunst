# WP-C — `visualops-platform`: batch AX reads (owner: Codex / tmux 2)

Work **only** in `crates/visualops-platform`. Do **not** touch `visualops-core`
/ `visualops-graph` / `visualops-mcp`. Do **not** run git. Keep
`cargo build -p visualops-platform` green and `examples/dump` working on a live
window.

## Context
`walk_element` currently reads each AX attribute with a **separate** synchronous
AX message (role, value, title, description, help, identifier, position, size,
enabled, focused, actions, children …). On a 400–5000 node tree that is
thousands of round-trips to the target app — the dominant cost of `capture`, and
the reason a live refresh in `visualops-mcp` is slow. This was left as a
`// PERF:` note (A1) in `docs/FIX-platform.md`, gated on having a baseline.

## C1 [PERF, main task] Batch reads via `AXUIElementCopyMultipleAttributeValues`
Replace the N per-attribute reads in `walk_element` with **one** batched call per
element:

- Declare the FFI (it is not in `accessibility-sys`):
  ```rust
  extern "C" {
      fn AXUIElementCopyMultipleAttributeValues(
          element: AXUIElementRef,
          attributes: CFArrayRef,
          options: AXCopyMultipleAttributeOptions, // 0 = stop on error off; use kAXCopyMultipleAttributeOptionStopOnError = 0 ... pass 0
          values: *mut CFArrayRef,
      ) -> AXError;
  }
  ```
  Pass `options = 0` so a missing attribute yields a `kCFNull` slot instead of
  aborting the whole batch. The returned `CFArray` is **positional**: index i in
  the result corresponds to attribute i in the request array. A slot is either
  the value or a `kCFNull` (treat as `None`); also tolerate an `AXValue` wrapping
  a `kAXErrorNoValue` if the OS returns that form.
- Build the request `CFArray<CFString>` **once** (it is identical for every
  element) and reuse it across the whole walk — do not rebuild per node.
- Map each result slot through the **same** typed extractors you have today
  (`attr_string` logic → from a `CFTypeRef`, `attr_ax_value` → position/size,
  bool for enabled/focused, `CFArray` for actions/children). Factor the
  CFType→Rust conversion out of the current `attr_*` helpers so both the batched
  path and any remaining single reads share it.
- **Children** (`kAXChildrenAttribute`) still come back as a `CFArray` of
  `AXUIElementRef`; keep the existing **`CFRetain` + RAII `AxElement`** ownership
  rule (P1) — retain each child you keep, free via `Drop`. The batch call does
  not change ownership semantics, only how many messages you send.
- Preserve behaviour exactly: same `RawAxNode` fields, same label-derivation rule
  (`title → description → value-only-if-AXStaticText`), same `ax_identifier`,
  same menubar second-root (P6), same 1.0s messaging timeout (P4),
  same `MAX_NODES` cap.

## C2 [MEASURE, required] Capture-latency baseline + after
The batch win must be **shown**, not assumed.

- Add an opt-in timing line to `examples/dump`: measure wall-clock around
  `backend.capture(&target)` and print `captured N nodes in M.MMM ms` to
  **stderr** (keep stdout pure JSON). Gate it on an env var (e.g.
  `VO_DUMP_TIMING=1`) or always print to stderr — your call, but stdout stays
  machine-readable.
- Report in your finish summary: node count + capture ms **before** (current
  per-attribute code, from git stash or a quick revert) and **after** (batched),
  on the same live window (Notes is the reference target). One number each is
  enough; if `leaks`/Instruments is handy, note allocations too.

> Note: `Date.now()`-style timing inside the pure crates is fine here because this
> is `examples/`, not library logic — use `std::time::Instant`.

## Out of scope (leave as-is / note only)
- Concurrency / parallel walks — single-threaded batched reads first.
- `m1` Type CGEvent fallback, `m4` real Drag — still post-POC.
- Any change to `RawAxNode` shape or to graph/core — if you think the contract
  needs a field, **stop and write it in the summary**, don't edit `core`.

Finish: print `cargo build -p visualops-platform` result, a `dump` run showing
**both roots** (window + menubar) with the timing line, and a before/after
capture-ms summary.
