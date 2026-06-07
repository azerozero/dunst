//! Tiny, dependency-free text utilities shared by ID synthesis and the risk
//! engine: ASCII lowercasing with French/Latin accent folding.

/// Lowercase `s` and fold common Latin accents onto their ASCII base letter.
/// Deterministic and allocation-light (POC scope: French + frequent
/// diacritics). Non-letter characters are preserved so risk matching keeps
/// word boundaries (`"forcer à quitter"` -> `"forcer a quitter"`).
pub(crate) fn normalize(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).map(deaccent).collect()
}

/// Map a single (already-lowercased) character to its accent-stripped form.
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
