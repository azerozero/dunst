use dunst_core::{DunstError, Result};

const CMD_FLAG: u64 = 0x0010_0000;
const V_KEYCODE: u16 = 0x09;

/// Delay between issuing Cmd+V and restoring the previous clipboard. The paste
/// keystroke is delivered to the target app's event queue and consumed
/// asynchronously on its run loop, which reads the pasteboard *after*
/// `key_web_background` returns. Restoring sooner races that read and makes the
/// app paste the previous (stale) clipboard. 300 ms matches the proven foreground
/// path (`paste_replace_field_foreground`'s `delay 0.3`).
const PASTE_CONSUME_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

#[cfg(target_os = "macos")]
pub fn read_clipboard_bytes() -> Result<Vec<u8>> {
    let output = std::process::Command::new("pbpaste")
        .output()
        .map_err(|err| DunstError::Execution(format!("read clipboard with pbpaste: {err}")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(DunstError::Execution(format!(
            "read clipboard with pbpaste exited with status {}",
            output.status
        )))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn read_clipboard_bytes() -> Result<Vec<u8>> {
    Err(DunstError::Execution(
        "read_clipboard_bytes requires a macOS backend".into(),
    ))
}

#[cfg(target_os = "macos")]
pub fn write_clipboard_bytes(bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let mut child = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| DunstError::Execution(format!("start pbcopy: {err}")))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| DunstError::Execution("pbcopy stdin unavailable".into()))?
        .write_all(bytes)
        .map_err(|err| DunstError::Execution(format!("write clipboard with pbcopy: {err}")))?;
    let status = child
        .wait()
        .map_err(|err| DunstError::Execution(format!("wait for pbcopy: {err}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(DunstError::Execution(format!(
            "write clipboard with pbcopy exited with status {status}"
        )))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn write_clipboard_bytes(_bytes: &[u8]) -> Result<()> {
    Err(DunstError::Execution(
        "write_clipboard_bytes requires a macOS backend".into(),
    ))
}

#[cfg(target_os = "macos")]
pub fn paste_text_background(
    pid: i32,
    window_id: u32,
    text: &str,
    restore_clipboard: bool,
) -> Result<()> {
    let previous = if restore_clipboard {
        Some(read_clipboard_bytes()?)
    } else {
        None
    };
    write_clipboard_bytes(text.as_bytes())?;
    let paste = crate::key_web_background(pid, window_id, V_KEYCODE, CMD_FLAG);
    // Let the target app consume the paste (read the pasteboard) before putting
    // the old clipboard back; restoring sooner races that read and the app ends
    // up pasting the previous clipboard. Only matters when we actually restore.
    if previous.is_some() {
        std::thread::sleep(PASTE_CONSUME_DELAY);
    }
    let restore = previous
        .as_deref()
        .map(write_clipboard_bytes)
        .unwrap_or(Ok(()));
    match (paste, restore) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(paste_err), Ok(())) => Err(paste_err),
        (Ok(()), Err(restore_err)) => Err(DunstError::Execution(format!(
            "paste completed, but clipboard restore failed: {restore_err}"
        ))),
        (Err(paste_err), Err(restore_err)) => Err(DunstError::Execution(format!(
            "paste failed: {paste_err}; clipboard restore also failed: {restore_err}"
        ))),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn paste_text_background(
    _pid: i32,
    _window_id: u32,
    _text: &str,
    _restore_clipboard: bool,
) -> Result<()> {
    Err(DunstError::Execution(
        "paste_text_background requires a macOS backend".into(),
    ))
}

/// Replace the focused field's whole content in one layout-safe step: put `text` on
/// the clipboard, foreground the target process, then native select-all + paste.
///
/// AppleScript `keystroke "a"/"v"` is translated to the **current keyboard layout**,
/// so it works on AZERTY/QWERTY/etc. — unlike a hardcoded keycode, where Cmd+A
/// (keycode 0x00) becomes **Cmd+Q = Quit** on AZERTY. The native Cmd+A also selects
/// the field's real DOM content (no AX char-count under-report), so there is no
/// trailing fragment. Foregrounds the window (not transparent); the field must
/// already be focused (click it first). Restores the previous clipboard.
#[cfg(target_os = "macos")]
pub fn paste_replace_field_foreground(pid: i32, text: &str) -> Result<()> {
    let previous = read_clipboard_bytes().ok();
    write_clipboard_bytes(text.as_bytes())?;
    let script = format!(
        "tell application \"System Events\"\n\
         set frontmost of (first process whose unix id is {pid}) to true\n\
         delay 0.4\n\
         keystroke \"a\" using command down\n\
         delay 0.2\n\
         keystroke \"v\" using command down\n\
         delay 0.3\n\
         end tell"
    );
    let result = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output();
    if let Some(prev) = previous {
        let _ = write_clipboard_bytes(&prev);
    }
    match result {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(DunstError::Execution(format!(
            "osascript paste-replace failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
        Err(err) => Err(DunstError::Execution(format!(
            "osascript spawn failed: {err}"
        ))),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn paste_replace_field_foreground(_pid: i32, _text: &str) -> Result<()> {
    Err(DunstError::Execution(
        "paste_replace_field_foreground requires a macOS backend".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_shortcut_uses_command_v() {
        assert_eq!(CMD_FLAG, 0x0010_0000);
        assert_eq!(V_KEYCODE, 0x09);
    }

    #[test]
    fn paste_consume_delay_outlasts_async_paste() {
        // Restoring the clipboard before the target app reads the pasteboard
        // makes it paste stale content; the delay must stay non-zero and at
        // least as long as the proven foreground path's `delay 0.3`.
        assert!(PASTE_CONSUME_DELAY >= std::time::Duration::from_millis(300));
    }
}
