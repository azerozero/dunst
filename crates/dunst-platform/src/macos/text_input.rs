use super::*;

pub(super) fn type_text(
    element: &AxElement,
    target: &Target,
    text: &str,
) -> std::result::Result<(), ActionFailure> {
    if let Some(outcome) = type_text_by_replacing_selection(element, target, text)? {
        return outcome;
    }

    if attr_settable(element, kAXValueAttribute) {
        let before = attr_string(element, kAXValueAttribute);
        let err = set_string_attr_raw(element, kAXValueAttribute, text);
        if err == kAXErrorSuccess {
            match wait_for_string_attr(element, kAXValueAttribute, text) {
                Some(value) if value == text => return Ok(()),
                Some(value) if before.as_deref() != Some(value.as_str()) => {
                    return Err(ActionFailure::Execution(format!(
                        "AX set-value changed the field but did not produce the requested value: expected {} chars, observed {} chars",
                        text.chars().count(),
                        value.chars().count()
                    )));
                }
                Some(_) => {}
                None => return Ok(()),
            }
            return Err(ActionFailure::Execution(
                "AX set-value reported success but the field did not change; keyboard fallback suppressed to avoid appending text".into(),
            ));
        } else if is_stale_ax_error(err) {
            return Err(ActionFailure::Ax {
                operation: "set AX string attribute",
                err,
            });
        }
    }

    // AX set-value replaces text; synthetic Unicode keystrokes append to the focused editor.
    set_bool_attr(element, kAXFocusedAttribute, true)?;
    std::thread::sleep(std::time::Duration::from_millis(80));
    post_window_bound_text(target, text)
}

pub(super) fn type_text_by_replacing_selection(
    element: &AxElement,
    target: &Target,
    text: &str,
) -> std::result::Result<Option<std::result::Result<(), ActionFailure>>, ActionFailure> {
    let Some(len) = text_character_count(element) else {
        return Ok(None);
    };
    if !attr_settable(element, kAXSelectedTextRangeAttribute) {
        return Ok(None);
    }

    let before = attr_string(element, kAXValueAttribute);
    set_bool_attr(element, kAXFocusedAttribute, true)?;
    let range = CFRange::init(0, len);
    let err = set_axvalue_attr_raw(
        element,
        kAXSelectedTextRangeAttribute,
        kAXValueTypeCFRange,
        (&range as *const CFRange).cast(),
    );
    if err == kAXErrorIllegalArgument || err == kAXErrorNoValue {
        return Ok(None);
    }
    if err != kAXErrorSuccess {
        return Err(ActionFailure::Ax {
            operation: "set AX selected text range",
            err,
        });
    }

    std::thread::sleep(std::time::Duration::from_millis(80));
    post_window_bound_text(target, text)?;
    Ok(Some(match wait_for_string_attr(element, kAXValueAttribute, text) {
        Some(value) if value == text => Ok(()),
        Some(value)
            if before
                .as_deref()
                .map(|before| value == format!("{before}{text}") || value == format!("{text}{before}"))
                .unwrap_or(false) =>
        {
            Err(ActionFailure::Execution(
                "keyboard replacement appended instead of replacing; selected text range was not honored".into(),
            ))
        }
        Some(value) if before.as_deref() != Some(value.as_str()) => {
            Err(ActionFailure::Execution(format!(
                "keyboard replacement changed the field but did not produce the requested value: expected {} chars, observed {} chars",
                text.chars().count(),
                value.chars().count()
            )))
        }
        Some(_) => Err(ActionFailure::Execution(
            "keyboard replacement posted but the field did not change".into(),
        )),
        None => Ok(()),
    }))
}

pub(super) fn post_window_bound_text(
    target: &Target,
    text: &str,
) -> std::result::Result<(), ActionFailure> {
    if target.window_id == 0 {
        return Err(ActionFailure::Execution(
            "element-bound typing requires a target window id; process-wide keyboard fallback suppressed".into(),
        ));
    }
    type_text_background_with_paste_fallback(target.pid, target.window_id, text)
}

pub(super) fn wait_for_string_attr(
    element: &AxElement,
    attr: &str,
    expected: &str,
) -> Option<String> {
    let timeout = type_settle_timeout(expected);
    let started = Instant::now();
    let mut last = attr_string(element, attr)?;
    if last == expected {
        return Some(last);
    }
    loop {
        if started.elapsed() >= timeout {
            return Some(last);
        }
        std::thread::sleep(TYPE_SETTLE_POLL_INTERVAL);
        match attr_string(element, attr) {
            Some(value) if value == expected => return Some(value),
            Some(value) => last = value,
            None => return None,
        }
    }
}

pub(super) fn type_settle_timeout(text: &str) -> Duration {
    let chars = text.chars().count() as u64;
    Duration::from_millis(
        (TYPE_SETTLE_BASE_MS + chars * TYPE_SETTLE_PER_CHAR_MS).min(TYPE_SETTLE_MAX_MS),
    )
}

pub(super) fn text_character_count(element: &AxElement) -> Option<CFIndex> {
    attr_number(element, kAXNumberOfCharactersAttribute)
        .map(|n| n.max(0.0) as CFIndex)
        .or_else(|| {
            attr_string(element, kAXValueAttribute).map(|value| value.chars().count() as CFIndex)
        })
}

pub(super) fn attr_settable(element: &AxElement, attr: &str) -> bool {
    let attr = CFString::new(attr);
    let mut settable: c_uchar = 0;
    // SAFETY: `element` and `attr` are valid; `settable` is a valid
    // out-parameter and AXError is checked before reading it as true.
    let err = unsafe {
        AXUIElementIsAttributeSettable(element.as_ptr(), attr.as_concrete_TypeRef(), &mut settable)
    };
    err == kAXErrorSuccess && settable != 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TextInputAtom {
    Char(char),
    Return { flags: u64 },
}

pub(super) const TEXT_NEWLINE_KEY_FLAGS: u64 = 0x0002_0000;

pub(super) fn text_contains_line_break(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

pub(super) fn for_text_input_atoms<F>(
    text: &str,
    mut f: F,
) -> std::result::Result<(), ActionFailure>
where
    F: FnMut(TextInputAtom) -> std::result::Result<(), ActionFailure>,
{
    let mut previous_was_cr = false;
    for ch in text.chars() {
        match ch {
            '\r' => {
                f(TextInputAtom::Return {
                    flags: TEXT_NEWLINE_KEY_FLAGS,
                })?;
                previous_was_cr = true;
            }
            '\n' if previous_was_cr => {
                previous_was_cr = false;
            }
            '\n' => {
                f(TextInputAtom::Return {
                    flags: TEXT_NEWLINE_KEY_FLAGS,
                })?;
                previous_was_cr = false;
            }
            ch => {
                f(TextInputAtom::Char(ch))?;
                previous_was_cr = false;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        for_text_input_atoms, text_contains_line_break, ActionFailure, TextInputAtom,
        TEXT_NEWLINE_KEY_FLAGS,
    };

    #[test]
    pub(super) fn text_input_atoms_map_line_endings_to_shift_return_keypresses() {
        let mut atoms = Vec::new();
        let result = for_text_input_atoms("a\nb\r\nc\rd", |atom| {
            atoms.push(atom);
            Ok::<_, ActionFailure>(())
        });
        assert!(result.is_ok());

        assert_eq!(
            atoms,
            vec![
                TextInputAtom::Char('a'),
                TextInputAtom::Return {
                    flags: TEXT_NEWLINE_KEY_FLAGS
                },
                TextInputAtom::Char('b'),
                TextInputAtom::Return {
                    flags: TEXT_NEWLINE_KEY_FLAGS
                },
                TextInputAtom::Char('c'),
                TextInputAtom::Return {
                    flags: TEXT_NEWLINE_KEY_FLAGS
                },
                TextInputAtom::Char('d'),
            ]
        );
    }

    #[test]
    pub(super) fn text_contains_line_break_detects_all_supported_line_endings() {
        assert!(!text_contains_line_break("single line"));
        assert!(text_contains_line_break("two\nlines"));
        assert!(text_contains_line_break("two\rlines"));
        assert!(text_contains_line_break("two\r\nlines"));
    }
}
