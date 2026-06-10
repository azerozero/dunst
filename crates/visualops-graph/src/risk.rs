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

/// Original keyword lists (HIGH / MEDIUM), kept as `&'static str` so the
/// `reasons` strings can report the human-readable form.
const HIGH_KEYWORDS: &[&str] = &[
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
];

const MEDIUM_KEYWORDS: &[&str] = &[
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
];

/// A keyword compiled once: the `needle` is the normalised form matched against
/// element text; `original` is the human-readable form used in `reasons`.
#[derive(Debug, Clone)]
struct Keyword {
    needle: String,
    original: &'static str,
}

/// Stateful so later phases can add policy/history; for the POC it is two
/// keyword tables matched (accent-insensitively) against element text. The
/// tables are **pre-normalised once** here (G5) so `assess` does no per-keyword
/// allocation per node.
#[derive(Debug, Clone)]
pub struct RiskEngine {
    high: Vec<Keyword>,
    medium: Vec<Keyword>,
}

impl Default for RiskEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskEngine {
    pub fn new() -> Self {
        Self {
            high: compile(HIGH_KEYWORDS),
            medium: compile(MEDIUM_KEYWORDS),
        }
    }

    /// Assess one node. Combines label, help and ax_identifier text and runs it
    /// through [`assess_text`](Self::assess_text). Highest tier wins; `reasons`
    /// lists every matched keyword at that tier.
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
        self.assess_text(&haystack)
    }

    /// Assess arbitrary text against the same keyword tiers — e.g. the payload an
    /// agent wants to type. Lets a destructive *value* raise the gate even when the
    /// target field is itself low-risk (audit #13). Normalises (lowercase +
    /// accent-fold) then matches high, then medium; highest tier wins.
    pub fn assess_text(&self, text: &str) -> RiskAssessment {
        // Keyword matching is unbounded substring containment (see `match_tier`),
        // so it can over-match a keyword embedded in a larger token ("reset" in
        // "preset"). That direction is **fail-safe for a risk gate**: over-matching
        // only ever asks for *more* approval, never less — a destructive word can't
        // be *missed* by substring search. If false-positive gating gets noisy,
        // switch to word-boundary matching on the normalised haystack.
        let hay = normalize(text);
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

/// Pre-normalise a keyword table once (G5): store the normalised needle next to
/// the original text used for `reasons`.
fn compile(keywords: &[&'static str]) -> Vec<Keyword> {
    keywords
        .iter()
        .map(|&kw| Keyword {
            needle: normalize(kw),
            original: kw,
        })
        .collect()
}

/// Collect `"matched keyword: <kw>"` for every keyword (in table order) whose
/// pre-normalised needle appears in the normalised haystack. Returns `None` when
/// nothing matched. No per-keyword normalisation/allocation here.
fn match_tier(hay: &str, keywords: &[Keyword]) -> Option<Vec<String>> {
    let reasons: Vec<String> = keywords
        .iter()
        .filter(|kw| hay.contains(kw.needle.as_str()))
        .map(|kw| format!("matched keyword: {}", kw.original))
        .collect();
    if reasons.is_empty() {
        None
    } else {
        Some(reasons)
    }
}
