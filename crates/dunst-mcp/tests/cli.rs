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
