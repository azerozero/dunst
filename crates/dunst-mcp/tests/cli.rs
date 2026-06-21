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
    for option in ["--client", "--dev-wrapper"] {
        assert!(
            setup_out.contains(option),
            "setup --help should mention {option:?}; got:\n{setup_out}"
        );
    }
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
        claude_out.contains("\"args\": [\"serve\"]"),
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
