//! Pure-logic layer: raw AX nodes -> Scene Graph -> Affordance Graph + Risk,
//! and scene-graph diffing for the audit trail.
//!
//! **No macOS dependency.** Everything here is a deterministic function of
//! [`visualops_core`] types and is unit-tested against `MockPerceptor`
//! fixtures. See `docs/WP-B-graph.md` for the full spec and done-criteria.

pub mod affordance;
pub mod audit;
pub mod risk;
pub mod scene;

pub use affordance::derive_affordances;
pub use audit::diff;
pub use risk::RiskEngine;
pub use scene::build_scene_graph;
