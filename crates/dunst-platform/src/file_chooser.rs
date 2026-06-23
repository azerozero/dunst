use std::{
    path::Path,
    time::{Duration, Instant},
};

use dunst_core::{DunstError, Result, WindowRef};

const SELECT_FILE_OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(12);

#[cfg(target_os = "macos")]
pub fn select_file(file: &Path, trigger_point: Option<(f64, f64)>, target_pid: i32) -> Result<()> {
    let mut cmd = std::process::Command::new("/usr/bin/osascript");
    append_osascript_lines(&mut cmd, select_file_osascript_lines());
    cmd.arg(file.as_os_str());
    match trigger_point {
        Some((x, y)) => {
            cmd.arg("1");
            cmd.arg(format!("{}", x.round() as i64));
            cmd.arg(format!("{}", y.round() as i64));
        }
        None => {
            cmd.arg("0");
            cmd.arg("0");
            cmd.arg("0");
        }
    }
    cmd.arg(target_pid.to_string());
    let output = command_output_with_timeout(cmd, SELECT_FILE_OSASCRIPT_TIMEOUT, "select_file")?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Err(DunstError::Execution(format!(
        "select_file failed: {}",
        if stderr.is_empty() { stdout } else { stderr }
    )))
}

#[cfg(not(target_os = "macos"))]
pub fn select_file(
    _file: &Path,
    _trigger_point: Option<(f64, f64)>,
    _target_pid: i32,
) -> Result<()> {
    Err(DunstError::Execution(
        "select_file requires a macOS backend".into(),
    ))
}

#[cfg(target_os = "macos")]
pub fn borrow_target_frontmost(target: &WindowRef) -> Result<Option<String>> {
    let mut cmd = std::process::Command::new("/usr/bin/osascript");
    append_osascript_lines(&mut cmd, borrow_target_frontmost_osascript_lines());
    cmd.arg(target.pid.to_string());
    cmd.arg(&target.title);
    let output = cmd
        .output()
        .map_err(|e| DunstError::Execution(format!("borrow window failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(DunstError::Execution(format!(
            "borrow window failed: {}",
            if stderr.is_empty() { stdout } else { stderr }
        )));
    }
    let previous = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!previous.is_empty() && previous != "0").then_some(previous))
}

#[cfg(not(target_os = "macos"))]
pub fn borrow_target_frontmost(_target: &WindowRef) -> Result<Option<String>> {
    Err(DunstError::Execution(
        "borrow_target_frontmost requires a macOS backend".into(),
    ))
}

#[cfg(target_os = "macos")]
pub fn restore_frontmost_pid(pid: &str) -> Result<()> {
    if pid.trim().is_empty() || pid == "0" {
        return Ok(());
    }
    let mut cmd = std::process::Command::new("/usr/bin/osascript");
    append_osascript_lines(&mut cmd, restore_frontmost_osascript_lines());
    cmd.arg(pid);
    let output = cmd
        .output()
        .map_err(|e| DunstError::Execution(format!("restore window failed: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Err(DunstError::Execution(format!(
        "restore window failed: {}",
        if stderr.is_empty() { stdout } else { stderr }
    )))
}

#[cfg(not(target_os = "macos"))]
pub fn restore_frontmost_pid(_pid: &str) -> Result<()> {
    Err(DunstError::Execution(
        "restore_frontmost_pid requires a macOS backend".into(),
    ))
}

#[cfg(target_os = "macos")]
pub fn borrow_target_frontmost_osascript_lines() -> &'static [&'static str] {
    &[
        "on run argv",
        "set targetPid to item 1 of argv",
        "set targetTitle to item 2 of argv",
        "set previousFrontPid to \"0\"",
        "tell application \"System Events\"",
        "try",
        "set previousFrontPid to ((unix id of first application process whose frontmost is true) as text)",
        "end try",
        "set targetProcess to first application process whose unix id is (targetPid as integer)",
        "set frontmost of targetProcess to true",
        "delay 0.05",
        "try",
        "repeat with w in windows of targetProcess",
        "try",
        "set windowName to (name of w) as text",
        "if targetTitle is \"\" or windowName contains targetTitle or targetTitle contains windowName then",
        "perform action \"AXRaise\" of w",
        "exit repeat",
        "end if",
        "end try",
        "end repeat",
        "end try",
        "end tell",
        "return previousFrontPid",
        "end run",
    ]
}

#[cfg(target_os = "macos")]
pub fn restore_frontmost_osascript_lines() -> &'static [&'static str] {
    &[
        "on run argv",
        "set previousFrontPid to item 1 of argv",
        "if previousFrontPid is \"0\" then return",
        "tell application \"System Events\"",
        "try",
        "set frontmost of (first application process whose unix id is (previousFrontPid as integer)) to true",
        "end try",
        "end tell",
        "end run",
    ]
}

#[cfg(target_os = "macos")]
pub fn select_file_osascript_lines() -> &'static [&'static str] {
    &[
        "on hasChooserButton(p)",
        "set okNames to {\"Open\", \"Ouvrir\", \"Choose\", \"Choisir\", \"Upload\", \"Envoi\"}",
        "tell application \"System Events\"",
        "try",
        "repeat with w in windows of p",
        "try",
        "set panelishWindow to false",
        "try",
        "if (name of p) is \"Open and Save Panel Service\" then set panelishWindow to true",
        "end try",
        "try",
        "set wName to (name of w) as text",
        "if wName contains \"Envoi du fichier\" then set panelishWindow to true",
        "end try",
        "try",
        "set wSubrole to subrole of w",
        "if wSubrole is \"AXDialog\" or wSubrole is \"AXSystemDialog\" or wSubrole is \"AXSheet\" then set panelishWindow to true",
        "end try",
        "if panelishWindow is false then error \"skip non-panel window\"",
        "set controls to entire contents of w",
        "repeat with c in controls",
        "try",
        "set cName to name of c",
        "if cName is in okNames then return true",
        "end try",
        "try",
        "if (value of attribute \"AXIdentifier\" of c) is \"OKButton\" then return true",
        "end try",
        "end repeat",
        "end try",
        "end repeat",
        "end try",
        "end tell",
        "return false",
        "end hasChooserButton",
        "on pressChooserButton(p)",
        "set okNames to {\"Open\", \"Ouvrir\", \"Choose\", \"Choisir\", \"Upload\", \"Envoi\"}",
        "tell application \"System Events\"",
        "try",
        "repeat with w in windows of p",
        "try",
        "set panelishWindow to false",
        "try",
        "if (name of p) is \"Open and Save Panel Service\" then set panelishWindow to true",
        "end try",
        "try",
        "set wName to (name of w) as text",
        "if wName contains \"Envoi du fichier\" then set panelishWindow to true",
        "end try",
        "try",
        "set wSubrole to subrole of w",
        "if wSubrole is \"AXDialog\" or wSubrole is \"AXSystemDialog\" or wSubrole is \"AXSheet\" then set panelishWindow to true",
        "end try",
        "if panelishWindow is false then error \"skip non-panel window\"",
        "set controls to entire contents of w",
        "repeat with c in controls",
        "try",
        "set cName to name of c",
        "if cName is in okNames then",
        "if enabled of c then",
        "perform action \"AXPress\" of c",
        "return true",
        "end if",
        "end if",
        "end try",
        "try",
        "if (value of attribute \"AXIdentifier\" of c) is \"OKButton\" then",
        "if enabled of c then",
        "perform action \"AXPress\" of c",
        "return true",
        "end if",
        "end if",
        "end try",
        "end repeat",
        "end try",
        "end repeat",
        "end try",
        "end tell",
        "return false",
        "end pressChooserButton",
        "on firstOpenPanelProcess(targetPid)",
        "tell application \"System Events\"",
        "repeat with p in application processes",
        "try",
        "if (name of p) is \"Open and Save Panel Service\" then",
        "if my hasChooserButton(p) then return p",
        "end if",
        "end try",
        "end repeat",
        "if targetPid is not \"0\" then",
        "repeat with p in application processes",
        "try",
        "if ((unix id of p) as text) is targetPid then",
        "if my hasChooserButton(p) then return p",
        "end if",
        "end try",
        "end repeat",
        "end if",
        "repeat with p in application processes",
        "try",
        "if frontmost of p is true then",
        "if my hasChooserButton(p) then return p",
        "end if",
        "end try",
        "end repeat",
        "end tell",
        "return missing value",
        "end firstOpenPanelProcess",
        "on run argv",
        "set filePath to item 1 of argv",
        "set shouldClick to item 2 of argv",
        "set targetPid to item 5 of argv",
        "tell application \"System Events\"",
        "set previousFrontPid to \"0\"",
        "try",
        "set previousFrontPid to ((unix id of first application process whose frontmost is true) as text)",
        "end try",
        "if shouldClick is \"1\" then",
        "set px to (item 3 of argv) as integer",
        "set py to (item 4 of argv) as integer",
        "click at {px, py}",
        "delay 0.6",
        "end if",
        "set panelProcess to missing value",
        "repeat 20 times",
        "set panelProcess to my firstOpenPanelProcess(targetPid)",
        "if panelProcess is not missing value then exit repeat",
        "delay 0.1",
        "end repeat",
        "if panelProcess is missing value then error \"native file chooser did not open\"",
        "try",
        "set frontmost of panelProcess to true",
        "on error",
        "try",
        "set frontmost of (first application process whose unix id is (targetPid as integer)) to true",
        "end try",
        "end try",
        "delay 0.15",
        "keystroke \"g\" using {command down, shift down}",
        "delay 0.2",
        "keystroke filePath",
        "delay 0.2",
        "key code 36",
        "delay 0.4",
        "set didPressChooserButton to false",
        "repeat 10 times",
        "set didPressChooserButton to my pressChooserButton(panelProcess)",
        "if didPressChooserButton is true then exit repeat",
        "delay 0.15",
        "end repeat",
        "if didPressChooserButton is false then key code 36",
        "delay 0.5",
        "if my hasChooserButton(panelProcess) then error \"native file chooser stayed open after file selection\"",
        "delay 0.2",
        "if previousFrontPid is not \"0\" then",
        "try",
        "set frontmost of (first application process whose unix id is (previousFrontPid as integer)) to true",
        "end try",
        "end if",
        "end tell",
        "end run",
    ]
}

#[cfg(target_os = "macos")]
fn append_osascript_lines(cmd: &mut std::process::Command, lines: &[&str]) {
    for line in lines {
        cmd.arg("-e").arg(line);
    }
}

#[cfg(target_os = "macos")]
fn command_output_with_timeout(
    mut cmd: std::process::Command,
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| DunstError::Execution(format!("{label} failed: {e}")))?;
    let started = Instant::now();

    loop {
        if child
            .try_wait()
            .map_err(|e| DunstError::Execution(format!("{label} wait failed: {e}")))?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|e| DunstError::Execution(format!("{label} failed: {e}")));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output().map_err(|e| {
                DunstError::Execution(format!("{label} timeout cleanup failed: {e}"))
            })?;
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(DunstError::Execution(format!(
                "{label} timed out after {} ms{}{}",
                timeout.as_millis(),
                if stdout.is_empty() {
                    String::new()
                } else {
                    format!("; stdout: {stdout}")
                },
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!("; stderr: {stderr}")
                }
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn select_file_script_handles_native_panel_process_variants() {
        let script = select_file_osascript_lines().join("\n");

        assert!(script.contains("Open and Save Panel Service"));
        assert!(script.contains("targetPid"));
        assert!(script.contains("frontmost of p is true"));
        assert!(script.contains("previousFrontPid"));
        assert!(script.contains("panelishWindow"));
        assert!(script.contains("AXDialog"));
        assert!(script.contains("Envoi du fichier"));
        assert!(script.contains("AXIdentifier"));
        assert!(script.contains("OKButton"));
        assert!(script.contains("skip non-panel window"));
        assert!(script.contains("pressChooserButton"));
        assert!(script.contains("native file chooser stayed open after file selection"));
    }

    #[test]
    fn select_file_script_compiles_as_applescript() {
        let script = format!("{}\n", select_file_osascript_lines().join("\n"));
        let stem = format!(
            "dunst_select_file_{}_{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let source = std::env::temp_dir().join(format!("{stem}.applescript"));
        let compiled = std::env::temp_dir().join(format!("{stem}.scpt"));
        std::fs::write(&source, script).expect("write temporary AppleScript source");

        let output = std::process::Command::new("/usr/bin/osacompile")
            .arg("-o")
            .arg(&compiled)
            .arg(&source)
            .output()
            .expect("run osacompile");

        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_file(&compiled);

        assert!(
            output.status.success(),
            "select_file AppleScript must compile:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
