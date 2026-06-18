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

use dunst_core::{RiskAssessment, RiskLevel, SceneNode};

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
/// pre-normalised needle appears on token boundaries in the normalised haystack.
/// Returns `None` when nothing matched. No per-keyword normalisation/allocation
/// here.
fn match_tier(hay: &str, keywords: &[Keyword]) -> Option<Vec<String>> {
    let reasons: Vec<String> = keywords
        .iter()
        .filter(|kw| contains_keyword(hay, kw.needle.as_str()))
        .map(|kw| format!("matched keyword: {}", kw.original))
        .collect();
    if reasons.is_empty() {
        None
    } else {
        Some(reasons)
    }
}

fn contains_keyword(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.len() > h.len() {
        return false;
    }
    for start in 0..=h.len() - n.len() {
        if h[start..start + n.len()] != n[..] {
            continue;
        }
        let before = start == 0 || !risk_word_char(h[start - 1]);
        let after = start + n.len() == h.len() || !risk_word_char(h[start + n.len()]);
        if before && after {
            return true;
        }
    }
    false
}

fn risk_word_char(ch: char) -> bool {
    ch.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use dunst_core::RiskLevel;

    use super::RiskEngine;

    #[test]
    fn typed_payload_risk_matches_destructive_words_on_boundaries() {
        let engine = RiskEngine::new();

        let risk = engine.assess_text("merci de vider le cache");
        assert_eq!(risk.level, RiskLevel::High);
        assert!(risk.requires_approval);
        assert!(risk.reasons.iter().any(|r| r.contains("vider")));
    }

    #[test]
    fn typed_payload_risk_does_not_match_keyword_inside_larger_word() {
        let engine = RiskEngine::new();

        for text in [
            "failover multi-provider",
            "provider fallback",
            "preset configuration",
            "sauvegarder dans le clipboard",
        ] {
            let risk = engine.assess_text(text);
            assert_ne!(risk.level, RiskLevel::High, "{text}");
            assert!(
                !risk
                    .reasons
                    .iter()
                    .any(|r| r.contains("vider") || r.contains("reset")),
                "{text}: {:?}",
                risk.reasons
            );
        }
    }

    #[test]
    fn typed_payload_risk_still_matches_multi_word_keywords() {
        let engine = RiskEngine::new();

        let risk = engine.assess_text("empty trash now");
        assert_eq!(risk.level, RiskLevel::High);
        assert!(risk.reasons.iter().any(|r| r.contains("empty trash")));

        let risk = engine.assess_text("forcer a quitter Firefox");
        assert_eq!(risk.level, RiskLevel::High);
        assert!(risk.reasons.iter().any(|r| r.contains("forcer à quitter")));
    }
}
