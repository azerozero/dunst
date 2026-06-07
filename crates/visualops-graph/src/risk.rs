//! Label/heuristic-based Risk Engine.
//!
//! For the POC, risk is derived from the element's label/help text against
//! keyword tiers, plus role/identifier hints. See WP-B for the keyword lists.
//!
//! - HIGH (`requires_approval = true`): destructive / irreversible —
//!   `supprimer`, `delete`, `effacer`, `éteindre`, `redémarrer`,
//!   `forcer à quitter`, `réinitialiser`, `remove`, `shut down`, ...
//! - MEDIUM: state-changing but recoverable — `envoyer`, `send`, `publier`,
//!   `deploy`, `enregistrer`, `coller`, `move`, ...
//! - LOW: everything else (navigation, reads, hovers).

use visualops_core::{RiskAssessment, SceneNode};

/// Stateful so later phases can add policy/history; for the POC it is keyword tables.
#[derive(Debug, Clone)]
pub struct RiskEngine {
    // WP-B: keyword tables (high/medium), compiled once in `new`.
    _private: (),
}

impl Default for RiskEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskEngine {
    pub fn new() -> Self {
        todo!("WP-B: build keyword tables")
    }

    /// Assess one node. Combines label, help and ax_identifier text.
    pub fn assess(&self, node: &SceneNode) -> RiskAssessment {
        let _ = node;
        todo!("WP-B: keyword match -> level + requires_approval + reasons")
    }
}
