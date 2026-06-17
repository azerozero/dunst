//! Dunst MCP — CLI entrypoint.
//!
//! Subcommands:
//!   demo        Run the AX-first pipeline on the bundled Notes fixture
//!   serve       Start the MCP stdio server
//!   doctor      Print local environment diagnostics for MCP setup
//!   setup       Print MCP client config snippets

mod engine;
mod serve;

use clap::{Args, Parser, Subcommand, ValueEnum};
use dunst_core::mock::{MockPerceptor, RecordingExecutor};
use dunst_core::{ActionResult, SemanticAction, Target};
use engine::Engine;

/// Fixture target for the device-free `demo` and the no-target `serve` fallback:
/// the bundled Notes capture, not a live process.
const DEMO_TARGET: Target = Target {
    pid: 1363,
    window_id: 105,
};

#[derive(Debug, Parser)]
#[command(
    name = "dunst-mcp",
    version,
    about = "AX-first macOS MCP server for background UI automation",
    long_about = "Dunst MCP exposes a macOS AX-first affordance graph over MCP, with risk gating and an audit trail. The default command runs the device-free Notes fixture demo."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the device-free Notes fixture demo.
    Demo,
    /// Start the MCP stdio server.
    Serve(ServeArgs),
    /// Print local environment diagnostics.
    Doctor,
    /// Print MCP client configuration snippets without writing files.
    Setup(SetupArgs),
}

#[derive(Args, Debug, Default)]
struct ServeArgs {
    /// Target a live process id.
    #[arg(long, value_name = "PID")]
    pid: Option<i32>,
    /// Target a live WindowServer window id.
    #[arg(long, value_name = "WINDOW_ID")]
    window: Option<u32>,
    /// Pick the largest on-screen window for this app owner name.
    #[arg(long, value_name = "APP")]
    app: Option<String>,
    /// Pick the largest on-screen window of any app.
    #[arg(long)]
    live: bool,
}

#[derive(Args, Debug)]
struct SetupArgs {
    /// Client config format to print.
    #[arg(long, value_enum, default_value_t = SetupClient::Codex)]
    client: SetupClient,
    /// Use this checkout's development wrapper instead of installed dunst-mcp.
    #[arg(long)]
    dev_wrapper: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SetupClient {
    Codex,
    Claude,
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command.unwrap_or(Command::Demo) {
        Command::Demo => run_demo(),
        Command::Serve(args) => run_serve(args),
        Command::Doctor => run_doctor(),
        Command::Setup(args) => run_setup(args),
    };
    std::process::exit(code);
}

fn run_demo() -> i32 {
    let perceptor = match MockPerceptor::notes_fixture() {
        Ok(p) => Box::new(p),
        Err(e) => {
            eprintln!("fixture load failed: {e}");
            return 1;
        }
    };
    let target = DEMO_TARGET;
    let mut eng = match Engine::new(perceptor, Box::new(RecordingExecutor::default()), target) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine init failed: {e}");
            return 1;
        }
    };

    let g = eng.scene_graph();
    println!("# Dunst MCP demo — Notes (fixture, AX-only)\n");
    println!(
        "scene graph: {} nodes, {} root(s), window \"{}\"\n",
        g.nodes.len(),
        g.roots.len(),
        g.window.title
    );

    // 1) Resolve a benign action by LABEL, not coordinates.
    section("1. find_element(\"Nouvelle note\") + affordances");
    if let Some(n) = pick(&eng, "Nouvelle note", None) {
        let id = n.id.clone();
        let bbox = n.bbox;
        let aff = eng.affordance_graph().affordances.get(&id).cloned();
        println!(
            "  -> id={id}  role={:?}  bbox={:?}",
            role_of(&eng, &id),
            bbox
        );
        if let Some(a) = &aff {
            println!(
                "     actions={:?}  risk={:?} (approval={})",
                a.actions, a.risk.level, a.risk.requires_approval
            );
        }
        section("2. click_element(\"btn_nouvelle_note\") — low risk, proceeds");
        match eng.click_element(&id, Some("create a new note")) {
            Ok(entry) => println!("  -> result={:?}", entry.result),
            Err(e) => println!("  -> error: {e}"),
        }
    } else {
        println!("  (not found — is dunst-graph implemented?)");
        return 1;
    }

    // 2) A destructive action is GATED until approved.
    section("3. click_element on \"Supprimer\" — high risk, DENIED pending approval");
    if let Some(n) = pick(&eng, "Supprimer", None) {
        let id = n.id.clone();
        if let Some(a) = eng.affordance_graph().affordances.get(&id) {
            println!(
                "  risk={:?} approval={} reasons={:?}",
                a.risk.level, a.risk.requires_approval, a.risk.reasons
            );
        }
        match eng.click_element(&id, Some("user asked to delete")) {
            Ok(entry) => {
                println!("  -> result={:?}", entry.result);
                if entry.result == ActionResult::PendingApproval {
                    section("4. approve(id) then retry — proceeds");
                    if let Err(e) = eng.approve(&id) {
                        println!("  -> approve rejected: {e}");
                    }
                    match eng.click_element(&id, Some("approved by operator")) {
                        Ok(e2) => println!("  -> result={:?}", e2.result),
                        Err(e) => println!("  -> error: {e}"),
                    }
                }
            }
            Err(e) => println!("  -> error: {e}"),
        }
    }

    // 3) Type into the note body.
    section("5. type_into(text area, \"Bonjour\")");
    if let Some(n) = pick(&eng, "Corps de la note", Some(SemanticAction::Type)) {
        let id = n.id.clone();
        match eng.type_into(&id, "Bonjour", Some("write greeting")) {
            Ok(entry) => println!("  -> id={id}  result={:?}", entry.result),
            Err(e) => println!("  -> error: {e}"),
        }
    }

    // 4) Audit trail.
    section("6. export_trace()");
    match eng.export_trace() {
        Ok(json) => println!("{json}"),
        Err(e) => println!("  -> error: {e}"),
    }
    0
}

/// Start the MCP stdio server. With `--pid P --window W` it drives a live macOS
/// window via the AX backend; otherwise it serves the Notes fixture so the
/// server is runnable and inspectable without a target.
fn run_serve(args: ServeArgs) -> i32 {
    let mut pid = args.pid;
    let mut window = args.window;
    let requested_live_target = args.app.is_some() || args.live;

    // Dynamic targeting: `--app "<Owner Name>"` picks that app's largest on-screen
    // window; `--live` bootstraps to the largest on-screen window of ANY app. In
    // both cases the client re-targets at runtime with `list_windows` + `attach`,
    // so nothing is hardcoded. (Robust to window-id churn, e.g. Chrome.)
    #[cfg(target_os = "macos")]
    if (pid.is_none() || window.is_none()) && (args.app.is_some() || args.live) {
        let pick = dunst_vision::capture::list_windows()
            .into_iter()
            .filter(|w| {
                w.on_screen
                    && w.w > 200.0
                    && w.h > 200.0
                    && match args.app.as_deref() {
                        Some(app) => w.app == app,
                        None => true,
                    }
            })
            .max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h)));
        match pick {
            Some(w) => {
                eprintln!(
                    "dunst-mcp: target -> pid={} window={} {:?} (attach to re-target)",
                    w.pid, w.window_id, w.title
                );
                pid = Some(w.pid);
                window = Some(w.window_id);
            }
            None => eprintln!("dunst-mcp: no matching on-screen window found"),
        }
    }

    if requested_live_target && (pid.is_none() || window.is_none()) {
        eprintln!(
            "dunst-mcp: live target requested but no matching window was found; refusing fixture fallback"
        );
        return 1;
    }

    let engine = match (pid, window) {
        (Some(pid), Some(window_id)) => {
            use dunst_platform::MacosBackend;
            match Engine::new(
                Box::new(MacosBackend::new()),
                Box::new(MacosBackend::new()),
                Target { pid, window_id },
            ) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("engine init (live pid={pid} window={window_id}) failed: {e}");
                    return 1;
                }
            }
        }
        _ => {
            eprintln!("dunst-mcp: no --pid/--window; serving the Notes fixture.");
            let p = match MockPerceptor::notes_fixture() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("fixture load failed: {e}");
                    return 1;
                }
            };
            match Engine::new(
                Box::new(p),
                Box::new(RecordingExecutor::default()),
                DEMO_TARGET,
            ) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("engine init failed: {e}");
                    return 1;
                }
            }
        }
    };
    serve::serve(engine)
}

fn run_doctor() -> i32 {
    println!("dunst-mcp doctor");
    match std::env::current_exe() {
        Ok(path) => println!("binary: {}", path.display()),
        Err(err) => println!("binary: unknown ({err})"),
    }
    println!("recommended MCP command: dunst-mcp serve");
    println!(
        "approval tool: {}",
        if std::env::var("DUNST_MCP_ENABLE_APPROVE_TOOL")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false)
        {
            "enabled by environment"
        } else {
            "disabled by default"
        }
    );
    doctor_path(".mcp.json", "Claude-style project config");
    doctor_path(".codex/config.toml", "Codex project config");
    doctor_executable("scripts/mcp-dunst.sh", "development wrapper");
    doctor_executable("target/debug/dunst-mcp", "development binary");

    #[cfg(target_os = "macos")]
    {
        println!("os: macOS");
        if dunst_platform::accessibility_trusted() {
            println!("accessibility: granted");
            println!("screen recording: not checked by this minimal doctor");
            0
        } else {
            println!("accessibility: not granted");
            println!(
                "hint: enable Accessibility for your terminal/agent host in System Settings > Privacy & Security > Accessibility"
            );
            println!("screen recording: enable it too if you use OCR/screenshot tools");
            1
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        println!("os: unsupported (dunst-mcp live automation is macOS-only)");
        println!("fixture mode is still available with: dunst-mcp serve");
        1
    }
}

fn doctor_path(path: &str, label: &str) {
    let status = if std::path::Path::new(path).exists() {
        "present"
    } else {
        "missing"
    };
    println!("{label}: {status} ({path})");
}

fn doctor_executable(path: &str, label: &str) {
    let status = std::fs::metadata(path)
        .map(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if m.permissions().mode() & 0o111 != 0 {
                    "executable"
                } else {
                    "not executable"
                }
            }
            #[cfg(not(unix))]
            {
                if m.is_file() {
                    "present"
                } else {
                    "missing"
                }
            }
        })
        .unwrap_or("missing");
    println!("{label}: {status} ({path})");
}

fn run_setup(args: SetupArgs) -> i32 {
    let command = if args.dev_wrapper {
        "scripts/mcp-dunst.sh"
    } else {
        "dunst-mcp"
    };
    match args.client {
        SetupClient::Codex => {
            println!("# Add to .codex/config.toml or $CODEX_HOME/config.toml");
            println!("[mcp_servers.dunst]");
            println!("command = \"{command}\"");
            println!("args = []");
            println!("startup_timeout_sec = 120");
        }
        SetupClient::Claude => {
            println!("{{");
            println!("  \"mcpServers\": {{");
            println!("    \"dunst\": {{");
            println!("      \"command\": \"{command}\",");
            println!("      \"args\": []");
            println!("    }}");
            println!("  }}");
            println!("}}");
        }
    }
    0
}

fn section(t: &str) {
    println!("\n\x1b[1m{t}\x1b[0m");
}

/// Pick the best `find_element` match, optionally requiring an affordance.
fn pick<'a>(
    eng: &'a Engine,
    query: &str,
    requires: Option<SemanticAction>,
) -> Option<&'a dunst_core::SceneNode> {
    eng.find_element(query)
        .into_iter()
        .find(|n| match requires {
            None => true,
            Some(act) => eng
                .affordance_graph()
                .affordances
                .get(&n.id)
                .map(|a| a.actions.contains(&act))
                .unwrap_or(false),
        })
}

fn role_of(eng: &Engine, id: &str) -> Option<dunst_core::Role> {
    eng.scene_graph().get(id).map(|n| n.role)
}
