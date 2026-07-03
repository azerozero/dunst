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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MenuHotkeyCombo {
    pub(super) cmd_char: char,
    pub(super) cmd_virtual_key: Option<u16>,
    pub(super) command: bool,
    pub(super) shift: bool,
    pub(super) option: bool,
    pub(super) control: bool,
}

pub(super) fn parse_menu_hotkey_combo(combo: &str) -> Option<MenuHotkeyCombo> {
    let mut parsed = MenuHotkeyCombo {
        cmd_char: '\0',
        cmd_virtual_key: None,
        command: false,
        shift: false,
        option: false,
        control: false,
    };
    let mut key = None;
    let mut virtual_key = None;
    for part in combo.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" => parsed.command = true,
            "shift" => parsed.shift = true,
            "opt" | "option" | "alt" => parsed.option = true,
            "ctrl" | "control" => parsed.control = true,
            other => match menu_cmd_char_for_key_name(other) {
                Some(ch) => {
                    key = Some(ch);
                    virtual_key = None;
                }
                None => {
                    key = None;
                    virtual_key = menu_virtual_key_for_key_name(other);
                }
            },
        }
    }
    if let Some(ch) = key {
        parsed.cmd_char = ch;
        return Some(parsed);
    }
    parsed.cmd_virtual_key = Some(virtual_key?);
    Some(parsed)
}

pub(super) fn menu_hotkey_matches(
    combo: &MenuHotkeyCombo,
    cmd_char: &str,
    cmd_modifiers: Option<u64>,
) -> bool {
    if combo.cmd_virtual_key.is_some() {
        return false;
    }
    let Some(item_char) = cmd_char.chars().find(|ch| !ch.is_whitespace()) else {
        return false;
    };
    if !item_char.eq_ignore_ascii_case(&combo.cmd_char) {
        return false;
    }
    menu_modifiers_ok(combo, cmd_modifiers)
}

pub(super) fn menu_hotkey_matches_virtual_key(
    combo: &MenuHotkeyCombo,
    virtual_key: u16,
    cmd_modifiers: Option<u64>,
) -> bool {
    combo.cmd_virtual_key == Some(virtual_key) && menu_modifiers_ok(combo, cmd_modifiers)
}

fn menu_modifiers_ok(combo: &MenuHotkeyCombo, cmd_modifiers: Option<u64>) -> bool {
    match cmd_modifiers {
        Some(modifiers) => ax_menu_modifiers_match(combo, modifiers),
        None => combo.command && !combo.shift && !combo.option && !combo.control,
    }
}

fn menu_cmd_char_for_key_name(key: &str) -> Option<char> {
    match key {
        "space" | "spacebar" => Some(' '),
        "plus" => Some('+'),
        "minus" => Some('-'),
        s if s.chars().count() == 1 => s.chars().next().map(|ch| ch.to_ascii_lowercase()),
        _ => None,
    }
}

/// Virtual keys for named non-character keys, matched against
/// AXMenuItemCmdVirtualKey (items whose shortcut has no CmdChar, e.g. arrows).
/// Only these names are safe to resolve by keycode: their physical position
/// does not depend on the keyboard layout, contrary to letter keys which must
/// stay on AXMenuItemCmdChar (AZERTY, cf. BUGS-TODO section 0).
fn menu_virtual_key_for_key_name(key: &str) -> Option<u16> {
    Some(match key {
        "enter" | "return" => 0x24,
        "tab" => 0x30,
        "escape" | "esc" => 0x35,
        "delete" | "backspace" => 0x33,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        "pagedown" => 0x79,
        "pageup" => 0x74,
        "home" => 0x73,
        "end" => 0x77,
        _ => return None,
    })
}

fn ax_menu_modifiers_match(combo: &MenuHotkeyCombo, modifiers: u64) -> bool {
    const AX_SHIFT: u64 = 1;
    const AX_OPTION: u64 = 2;
    const AX_CONTROL: u64 = 4;
    const AX_NO_COMMAND: u64 = 8;

    let ax_style = modifiers & !(AX_SHIFT | AX_OPTION | AX_CONTROL | AX_NO_COMMAND) == 0;
    if ax_style {
        return combo.shift == (modifiers & AX_SHIFT != 0)
            && combo.option == (modifiers & AX_OPTION != 0)
            && combo.control == (modifiers & AX_CONTROL != 0)
            && combo.command == (modifiers & AX_NO_COMMAND == 0);
    }

    combo.shift == (modifiers & 0x0002_0000 != 0)
        && combo.option == (modifiers & 0x0008_0000 != 0)
        && combo.control == (modifiers & 0x0004_0000 != 0)
        && combo.command == (modifiers & 0x0010_0000 != 0)
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
