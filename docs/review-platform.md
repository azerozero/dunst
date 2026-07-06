> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# Review - `crates/visualops-platform` (macOS AX backend)

**Read-only** review of the macOS FFI backend (`src/lib.rs` module `macos`,
`examples/dump.rs`). No `.rs` file modified. References: the `visualops-core`
contract (frozen, unmodified) and `docs/WP-A-platform.md` (WP spec).

Method: trace Core Foundation ownership (Create/Copy → CFRelease), verify
`unsafe`, null-checks, `AXValueGetValue` type correctness, traversal, mapping of
`SemanticAction`, bounds, and contract compliance. The exact semantics of
`core-foundation 0.10` helpers (`downcast`, `downcast_into`, `wrap_under_get_rule`,
`wrap_under_create_rule`) were reread in the crate source to ground the
retain/release analysis. The crate **compiles with no warnings** on this machine
(`cargo build`/`clippy -p visualops-platform`), so the FFI types
(`AXValueGetValue -> bool`, `kAXValueType*` constants) are consistent.

## Ownership Summary (What Is Correct)

To avoid false positives, here is what is **healthy**:

- `attr_value` (l.346): `AXUIElementCopyAttributeValue` (Copy) → `value`
  wrapped by `wrap_under_create_rule` ⇒ freed at `CFType` Drop. The null-check
  (`err == kAXErrorSuccess && !value.is_null()`) is correct.
- `attr_string`/`attr_bool` (l.359/367): `downcast::<T>()` follows the Get Rule
  (+1 independent, freed at Drop); the source `CFType` is freed separately.
  **Balanced, no leak, no double-free.**
- `attr_array` (l.372): `downcast_into::<CFArray>()` consumes the `CFType`
  without touching the counter (transfer) ⇒ the `CFArray` is freed at Drop.
  **Correct.**
- `action_names` (l.388): `AXUIElementCopyActionNames` (Copy) →
  `wrap_under_create_rule` ⇒ freed. `cf_strings` (l.407) uses
  `wrap_under_get_rule` (+1) then Drop (−1) per element. **Balanced.**
- `attr_ax_element`/`attr_ax_value` (l.377/500): `mem::forget(value)` after type
  check correctly transfers the +1 to the returned raw pointer; on the type
  failure path, the `CFType` is freed correctly (Drop). The **transfer** is
  correct - the defect is downstream (see BLOCKING/MAJOR: the receiver never
  releases it).
- `AXValueGetValue` types: `kAXValueTypeCGRect/CGPoint/CGSize` matched to
  `CGRect`/`CGPoint`/`CGSize` buffers (`core-graphics`, `repr(C)`, `f64`/CGFloat
  fields) - exact 32/16/16-byte sizes. `then_some` confirms a `bool` return.
  **No UB.**
- No panic on missing attribute: `Option` + `unwrap_or(_else)` everywhere
  (l.195, 215, 216...). Compliant with the "No panics" done criteria.
- `MAX_NODES`/`MAX_DEPTH` bounds (l.51-52) are effectively applied in
  `walk_element` (l.220, 227) and `find_element` (l.244). Depth ≤ 40 ⇒ no stack
  overflow risk.

**Soundness conclusion: no double-free, no use-after-free, no UB.**
The only ownership defect is a **leak** (missing release), handled below.

---

## MAJOR

### M1 - Systematic Core Foundation leak: no `CFRelease` is ever executed

**File:line:**
- `release_ax_element` - `src/lib.rs:511-516`: **only** caller of `CFRelease`,
  marked `#[allow(dead_code)]`, **never called**.
- `ax_elements` - `src/lib.rs:435`: `CFRetain(cf_ref)` (+1) on **every**
  element, with no paired release.
- `app_element` - `src/lib.rs:132`: `AXUIElementCreateApplication` (Create, +1)
  never released by `capture` (l.62), `window_ref` (l.76), `perform` (l.93).
- `attr_ax_value` - `src/lib.rs:500-509`: `AXValueRef` (+1 via `forget`) returned
  to `attr_cgrect/cgpoint/cgsize` (l.457/475/487) then never released.
- `attr_ax_element` - `src/lib.rs:377-386`: `AXUIElementRef` (+1) from
  `AXMainWindow` returned to `resolve_window`, never released.

**Problem.** The backend **never releases** a single `AXUIElementRef` or
`AXValueRef` that it owns. Consequences per `capture`:
- the application element (+1),
- the resolved window (+1),
- **every node in the tree**: `walk_element` (l.225-233) iterates
  `ax_elements(&children)` (each child retained +1) then recurses without
  releasing ⇒ up to `MAX_NODES` (5 000) refs leaked per capture,
- **1 to 2 `AXValueRef`s per node** (`AXFrame`, or `AXPosition`+`AXSize`).

`resolve_window` (l.145-175) makes it worse: `ax_elements` retains **all**
windows but returns only one ⇒ the others leak; on failure it rereads
`kAXWindowsAttribute` a 2nd time (l.146 then l.163), retaining everything again.

> ⚠️ **Important nuance - the `CFRetain` in `ax_elements` (l.435) is correct and
> necessary; it is NOT the bug.** In `find_element` (l.238-263), children are
> pushed onto `stack` (l.256) and dereferenced in *later* iterations, **after**
> the parent `CFArray` has been freed (end of the `if let Some(children)` block).
> Without the retain, this would be a **use-after-free**. The defect is therefore
> not the retain but the **absence of the paired `CFRelease`**.

**Severity.** Sound (no UB), and **harmless for the WP-A deliverable**: the
`dump` example (`examples/dump.rs`) is a one-shot process, and the OS reclaims
everything on exit. **But** the contract will be wired into `visualops-mcp`, a
long-running server that captures in a loop: the leak there is **unbounded**
(~5 000 AX refs + AXValues per capture). → **Fix before server integration
(BLOCKING in server context).**

**Concrete fix.** Introduce an owning RAII wrapper, for example:

```rust
struct AxElement(AXUIElementRef);
impl Drop for AxElement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }
}
```

then:
- `ax_elements` returns `Vec<AxElement>` (the current `CFRetain` becomes the +1
  owned by the wrapper); `walk_element`/`find_element` hold `AxElement`s that
  release at Drop. For the element *returned* by `find_element`/`resolve_window`,
  `mem::forget` the wrapper (transfer) or return the `AxElement` itself.
- `app_element` returns an `AxElement` (releases the app at the end of `capture`/
  `window_ref`/`perform`).
- `attr_ax_value`: release the `AXValueRef` after `AXValueGetValue` (RAII wrapper,
  or explicit `CFRelease` at the end of `attr_cgrect/cgpoint/cgsize`).
- `attr_ax_element`: same treatment for the main window.
- Remove `#[allow(dead_code)]` on `release_ax_element` once used, or remove it in
  favor of `Drop`.

---

## MINOR

### m1 - `Type`: no CGEvent fallback although the spec requires it
**`src/lib.rs:107-112`** (and `set_string_attr` l.304). WP-A specifies for
`Type`: "`AXUIElementSetAttributeValue(...)`; **if that errors, fall back to
CGEvent keystrokes**". The implementation returns the AX error directly with no
fallback. Many fields reject `setValue` on `kAXValueAttribute` (web views,
NSTextView through indirect editing) ⇒ `Type` will fail where the spec expects
success. **Fix:** on `Err`, synthesize keystrokes via
`CGEvent::new_keyboard_event` (character sequence) before giving up.

### m2 - `element_matches`: label fallback inconsistent with `walk_element`
**`src/lib.rs:280`** vs **`src/lib.rs:197-205`**. During capture, the label
derives from `title → description → (value only if AXStaticText)`. During
resolution, `element_matches` tries `title → description → value` (**value for
every role**). An element whose `value` (non-empty) matches the searched `label`
can therefore match a different element than the captured one. **Fix:** align
label derivation with `walk_element` (value fallback **only** for `AXStaticText`).

### m3 - `attr_string` masks empty strings → loss of "empty vs absent"
**`src/lib.rs:364`**: `.filter(|s| !s.is_empty())`. A genuinely empty text field
(`AXValue == ""`) becomes `value = None` instead of `Some("")`. For the
diff/audit (`visualops-graph::audit::diff`), "field cleared" can no longer be
distinguished from "no value". **Fix:** filter emptiness only for `label`
(aesthetic ID concern), not for `value`; or preserve `Some("")` for `value`.

### m4 - `Drag` affordance emitted by WP-B but rejected by `perform`
**`src/lib.rs:114-116`**. `perform` returns `Execution(...)` for
`Drag`/`Toggle`/`Scroll` - compliant with the spec ("others → Execution error").
But `visualops-graph::derive_affordances` **exposes `Drag`** on `Row`/`Cell`. An
agent following the affordance graph will therefore request an action that always
fails at execution (no panic, but inter-crate inconsistency). **Fix (platform
side, optional for POC):** implement `Drag` via two CGEvents (mouse down at
source center → mouse up on the target), or explicitly document non-support so
the MCP layer filters `Drag` out of executable actions.

### m5 - First-match re-resolution: nodes with collision suffixes are unreachable
**`src/lib.rs:238-283`**. `find_element` matches `(role, identifier, label)` and
returns the **first** in preorder - explicitly accepted by WP-A ("first match
wins (POC heuristic)"). Limit to document: when `synth_id` (WP-B) had to
disambiguate with `_2/_3` (same role+label+identifier), `find_element` **always
returns the first**. The executor then acts on an element *different* from the one
evaluated by the Risk Engine - a safety implication to keep in mind.
**Fix (post-POC):** propagate a structural index (the WP-B `path`) into
`SceneNode`/resolution, or match by ordinal among elements with the same key.

### m6 - Robustness / efficiency (grouped, non-blocking)
- **`src/lib.rs:146` & `163`**: `resolve_window` reads `kAXWindowsAttribute`
  **twice** (redundant CFArray copy + traversal). Memoize the 1st read.
- **`src/lib.rs:244`**: bound `seen > MAX_NODES || depth > MAX_DEPTH` (strict) vs
  `walk_element` `>=` (l.220) - harmless off-by-one inconsistency.
- **`src/lib.rs:139`**: return value of `AXUIElementSetMessagingTimeout` ignored
  (acceptable, non-critical setter).
- **`src/lib.rs:57,179`**: dependency on private SPI `_AXUIElementGetWindow`
  (recommended by WP-A, with correct fallbacks `AXMainWindow` → 1st window) -
  fragile across macOS versions; fallbacks cover failure, OK for POC.

---

## Informational (Compliant With WP-A - Not a Defect)

- **`src/lib.rs:71`** - `capture` returns a single root (the window). Compliant
  with WP-A ("Return the window element as the single root"). Note for
  integrators: the **menu bar** (and therefore the destructive items
  `Supprimer`/`Éteindre`/`Forcer à quitter`/`Redémarrer…` from the
  `ARCHITECTURE.md` risk scenario) **does not appear** in a live capture - it is
  an attribute of the *application* element (`AXMenuBar`), not a descendant of
  the window. The 2-root fixture is minted separately by the architect; the risk
  demo relies on that fixture, not on live capture. If real menu items are
  needed, `AXMenuBar` on the app will also have to be traversed.
- **`src/lib.rs:81`** - `window_ref.app_name` via the app's `kAXTitleAttribute`
  (often empty ⇒ `""`). Compliant with WP-A, which also mentions
  `NSRunningApplication.localizedName` as a more reliable option.
- **`src/lib.rs:103`** - `Pick → kAXPressAction`: compliant with the WP-A table.
- **`examples/dump.rs`** - pid/window_id parsing, usage message, `exit(1)` on
  error, pretty JSON via `serde_json` (dev-dependency): compliant and clean.
- **`visualops-core` contract**: all `RawAxNode` fields are populated
  (`ax_role`, `label`, `help`, `value`, `ax_identifier`, `ax_actions`, `frame`,
  `enabled`, `focused`, `children`); `ax_actions` normalized (strip `AX` +
  lowercase) ⇒ consistent with `visualops-graph::map_action`. **No contract
  modification.**

---

## Summary by Severity

| Severity | # | Finding |
|---|---|---|
| **BLOCKING** | 0 | No double-free, use-after-free, UB, panic, or build/contract breakage. |
| **MAJOR** | 1 | **M1** Systematic CF leak: `CFRelease` never executed (app + entire AX tree + AXValues leaked per capture). Sound but **unbounded** ⇒ **BLOCKING once wired into the `visualops-mcp` server**. The `CFRetain` (l.435) is correct/necessary (`find_element` safety); the defect is the **absence of release**. Fix: RAII wrapper `Drop→CFRelease`. |
| **MINOR** | 6 | **m1** `Type` without CGEvent fallback (spec not respected) · **m2** `value` label fallback inconsistent between capture/resolution · **m3** `attr_string` masks `""` (empty/absent loss for diff) · **m4** `Drag` exposed by WP-B but rejected by `perform` · **m5** first-match unreachable for IDs suffixed `_2` (executor safety) · **m6** misc (double window read, bound off-by-one, ignored timeout return, private SPI). |

**Verdict.** FFI code is **safe** (no UB, correct null-checks, exact
`AXValueGetValue` types, no panic, bounds respected, contract respected) and
**compliant with WP-A** in functional scope. The only real defect is the
**generalized CF memory leak (M1)**: tolerable for the one-shot `dump`
deliverable, but must be fixed (RAII wrapper) before connecting it to the
long-running server. The MINOR points are robustness/coherence gaps, not
blockers for the POC.
