use super::*;

pub(super) fn likely_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }
    if trimmed.starts_with("www.") && trimmed.contains('.') {
        return Some(format!("https://{trimmed}"));
    }
    None
}

pub(super) fn push_unique_string(out: &mut Vec<String>, value: &str, limit: usize) {
    if out.len() >= limit || out.iter().any(|existing| existing == value) {
        return;
    }
    out.push(value.to_string());
}

pub(super) fn push_unique_action(out: &mut Vec<SemanticAction>, action: SemanticAction) {
    if !out.contains(&action) {
        out.push(action);
    }
}

pub(super) fn canonical_file_path(path: &str) -> dunst_core::Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(VisualOpsError::Execution(
            "select_file requires a non-empty path".into(),
        ));
    }
    let path = Path::new(trimmed);
    let canonical = path
        .canonicalize()
        .map_err(|e| VisualOpsError::Execution(format!("file {trimmed:?} not accessible: {e}")))?;
    if !canonical.is_file() {
        return Err(VisualOpsError::Execution(format!(
            "path {:?} is not a file",
            canonical.display()
        )));
    }
    Ok(canonical)
}

pub(super) fn off_target_raw_allowed() -> bool {
    std::env::var("DUNST_MCP_ALLOW_OFF_TARGET_RAW")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

pub(super) fn normalize_match(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => normalized.push('a'),
            'ç' => normalized.push('c'),
            'è' | 'é' | 'ê' | 'ë' => normalized.push('e'),
            'ì' | 'í' | 'î' | 'ï' => normalized.push('i'),
            'ñ' => normalized.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => normalized.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => normalized.push('u'),
            'ý' | 'ÿ' => normalized.push('y'),
            'æ' => normalized.push_str("ae"),
            'œ' => normalized.push_str("oe"),
            other => normalized.push(other),
        }
    }
    normalized
}

pub(super) fn normalized_contains_query(haystack: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if !haystack.contains(query) {
        return false;
    }
    if !query.chars().all(|ch| ch.is_ascii_alphanumeric()) || query.chars().count() < 4 {
        return true;
    }
    normalized_contains_word(haystack, query)
}

pub(super) fn normalized_contains_word(haystack: &str, query: &str) -> bool {
    let h: Vec<char> = haystack.chars().collect();
    let q: Vec<char> = query.chars().collect();
    if q.is_empty() || q.len() > h.len() {
        return false;
    }
    for start in 0..=h.len() - q.len() {
        if h[start..start + q.len()] != q[..] {
            continue;
        }
        let before = start == 0 || !h[start - 1].is_ascii_alphanumeric();
        let after = start + q.len() == h.len() || !h[start + q.len()].is_ascii_alphanumeric();
        if before && after {
            return true;
        }
    }
    false
}

pub(super) fn retry_user_active_guard<T, F>(f: F) -> dunst_core::Result<T>
where
    F: FnMut() -> dunst_core::Result<T>,
{
    retry_user_active_guard_after(Duration::from_millis(400), f)
}

pub(super) fn retry_user_active_guard_after<T, F>(
    delay: Duration,
    mut f: F,
) -> dunst_core::Result<T>
where
    F: FnMut() -> dunst_core::Result<T>,
{
    let mut next_delay = delay;
    let mut last_guard_err = None;
    for _ in 0..4 {
        match f() {
            Err(err) if is_user_active_guard_error(&err) => {
                last_guard_err = Some(err);
                std::thread::sleep(next_delay);
                next_delay += delay;
            }
            other => return other,
        }
    }
    Err(last_guard_err.expect("guard retry loop stores the last guard error"))
}

pub(super) fn is_user_active_guard_error(err: &VisualOpsError) -> bool {
    err.to_string().contains("user-active guard blocked")
}

pub(super) fn is_element_not_found(err: &VisualOpsError) -> bool {
    matches!(err, VisualOpsError::ElementNotFound(_))
}

pub(super) fn is_terminal_app_name(value: &str) -> bool {
    let app = normalize_match(value);
    [
        "iterm",
        "iterm2",
        "terminal",
        "wezterm",
        "ghostty",
        "alacritty",
        "kitty",
        "warp",
    ]
    .iter()
    .any(|needle| app.contains(needle))
}
