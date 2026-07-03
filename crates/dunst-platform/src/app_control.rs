//! Launching and quitting applications through macOS command-line helpers.

/// Launches `app` in the background, optionally opening `url`; returns whether it started.
///
/// Uses `open -g -a`. Any `extra_args` are passed as application argv after `--args`.
#[cfg(target_os = "macos")]
pub fn launch_app(app: &str, url: Option<&str>, extra_args: &[String]) -> bool {
    let mut cmd = std::process::Command::new("/usr/bin/open");
    cmd.args(["-g", "-a", app]);
    // `open` treats paths/URLs before `--args` as documents to open, and
    // everything after `--args` as application argv. Keep the URL before
    // `--args`; otherwise Chrome/Firefox can launch but stay on a new tab.
    if let Some(u) = url {
        cmd.arg(u);
    }
    if !extra_args.is_empty() {
        cmd.arg("--args");
        cmd.args(extra_args);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// Non-macOS stub; returns `false`.
#[cfg(not(target_os = "macos"))]
pub fn launch_app(_app: &str, _url: Option<&str>, _extra_args: &[String]) -> bool {
    false
}

/// Asks `app` to quit via AppleScript and returns whether the command succeeded.
#[cfg(target_os = "macos")]
pub fn close_app(app: &str) -> bool {
    std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "quit application (item 1 of argv)",
            "-e",
            "end run",
            app,
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Non-macOS stub; returns `false`.
#[cfg(not(target_os = "macos"))]
pub fn close_app(_app: &str) -> bool {
    false
}
