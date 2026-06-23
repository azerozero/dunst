use dunst_core::{DunstError, Result};

const CMD_FLAG: u64 = 0x0010_0000;
const V_KEYCODE: u16 = 0x09;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_shortcut_uses_command_v() {
        assert_eq!(CMD_FLAG, 0x0010_0000);
        assert_eq!(V_KEYCODE, 0x09);
    }
}
