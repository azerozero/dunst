//! Tiny, dependency-free text utilities shared by ID synthesis and the risk
//! engine: ASCII lowercasing with French/Latin accent folding.

/// Lowercase `s` and fold accents onto their ASCII base letter. Handles **both**
/// precomposed (`é`, NFC) and decomposed (`e` + `U+0301`, NFD) forms:
///
/// 1. lowercase,
/// 2. drop Unicode combining diacritical marks (`U+0300..=U+036F`) — this folds
///    NFD-decomposed accents (`"e\u{301}teindre"` -> `"eteindre"`),
/// 3. fold the remaining precomposed chars via [`deaccent`] (NFC fallback).
///
/// Deterministic and allocation-light (POC scope: French + frequent
/// diacritics). Non-letter characters are preserved so risk matching keeps word
/// boundaries (`"forcer à quitter"` -> `"forcer a quitter"`).
pub(crate) fn normalize(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !is_combining_mark(*c))
        .map(deaccent)
        .collect()
}

/// Unicode combining diacritical marks left behind by NFD decomposition. Dropping
/// them folds a decomposed accent onto its base letter (`e` + `U+0301` -> `e`).
fn is_combining_mark(c: char) -> bool {
    ('\u{0300}'..='\u{036F}').contains(&c)
}

/// Map a single (already-lowercased) precomposed char to its accent-stripped
/// form. Fallback for NFC input that never went through NFD decomposition.
fn deaccent(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
        'ç' => 'c',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'ñ' => 'n',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
        'û' | 'ü' | 'ú' | 'ù' => 'u',
        'ÿ' | 'ý' => 'y',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn folds_precomposed_nfc() {
        assert_eq!(normalize("Éteindre"), "eteindre");
        assert_eq!(normalize("Réinitialiser"), "reinitialiser");
        assert_eq!(normalize("Forcer à quitter"), "forcer a quitter");
    }

    #[test]
    fn folds_decomposed_nfd() {
        // base letter + combining acute accent (U+0301)
        assert_eq!(normalize("E\u{301}teindre"), "eteindre");
        assert_eq!(normalize("Re\u{301}initialiser"), "reinitialiser");
        // NFC and NFD forms normalise identically
        assert_eq!(normalize("E\u{301}teindre"), normalize("Éteindre"));
    }
}
