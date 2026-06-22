use std::{path::PathBuf, process::Command};

fn dunst_mcp() -> Command {
    let path = option_env!("CARGO_BIN_EXE_dunst-mcp")
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_else(|| {
            let mut path = std::env::current_exe().expect("current test executable");
            path.pop();
            if path.file_name().and_then(|name| name.to_str()) == Some("deps") {
                path.pop();
            }
            path.push("dunst-mcp");
            path
        });
    Command::new(path)
}

#[test]
fn top_level_help_lists_all_operator_commands() {
    let output = dunst_mcp().arg("--help").output().expect("run --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8(output.stdout).expect("utf8 help");

    for command in ["demo", "serve", "doctor", "setup"] {
        assert!(
            stdout.contains(command),
            "top-level help should mention {command:?}; got:\n{stdout}"
        );
    }
}

#[test]
fn subcommand_help_lists_live_and_setup_options() {
    let serve = dunst_mcp()
        .args(["serve", "--help"])
        .output()
        .expect("run serve --help");
    assert!(serve.status.success(), "serve --help should exit 0");
    let serve_out = String::from_utf8(serve.stdout).expect("utf8 serve help");
    for option in ["--pid", "--window", "--app", "--live"] {
        assert!(
            serve_out.contains(option),
            "serve --help should mention {option:?}; got:\n{serve_out}"
        );
    }

    let setup = dunst_mcp()
        .args(["setup", "--help"])
        .output()
        .expect("run setup --help");
    assert!(setup.status.success(), "setup --help should exit 0");
    let setup_out = String::from_utf8(setup.stdout).expect("utf8 setup help");
    for option in [
        "--client",
        "--dev-wrapper",
        "--dry-run",
        "--apply",
        "--edit",
        "--migrate",
        "--config",
    ] {
        assert!(
            setup_out.contains(option),
            "setup --help should mention {option:?}; got:\n{setup_out}"
        );
    }
}

#[test]
#[cfg(target_os = "macos")]
fn doctor_reports_permission_statuses() {
    let output = dunst_mcp().arg("doctor").output().expect("run doctor");
    let stdout = String::from_utf8(output.stdout).expect("utf8 doctor");

    assert!(
        stdout.contains("accessibility:"),
        "doctor should report Accessibility status:\n{stdout}"
    );
    assert!(
        stdout.contains("screen recording:"),
        "doctor should report Screen Recording status:\n{stdout}"
    );
    assert!(
        !stdout.contains("not checked by this minimal doctor"),
        "doctor should not leave Screen Recording as an unchecked gap:\n{stdout}"
    );
}

#[test]
fn setup_installed_configs_start_the_server() {
    let codex = dunst_mcp()
        .args(["setup", "--client", "codex"])
        .output()
        .expect("run codex setup");
    assert!(codex.status.success(), "codex setup should exit 0");
    let codex_out = String::from_utf8(codex.stdout).expect("utf8 codex setup");
    assert!(
        codex_out.contains("command = \"dunst-mcp\""),
        "installed codex setup should use the binary:\n{codex_out}"
    );
    assert!(
        codex_out.contains("args = [\"serve\"]"),
        "installed codex setup must start the MCP server, not demo:\n{codex_out}"
    );

    let claude = dunst_mcp()
        .args(["setup", "--client", "claude"])
        .output()
        .expect("run claude setup");
    assert!(claude.status.success(), "claude setup should exit 0");
    let claude_out = String::from_utf8(claude.stdout).expect("utf8 claude setup");
    assert!(
        claude_out.contains("\"command\": \"dunst-mcp\""),
        "installed claude setup should use the binary:\n{claude_out}"
    );
    assert!(
        claude_out.contains("\"args\":") && claude_out.contains("\"serve\""),
        "installed claude setup must start the MCP server, not demo:\n{claude_out}"
    );
}

#[test]
fn setup_dev_wrapper_keeps_wrapper_owned_args_empty() {
    let output = dunst_mcp()
        .args(["setup", "--client", "codex", "--dev-wrapper"])
        .output()
        .expect("run codex dev-wrapper setup");
    assert!(output.status.success(), "dev wrapper setup should exit 0");
    let stdout = String::from_utf8(output.stdout).expect("utf8 dev-wrapper setup");

    assert!(
        stdout.contains("command = \"scripts/mcp-dunst.sh\""),
        "dev wrapper setup should use the repo wrapper:\n{stdout}"
    );
    assert!(
        stdout.contains("args = []"),
        "dev wrapper already starts serve --live and should not get duplicate args:\n{stdout}"
    );
}

#[test]
fn setup_apply_writes_idempotent_codex_config() {
    let path =
        std::env::temp_dir().join(format!("dunst-mcp-test-{}-codex.toml", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let output = dunst_mcp()
        .args([
            "setup",
            "--client",
            "codex",
            "--apply",
            "--config",
            path.to_str().unwrap(),
        ])
        .output()
        .expect("run setup --apply");
    assert!(output.status.success(), "setup --apply should exit 0");
    let written = std::fs::read_to_string(&path).expect("config written");
    assert!(written.contains("[mcp_servers.dunst]"));
    assert!(written.contains("command = \"dunst-mcp\""));
    assert!(written.contains("args = [\"serve\"]"));

    let output = dunst_mcp()
        .args([
            "setup",
            "--client",
            "codex",
            "--apply",
            "--config",
            path.to_str().unwrap(),
        ])
        .output()
        .expect("rerun setup --apply");
    assert!(
        output.status.success(),
        "second setup --apply should exit 0"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("config still present"),
        written,
        "setup --apply should be idempotent"
    );
    let _ = std::fs::remove_file(&path);
}
