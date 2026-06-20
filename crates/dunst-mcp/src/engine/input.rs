/// Parse a hotkey combo like `"cmd+l"` into `(modifier flags, keycode)`.
pub(super) fn parse_combo(combo: &str) -> Option<(u64, u16)> {
    let mut flags = 0u64;
    let mut key = None;
    for part in combo.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" => flags |= 0x0010_0000,
            "shift" => flags |= 0x0002_0000,
            "opt" | "option" | "alt" => flags |= 0x0008_0000,
            "ctrl" | "control" => flags |= 0x0004_0000,
            other => key = keycode_for(other),
        }
    }
    Some((flags, key?))
}

pub(super) fn layout_sensitive_hotkey_message(combo: &str) -> Option<String> {
    let mut has_cmd = false;
    let mut has_non_cmd_modifier = false;
    let mut key = None;

    for part in combo.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" => has_cmd = true,
            "shift" | "opt" | "option" | "alt" | "ctrl" | "control" => has_non_cmd_modifier = true,
            other => key = Some(other.to_string()),
        }
    }

    match (has_cmd, has_non_cmd_modifier, key.as_deref()) {
        (true, false, Some("a")) => Some(
            "hotkey \"cmd+a\" is keyboard-layout sensitive on macOS and can hit the wrong Command shortcut on non-US layouts; use type_into on a text element instead"
                .into(),
        ),
        _ => None,
    }
}

/// macOS virtual keycode for a key name or single character (US ANSI layout).
fn keycode_for(k: &str) -> Option<u16> {
    Some(match k {
        "enter" | "return" => 0x24,
        "tab" => 0x30,
        "escape" | "esc" => 0x35,
        "space" => 0x31,
        "delete" | "backspace" => 0x33,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        "pagedown" => 0x79,
        "pageup" => 0x74,
        "home" => 0x73,
        "end" => 0x77,
        "plus" => 0x18,
        "minus" => 0x1B,
        s if s.chars().count() == 1 => char_keycode(s.chars().next()?)?,
        _ => return None,
    })
}

pub(super) fn is_press_key_name(key: &str) -> bool {
    matches!(
        key.trim().to_ascii_lowercase().as_str(),
        "return"
            | "enter"
            | "tab"
            | "escape"
            | "esc"
            | "space"
            | "spacebar"
            | "delete"
            | "backspace"
            | "up"
            | "arrowup"
            | "up_arrow"
            | "down"
            | "arrowdown"
            | "down_arrow"
            | "left"
            | "arrowleft"
            | "left_arrow"
            | "right"
            | "arrowright"
            | "right_arrow"
            | "pageup"
            | "page_up"
            | "pagedown"
            | "page_down"
            | "home"
            | "end"
    )
}

/// macOS virtual keycode for a single character (US ANSI layout).
pub(super) fn char_keycode(c: char) -> Option<u16> {
    Some(match c.to_ascii_lowercase() {
        'a' => 0x00,
        'b' => 0x0B,
        'c' => 0x08,
        'd' => 0x02,
        'e' => 0x0E,
        'f' => 0x03,
        'g' => 0x05,
        'h' => 0x04,
        'i' => 0x22,
        'j' => 0x26,
        'k' => 0x28,
        'l' => 0x25,
        'm' => 0x2E,
        'n' => 0x2D,
        'o' => 0x1F,
        'p' => 0x23,
        'q' => 0x0C,
        'r' => 0x0F,
        's' => 0x01,
        't' => 0x11,
        'u' => 0x20,
        'v' => 0x09,
        'w' => 0x0D,
        'x' => 0x07,
        'y' => 0x10,
        'z' => 0x06,
        '0' => 0x1D,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '5' => 0x17,
        '6' => 0x16,
        '7' => 0x1A,
        '8' => 0x1C,
        '9' => 0x19,
        '=' => 0x18,
        '-' => 0x1B,
        _ => return None,
    })
}
