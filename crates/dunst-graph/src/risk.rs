//! Label/heuristic-based Risk Engine.
//!
//! For the POC, risk is derived from the element's label/help/identifier text
//! against keyword tiers. See WP-B for the keyword lists.
//!
//! - HIGH (`requires_approval = true`): destructive / irreversible **or a
//!   financial / external commit** — `supprimer`, `delete`, `effacer`,
//!   `éteindre`, `redémarrer`, `forcer à quitter`, `réinitialiser`, `remove`,
//!   `shut down`, `révoquer`/`revoke`, `écraser`/`overwrite`, `payer`/`pay`,
//!   `commander`/`checkout`, ... These block until the operator approves the
//!   exact gated id.
//! - MEDIUM: state-changing but recoverable — `envoyer`, `send`, `publier`,
//!   `deploy`, `enregistrer`, `coller`, `move`, `submit`, `apply`, ...
//! - LOW: everything else (navigation, reads, hovers).
//!
//! This is a keyword denylist, so it is heuristic by construction: a genuinely
//! novel destructive label still reads LOW. The list is kept broad (below) to
//! shrink that fail-open surface, and the whole write path stays bounded by
//! `approve` being disabled by default (see `docs/CONTRACTS.md`). A structural
//! "gate every unknown verb" posture is not viable here because the signal is the
//! element's own label text — most benign controls (icon-only toolbar buttons,
//! `OK`, `Nouvelle note`) carry no keyword and must stay LOW to remain usable.

use dunst_core::{RiskAssessment, RiskLevel, SceneNode};

use crate::text::normalize;

/// Original keyword lists (HIGH / MEDIUM), kept as `&'static str` so the
/// `reasons` strings can report the human-readable form.
const HIGH_KEYWORDS: &[&str] = &[
    // Destructive / irreversible (base words also cover their multi-word forms,
    // e.g. "delete" matches "delete permanently", "reset" matches "factory reset").
    "supprimer",
    "delete",
    "effacer",
    "remove",
    "détruire",
    "destroy",
    "écraser",
    "overwrite",
    "éteindre",
    "shut down",
    "redémarrer",
    "restart",
    "forcer à quitter",
    "force quit",
    "forcer l'arrêt",
    "kill",
    "terminate",
    "réinitialiser",
    "reset",
    "déconnexion",
    "log out",
    "formater",
    "erase",
    "vider",
    "empty trash",
    "purger",
    "purge",
    "wipe",
    "révoquer",
    "revoke",
    "dépublier",
    "unpublish",
    "désinstaller",
    "uninstall",
    "discard",
    "ne pas enregistrer",
    "don't save",
    // Financial / external commits: irreversible side effects an agent must not
    // trigger on its own (the food-ordering / checkout flows this POC drives).
    "payer",
    "pay",
    "acheter",
    "buy",
    "purchase",
    "checkout",
    "commander",
    "passer la commande",
    "place order",
    "résilier",
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
    "soumettre",
    "submit",
    "confirmer",
    "confirm",
    "appliquer",
    "apply",
    "téléverser",
    "upload",
    "installer",
    "install",
    "dupliquer",
    "duplicate",
    "importer",
    "import",
    "exporter",
    "export",
    "commit",
];

/// A keyword compiled once: `needle` is the normalised form **pre-collected into
/// chars** so matching allocates nothing per node; `original` is the
/// human-readable form used in `reasons`.
#[derive(Debug, Clone)]
struct Keyword {
    needle: Vec<char>,
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
    /// Builds a risk engine with the compiled high- and medium-risk keyword sets.
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
        // Collect the haystack chars once here; the keyword loops below borrow it,
        // instead of re-collecting per keyword (previously ~N_keywords allocations
        // per node per refresh).
        let hay: Vec<char> = normalize(text).chars().collect();
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
            needle: normalize(kw).chars().collect(),
            original: kw,
        })
        .collect()
}

/// Collect `"matched keyword: <kw>"` for every keyword (in table order) whose
/// pre-normalised needle appears on token boundaries in the normalised haystack.
/// Returns `None` when nothing matched. No per-keyword normalisation/allocation
/// here.
fn match_tier(hay: &[char], keywords: &[Keyword]) -> Option<Vec<String>> {
    let reasons: Vec<String> = keywords
        .iter()
        .filter(|kw| contains_keyword(hay, &kw.needle))
        .map(|kw| format!("matched keyword: {}", kw.original))
        .collect();
    if reasons.is_empty() {
        None
    } else {
        Some(reasons)
    }
}

fn contains_keyword(h: &[char], n: &[char]) -> bool {
    if n.is_empty() {
        return true;
    }
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

    #[test]
    fn broadened_denylist_gates_destructive_and_financial_commits() {
        let engine = RiskEngine::new();
        for (text, kw) in [
            ("Payer maintenant", "payer"),
            ("Checkout", "checkout"),
            ("Passer la commande", "passer la commande"),
            ("Révoquer l'accès", "révoquer"),
            ("Overwrite file", "overwrite"),
            ("Uninstall app", "uninstall"),
            ("Ne pas enregistrer", "ne pas enregistrer"),
            ("Purge cache", "purge"),
        ] {
            let risk = engine.assess_text(text);
            assert_eq!(risk.level, RiskLevel::High, "{text} should gate");
            assert!(risk.requires_approval, "{text} should require approval");
            assert!(
                risk.reasons.iter().any(|r| r.contains(kw)),
                "{text}: expected reason for {kw}, got {:?}",
                risk.reasons
            );
        }
    }

    #[test]
    fn broadened_denylist_avoids_false_positives_on_lookalike_words() {
        let engine = RiskEngine::new();
        // Each embeds a keyword as a substring that the word-boundary check must
        // reject (payment⊃pay, buyer⊃buy, committee⊃commit, important⊃import,
        // wipers⊃wipe, applying⊃apply, purchases⊃purchase).
        for text in [
            "payment methods",
            "buyer profile",
            "committee meeting notes",
            "important reminder",
            "windshield wipers",
            "applying filters",
            "your recent purchases",
            "display settings",
        ] {
            let risk = engine.assess_text(text);
            assert_eq!(risk.level, RiskLevel::Low, "{text} should stay LOW");
        }
    }
}
