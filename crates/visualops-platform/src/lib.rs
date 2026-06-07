//! macOS platform backend: the **real** [`Perceptor`] (AX tree walk) and
//! [`ActionExecutor`] (perform AX action / set value / CGEvent).
//!
//! This is the only crate that touches macOS FFI. See `docs/WP-A-platform.md`
//! for the full spec, the AX attribute list, and done-criteria.

use visualops_core::{
    ActionExecutor, Perceptor, RawAxNode, Result, SceneNode, SemanticAction, Target, VisualOpsError,
    WindowRef,
};

/// AX-backed perception + action for macOS.
#[derive(Debug, Default)]
pub struct MacosBackend {
    _private: (),
}

impl MacosBackend {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Perceptor for MacosBackend {
    fn capture(&self, target: &Target) -> Result<Vec<RawAxNode>> {
        let _ = target;
        // WP-A: AXUIElementCreateApplication(pid) -> find window by id ->
        // recursively read AXRole/AXTitle/AXHelp/AXValue/AXIdentifier/
        // AXActions(AXUIElementCopyActionNames)/AXFrame -> RawAxNode tree.
        Err(VisualOpsError::Perception("WP-A: not yet implemented".into()))
    }

    fn window_ref(&self, target: &Target) -> Result<WindowRef> {
        let _ = target;
        Err(VisualOpsError::Perception("WP-A: not yet implemented".into()))
    }
}

impl ActionExecutor for MacosBackend {
    fn perform(
        &self,
        target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> Result<()> {
        let _ = (target, node, action, argument);
        // WP-A: resolve `node` back to its AXUIElement (via ax_identifier +
        // role + label, re-walking if needed), then:
        //   Click/Pick -> AXUIElementPerformAction(kAXPressAction)
        //   OpenMenu   -> performAction("AXShowMenu")
        //   Type       -> AXUIElementSetAttributeValue(kAXValueAttribute, text)
        //                 or CGEvent keystrokes
        //   Raise      -> performAction(kAXRaiseAction)
        Err(VisualOpsError::Execution("WP-A: not yet implemented".into()))
    }
}
