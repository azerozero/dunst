//! Shared contracts for VisualOps MCP.
//!
//! This crate is the *frozen interface* every other crate builds against:
//!
//! - [`types`] — the data model (raw AX nodes, scene graph, affordances, risk, audit).
//! - [`traits`] — the boundaries: [`Perceptor`](traits::Perceptor) (pixels/AX -> raw nodes)
//!   and [`ActionExecutor`](traits::ActionExecutor) (semantic action -> OS event).
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
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Crate-wide error type. Kept deliberately small for the POC.
#[derive(Debug, thiserror::Error)]
pub enum VisualOpsError {
    #[error("element not found: {0}")]
    ElementNotFound(String),
    #[error("action {action} not available on element {id}")]
    ActionUnavailable { id: String, action: String },
    #[error("action {action} on {id} requires approval (risk={risk})")]
    ApprovalRequired {
        id: String,
        action: String,
        risk: String,
    },
    #[error("perception backend failed: {0}")]
    Perception(String),
    #[error("action execution failed: {0}")]
    Execution(String),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, VisualOpsError>;
