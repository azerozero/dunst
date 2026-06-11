//! The two platform boundaries. Implemented for real in `dunst-platform`
//! (macOS AX) and faked in [`crate::mock`] for device-free testing.

use crate::types::{RawAxNode, SceneNode, SemanticAction, WindowRef};
use crate::Result;

/// A target window to perceive / act on.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub pid: i32,
    pub window_id: u32,
}

/// Turns a live UI into raw AX nodes. The *only* part that must touch macOS
/// for perception. Everything downstream (scene graph, affordances, risk) is
/// pure logic over [`RawAxNode`].
pub trait Perceptor: Send + Sync {
    /// Walk the target window and return its root node(s).
    fn capture(&self, target: &Target) -> Result<Vec<RawAxNode>>;

    /// Metadata for the target window (pid, id, app, title).
    fn window_ref(&self, target: &Target) -> Result<WindowRef>;
}

/// Performs a resolved semantic action against a concrete element. The MCP
/// server resolves an element ID to a [`SceneNode`] (which carries the native
/// `ax_actions` and `ax_identifier`) and hands it here. Implementations map the
/// semantic action onto an AX `performAction`, `setValue`, or CGEvent.
pub trait ActionExecutor: Send + Sync {
    fn perform(
        &self,
        target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> Result<()>;
}
