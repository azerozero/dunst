//! Shared contracts for Dunst MCP.
//!
//! This crate is the *frozen interface* every other crate builds against:
//!
//! - [`types`] — the data model (raw AX nodes, scene graph, affordances, risk, audit).
//! - [`traits`] — the boundaries: [`Perceptor`] (pixels/AX -> raw nodes)
//!   and [`ActionExecutor`] (semantic action -> OS event).
//! - [`mock`] — a [`MockPerceptor`](mock::MockPerceptor) that replays a captured AX tree
//!   from JSON, so the pure-logic crate (`dunst-graph`) can be built and tested with
//!   **zero macOS dependency**.
//!
//! Pipeline (see `docs/ARCHITECTURE.md`):
//! `Perceptor -> RawAxNode tree -> SceneGraph -> AffordanceGraph -> Risk -> MCP`.

pub mod mock;
pub mod traits;
pub mod types;

pub use traits::{ActionExecutor, Perceptor, Target};
pub use types::*;

/// Milliseconds since the Unix epoch. Single clock source for `captured_at_ms`,
/// `last_seen_ms`, freshness and audit timestamps.
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        // NOTE: A clock set before the Unix epoch yields `0` here, which would
        // inflate every downstream `freshness_ms`. That only happens with a
        // grossly-misconfigured system clock; a saturating `0` is a safe, inert
        // floor rather than a panic.
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Crate-wide error type. Kept deliberately small for the POC.
#[derive(Debug, thiserror::Error)]
pub enum DunstError {
    /// No element matched the requested ID.
    #[error("element not found: {0}")]
    ElementNotFound(String),
    /// The requested action is not offered by the target element.
    #[error("action {action} not available on element {id}")]
    ActionUnavailable {
        /// ID of the element the action was requested on.
        id: String,
        /// Name of the unavailable action.
        action: String,
    },
    /// The action is gated behind operator approval by the risk engine.
    #[error("action {action} on {id} requires approval (risk={risk})")]
    ApprovalRequired {
        /// ID of the element the action was requested on.
        id: String,
        /// Name of the action requiring approval.
        action: String,
        /// Assessed risk level that triggered the gate.
        risk: String,
    },
    /// The perception backend failed to capture the UI.
    #[error("perception backend failed: {0}")]
    Perception(String),
    /// The executor failed to perform the action against the OS.
    #[error("action execution failed: {0}")]
    Execution(String),
    /// A JSON (de)serialization error, forwarded from [`serde_json`].
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    /// An I/O error, forwarded from [`std::io`].
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Crate-wide result type aliasing [`DunstError`] as the error.
pub type Result<T> = std::result::Result<T, DunstError>;
