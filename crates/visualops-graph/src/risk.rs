//! Label/heuristic-based Risk Engine.
//!
//! For the POC, risk is derived from the element's label/help/identifier text
//! against keyword tiers. See WP-B for the keyword lists.
//!
//! - HIGH (`requires_approval = true`): destructive / irreversible —
//!   `supprimer`, `delete`, `effacer`, `éteindre`, `redémarrer`,
//!   `forcer à quitter`, `réinitialiser`, `remove`, `shut down`, ...
//! - MEDIUM: state-changing but recoverable — `envoyer`, `send`, `publier`,
//!   `deploy`, `enregistrer`, `coller`, `move`, ...
//! - LOW: everything else (navigation, reads, hovers).

use visualops_core::{RiskAssessment, RiskLevel, SceneNode};

use crate::text::normalize;

/// Stateful so later phases can add policy/history; for the POC it is two
/// keyword tables matched (accent-insensitively) against element text.
#[derive(Debug, Clone)]
pub struct RiskEngine {
    high: Vec<&'static str>,
    medium: Vec<&'static str>,
}

impl Default for RiskEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskEngine {
    pub fn new() -> Self {
        Self {
            high: vec![
                "supprimer",
                "delete",
                "effacer",
                "remove",
                "éteindre",
                "shut down",
                "redémarrer",
                "restart",
                "forcer à quitter",
                "force quit",
                "réinitialiser",
                "reset",
                "déconnexion",
                "log out",
                "formater",
                "erase",
                "vider",
                "empty trash",
            ],
            medium: vec![
                "envoyer",
                "send",
                "publier",
                "publish",
                "deploy",
                "déployer",
                "enregistrer",
                "save",
                "coller",
                "paste",
                "déplacer",
                "move",
                "renommer",
                "rename",
                "partager",
                "share",
                "archiver",
            ],
        }
    }

    /// Assess one node. Combines label, help and ax_identifier text, normalises
    /// it (lowercase + accent-fold), and matches against the keyword tiers.
    /// Highest tier wins; `reasons` lists every matched keyword at that tier.
    pub fn assess(&self, node: &SceneNode) -> RiskAssessment {
        let mut haystack = String::new();
        if let Some(label) = &node.label {
            haystack.push_str(label);
            haystack.push(' ');
        }
        if let Some(help) = &node.help {
            haystack.push_str(help);
            haystack.push(' ');
        }
        if let Some(ident) = &node.ax_identifier {
            haystack.push_str(ident);
        }
        let hay = normalize(&haystack);

        if let Some(reasons) = match_tier(&hay, &self.high) {
            return RiskAssessment {
                level: RiskLevel::High,
                requires_approval: true,
                reasons,
            };
        }
        if let Some(reasons) = match_tier(&hay, &self.medium) {
            return RiskAssessment {
                level: RiskLevel::Medium,
                requires_approval: false,
                reasons,
            };
        }
        RiskAssessment::low()
    }
}

/// Collect `"matched keyword: <kw>"` for every keyword (in table order) that
/// appears in the normalised haystack. Returns `None` when nothing matched.
fn match_tier(hay: &str, keywords: &[&str]) -> Option<Vec<String>> {
    let reasons: Vec<String> = keywords
        .iter()
        .filter(|kw| hay.contains(&normalize(kw)))
        .map(|kw| format!("matched keyword: {kw}"))
        .collect();
    if reasons.is_empty() {
        None
    } else {
        Some(reasons)
    }
}
