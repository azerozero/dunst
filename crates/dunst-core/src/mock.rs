//! Device-free fakes. `MockPerceptor` replays a captured AX tree from JSON so
//! the entire graph/affordance/risk/audit pipeline can be developed and tested
//! without macOS, a real app, or accessibility permissions.

use crate::traits::{ActionExecutor, Perceptor, Target};
use crate::types::{RawAxNode, SceneNode, SemanticAction, WindowRef};
use crate::Result;
use std::sync::Mutex;

/// Window id carried by the bundled fixtures. Deliberately far above the range
/// the macOS window server assigns to real on-screen windows (CGWindowIDs start
/// low and increment), so `dunst_vision::capture::window_bounds` reliably returns
/// `None` for it in device-free tests. That keeps geometry guards falling back to
/// the fixture's cached rect instead of colliding with whatever live desktop
/// widget happens to own a small CG window id on the developer's machine — the
/// cause of the "4 raw-approval tests are flaky locally" gremlin.
pub const NOTES_FIXTURE_WINDOW_ID: u32 = 900_000_105;

/// A [`Perceptor`] backed by an in-memory list of root nodes (typically loaded
/// from a fixture such as `fixtures/notes.json`).
pub struct MockPerceptor {
    roots: Vec<RawAxNode>,
    window: WindowRef,
}

impl MockPerceptor {
    /// Builds a mock from in-memory root nodes and a window reference.
    pub fn new(roots: Vec<RawAxNode>, window: WindowRef) -> Self {
        Self { roots, window }
    }

    /// Load roots from a JSON array of [`RawAxNode`].
    ///
    /// # Errors
    ///
    /// Returns an error if `json` is not a valid JSON array of [`RawAxNode`].
    pub fn from_json(json: &str, window: WindowRef) -> Result<Self> {
        let roots: Vec<RawAxNode> = serde_json::from_str(json)?;
        Ok(Self::new(roots, window))
    }

    /// Convenience: the bundled Notes fixture.
    ///
    /// # Errors
    ///
    /// Returns an error if the bundled Notes fixture fails to parse as a JSON
    /// array of [`RawAxNode`].
    pub fn notes_fixture() -> Result<Self> {
        let json = include_str!("../fixtures/notes.json");
        let window = WindowRef {
            pid: 1363,
            window_id: NOTES_FIXTURE_WINDOW_ID,
            app_name: "Notes".into(),
            title: "Notes – Aucune note".into(),
        };
        Self::from_json(json, window)
    }
}

impl Perceptor for MockPerceptor {
    fn capture(&self, _target: &Target) -> Result<Vec<RawAxNode>> {
        Ok(self.roots.clone())
    }

    fn window_ref(&self, _target: &Target) -> Result<WindowRef> {
        Ok(self.window.clone())
    }
}

/// An [`ActionExecutor`] that records calls instead of touching the OS. Useful
/// for asserting that the MCP server resolved and gated an action correctly.
#[derive(Default)]
pub struct RecordingExecutor {
    /// Recorded calls as `(node id, action, argument)` tuples.
    pub calls: Mutex<Vec<(String, SemanticAction, Option<String>)>>,
}

impl ActionExecutor for RecordingExecutor {
    fn perform(
        &self,
        _target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((node.id.clone(), action, argument.map(str::to_owned)));
        Ok(())
    }
}
