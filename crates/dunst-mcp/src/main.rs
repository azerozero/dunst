//! Dunst MCP — CLI entrypoint.
//!
//! Subcommands:
//!   demo        Run the AX-first pipeline on the bundled Notes fixture
//!   serve       Start the MCP stdio server
//!   doctor      Print local environment diagnostics for MCP setup
//!   setup       Manage MCP client config snippets/files

mod engine;
mod serve;

use clap::{Args, Parser, Subcommand, ValueEnum};
use dunst_core::mock::{MockPerceptor, RecordingExecutor, NOTES_FIXTURE_WINDOW_ID};
use dunst_core::{ActionResult, SemanticAction, Target};
use engine::Engine;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Fixture target for the device-free `demo` and the no-target `serve` fallback:
/// the bundled Notes capture, not a live process.
const DEMO_TARGET: Target = Target {
    pid: 1363,
    window_id: NOTES_FIXTURE_WINDOW_ID,
};
const CLI_LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n",
    "git ",
    env!("DUNST_BUILD_GIT_SHA"),
    "\n",
    "dirty ",
    env!("DUNST_BUILD_GIT_DIRTY"),
    "\n",
    "built_unix ",
    env!("DUNST_BUILD_TIME_UNIX")
);
/// Committed project-local stdio entry point, also the `--dev-wrapper` command.
const DEV_WRAPPER: &str = "scripts/mcp-dunst.sh";

#[derive(Debug, Parser)]
#[command(
    name = "dunst-mcp",
    version,
    long_version = CLI_LONG_VERSION,
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
    Doctor(DoctorArgs),
    /// Manage MCP client configuration snippets/files.
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
    /// Pick the frontmost sizeable on-screen window for this app owner name.
    #[arg(long, value_name = "APP")]
    app: Option<String>,
    /// Pick the frontmost sizeable on-screen window of any app.
    #[arg(long)]
    live: bool,
}

#[derive(Args, Debug, Default)]
struct DoctorArgs {
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct SetupArgs {
    /// Client config format to print.
    #[arg(long, value_enum, default_value_t = SetupClient::Codex)]
    client: SetupClient,
    /// Use this checkout's development wrapper instead of installed dunst-mcp.
    #[arg(long)]
    dev_wrapper: bool,
    /// Print the planned config without writing files.
    #[arg(long, conflicts_with_all = ["apply", "edit", "migrate"])]
    dry_run: bool,
    /// Write or update the selected config file idempotently.
    #[arg(long, conflicts_with_all = ["dry_run", "edit", "migrate"])]
    apply: bool,
    /// Show the current file and the merged desired config without writing.
    #[arg(long, conflicts_with_all = ["dry_run", "apply", "migrate"])]
    edit: bool,
    /// Rewrite an existing dunst entry's command to the current launch command.
    ///
    /// Not a schema migration: it re-renders the `dunst` server entry to the
    /// command `setup` would write now (the `scripts/mcp-dunst.sh` wrapper inside
    /// a checkout, otherwise `dunst-mcp serve`) while preserving other sections.
    /// Pass `--dev-wrapper` to force the wrapper form.
    #[arg(long, conflicts_with_all = ["dry_run", "apply", "edit"])]
    migrate: bool,
    /// Override the config path; defaults to .codex/config.toml or .mcp.json.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
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
        Command::Doctor(args) => run_doctor(args),
        Command::Setup(args) => run_setup(args),
    };
    std::process::exit(code);
}

/// Build the device-free `Engine` used by the `demo` command and by the
/// no-target `serve` fallback: the bundled Notes fixture as both perceptor
/// and executor.
fn fixture_engine() -> Result<Engine, String> {
    let perceptor =
        MockPerceptor::notes_fixture().map_err(|e| format!("fixture load failed: {e}"))?;
    Engine::new(
        Box::new(perceptor),
        Box::new(RecordingExecutor::default()),
        DEMO_TARGET,
    )
    .map_err(|e| format!("engine init failed: {e}"))
}

/// Demo sections 1-2: resolve "Nouvelle note" by label and click it (low
/// risk, proceeds). Returns `false` if the element is not found, in which
/// case the caller aborts the demo.
fn demo_find_and_click(eng: &mut Engine) -> bool {
    section("1. find_element(\"Nouvelle note\") + affordances");
    let Some(n) = pick(eng, "Nouvelle note", None) else {
        println!("  (not found — is dunst-graph implemented?)");
        return false;
    };
    let id = n.id.clone();
    let bbox = n.bbox;
    let aff = eng.affordance_graph().affordances.get(&id).cloned();
    println!(
        "  -> id={id}  role={:?}  bbox={:?}",
        role_of(eng, &id),
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
    true
}

/// Demo sections 3-4: click "Supprimer" (high risk, denied pending
/// approval), then approve it and retry (proceeds).
fn demo_gated_delete(eng: &mut Engine) {
    section("3. click_element on \"Supprimer\" — high risk, DENIED pending approval");
    let Some(n) = pick(eng, "Supprimer", None) else {
        return;
    };
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

/// Demo sections 5-6: type into the note body, then export the audit trace.
fn demo_type_and_trace(eng: &mut Engine) {
    section("5. type_into(text area, \"Bonjour\")");
    if let Some(n) = pick(eng, "Corps de la note", Some(SemanticAction::Type)) {
        let id = n.id.clone();
        match eng.type_into(&id, "Bonjour", Some("write greeting")) {
            Ok(entry) => println!("  -> id={id}  result={:?}", entry.result),
            Err(e) => println!("  -> error: {e}"),
        }
    }

    section("6. export_trace()");
    match eng.export_trace() {
        Ok(json) => println!("{json}"),
        Err(e) => println!("  -> error: {e}"),
    }
}

/// Run the device-free Notes fixture demo end to end.
fn run_demo() -> i32 {
    let mut eng = match fixture_engine() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
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

    if !demo_find_and_click(&mut eng) {
        return 1;
    }
    demo_gated_delete(&mut eng);
    demo_type_and_trace(&mut eng);
    0
}

/// Resolve `--app`/`--live` to a concrete `(pid, window_id)` by picking a
/// live on-screen window, printing the same messages `run_serve` used to
/// print inline. Returns `None` (after eprintln'ing why) when no target was
/// requested this way, or no matching window was found.
#[cfg(target_os = "macos")]
fn resolve_live_target(args: &ServeArgs) -> Option<(i32, u32)> {
    if !(args.app.is_some() || args.live) {
        return None;
    }
    // Dynamic targeting: CoreGraphics returns layer-0 windows in z-order, so pick
    // the first sizeable on-screen match. With multiple Firefox windows this means
    // the active/frontmost eligible window, not whichever window happens to be
    // largest.
    let pick = dunst_vision::capture::list_windows().into_iter().find(|w| {
        w.on_screen
            && w.w > 200.0
            && w.h > 200.0
            && match args.app.as_deref() {
                Some(app) => w.app == app,
                None => true,
            }
    });
    match pick {
        Some(w) => {
            eprintln!(
                "dunst-mcp: target -> pid={} window={} {:?} (attach to re-target)",
                w.pid, w.window_id, w.title
            );
            Some((w.pid, w.window_id))
        }
        None => {
            eprintln!("dunst-mcp: no matching on-screen window found");
            None
        }
    }
}

/// Build the live macOS `Engine` for `serve --pid P --window W`.
fn live_engine(pid: i32, window_id: u32) -> Result<Engine, String> {
    use dunst_platform::MacosBackend;
    Engine::new(
        Box::new(MacosBackend::new()),
        Box::new(MacosBackend::new()),
        Target { pid, window_id },
    )
    .map_err(|e| format!("engine init (live pid={pid} window={window_id}) failed: {e}"))
}

/// Start the MCP stdio server. With `--pid P --window W` it drives a live macOS
/// window via the AX backend; otherwise it serves the Notes fixture so the
/// server is runnable and inspectable without a target.
fn run_serve(args: ServeArgs) -> i32 {
    let mut pid = args.pid;
    let mut window = args.window;
    let requested_live_target = args.app.is_some() || args.live;

    #[cfg(target_os = "macos")]
    if pid.is_none() || window.is_none() {
        if let Some((p, w)) = resolve_live_target(&args) {
            pid = Some(p);
            window = Some(w);
        }
    }

    if requested_live_target && (pid.is_none() || window.is_none()) {
        eprintln!(
            "dunst-mcp: live target requested but no matching window was found; refusing fixture fallback"
        );
        return 1;
    }

    let engine = match (pid, window) {
        (Some(pid), Some(window_id)) => match live_engine(pid, window_id) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        },
        _ => {
            eprintln!("dunst-mcp: no --pid/--window; serving the Notes fixture.");
            match fixture_engine() {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            }
        }
    };
    serve::serve(engine)
}

fn run_doctor(args: DoctorArgs) -> i32 {
    let binary = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|err| format!("unknown ({err})"));
    let approve_tool = std::env::var("DUNST_MCP_ENABLE_APPROVE_TOOL")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let checks = collect_doctor_checks();
    // Reduce the per-check severities to one exit code: worst wins, so a single
    // FAIL yields 2 (blocking) even amid passes, while WARN alone yields 1.
    let worst = checks
        .iter()
        .map(|check| check.status)
        .max()
        .unwrap_or(Health::Pass);
    let code = match worst {
        Health::Fail => 2,
        Health::Warn => 1,
        Health::Pass | Health::Info => 0,
    };
    if args.json {
        print_doctor_json(&binary, approve_tool, &checks, code);
    } else {
        print_doctor_text(&binary, approve_tool, &checks);
    }
    code
}

/// Severity of a single [`run_doctor`] check.
///
/// Declared worst-last so `Ord` makes the maximum severity the one that decides
/// the process exit code (`Fail` -> 2, `Warn` -> 1, `Pass`/`Info` -> 0).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Health {
    /// Context line that never affects the exit code.
    Info,
    /// Check succeeded.
    Pass,
    /// Non-blocking issue; degrades an optional capability.
    Warn,
    /// Blocking issue for live automation.
    Fail,
}

impl Health {
    fn as_str(self) -> &'static str {
        match self {
            Health::Info => "info",
            Health::Pass => "pass",
            Health::Warn => "warn",
            Health::Fail => "fail",
        }
    }
}

/// One line of doctor output plus the machine-readable status behind it.
struct DoctorCheck {
    label: String,
    status: Health,
    message: String,
    hint: Option<String>,
}

impl DoctorCheck {
    fn new(label: impl Into<String>, status: Health, message: impl Into<String>) -> Self {
        DoctorCheck {
            label: label.into(),
            status,
            message: message.into(),
            hint: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

fn collect_doctor_checks() -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    checks.push(path_check(".mcp.json", "Claude-style project config"));
    if let Some(check) = config_check(".mcp.json", SetupClient::Claude) {
        checks.push(check);
    }
    checks.push(path_check(".codex/config.toml", "Codex project config"));
    if let Some(check) = config_check(".codex/config.toml", SetupClient::Codex) {
        checks.push(check);
    }
    checks.push(executable_check(
        "scripts/mcp-dunst.sh",
        "development wrapper",
    ));
    checks.push(executable_check(
        "target/debug/dunst-mcp",
        "development binary",
    ));
    checks.push(cargo_check());
    checks.extend(platform_checks());
    checks
}

fn path_check(path: &str, label: &str) -> DoctorCheck {
    let present = Path::new(path).exists();
    DoctorCheck::new(
        format!("{path}:present"),
        Health::Info,
        format!(
            "{label}: {} ({path})",
            if present { "present" } else { "missing" }
        ),
    )
}

fn executable_check(path: &str, label: &str) -> DoctorCheck {
    let status = if is_executable(Path::new(path)) {
        "executable"
    } else if Path::new(path).is_file() {
        "not executable"
    } else {
        "missing"
    };
    DoctorCheck::new(
        format!("{path}:exec"),
        Health::Info,
        format!("{label}: {status} ({path})"),
    )
}

fn cargo_check() -> DoctorCheck {
    // The dev wrapper builds `target/debug/dunst-mcp` on launch, so a missing
    // toolchain would only surface at the MCP handshake. Report it now, but keep
    // it informational: installed configs calling `dunst-mcp serve` never need it.
    if resolve_command_path("cargo").is_some() {
        DoctorCheck::new("cargo", Health::Info, "cargo: present")
    } else {
        DoctorCheck::new("cargo", Health::Info, "cargo: not found").with_hint(
            "only needed to build target/debug via scripts/mcp-dunst.sh; installed configs using `dunst-mcp serve` do not need it",
        )
    }
}

/// Validates the `dunst` entry in a client config: the command must both start
/// the server (not the demo) and resolve to a real executable.
fn config_check(path: &str, client: SetupClient) -> Option<DoctorCheck> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed = match client {
        SetupClient::Claude => parse_claude_dunst_config(&text),
        SetupClient::Codex => parse_codex_dunst_config(&text),
    };
    let label = format!("{path}:command");
    Some(match parsed {
        Ok(Some((command, args))) => {
            if !mcp_command_starts_server(&command, &args) {
                DoctorCheck::new(
                    label,
                    Health::Fail,
                    format!(
                        "{path}: warning: dunst command may start the demo instead of MCP serve ({command} {args:?})"
                    ),
                )
                .with_hint("point the command at `dunst-mcp serve` or scripts/mcp-dunst.sh")
            } else if resolve_command_path(&command).is_none() {
                DoctorCheck::new(
                    label,
                    Health::Warn,
                    format!("{path}: dunst command ok ({command} {args:?})"),
                )
                .with_hint(format!(
                    "{command} not found on PATH or relative to the current directory; run `cargo install --path crates/dunst-mcp` or launch from the project root"
                ))
            } else {
                DoctorCheck::new(
                    label,
                    Health::Pass,
                    format!("{path}: dunst command ok ({command} {args:?})"),
                )
            }
        }
        Ok(None) => DoctorCheck::new(label, Health::Fail, format!("{path}: dunst server missing")),
        Err(err) => DoctorCheck::new(label, Health::Fail, format!("{path}: invalid ({err})")),
    })
}

#[cfg(target_os = "macos")]
fn platform_checks() -> Vec<DoctorCheck> {
    let mut checks = vec![DoctorCheck::new("os", Health::Info, "os: macOS")];
    if dunst_platform::accessibility_trusted() {
        checks.push(DoctorCheck::new(
            "accessibility",
            Health::Pass,
            "accessibility: granted",
        ));
    } else {
        // Accessibility gates every AX action: without it live automation cannot
        // work, so this is a hard failure.
        checks.push(
            DoctorCheck::new("accessibility", Health::Fail, "accessibility: not granted").with_hint(
                "enable Accessibility for your terminal/agent host in System Settings > Privacy & Security > Accessibility",
            ),
        );
    }
    if dunst_platform::screen_capture_trusted() {
        checks.push(DoctorCheck::new(
            "screen-recording",
            Health::Pass,
            "screen recording: granted",
        ));
    } else {
        // Only screenshot/OCR tools need Screen Recording; core AX automation runs
        // without it, so this degrades a capability rather than blocking.
        checks.push(
            DoctorCheck::new("screen-recording", Health::Warn, "screen recording: not granted").with_hint(
                "enable Screen Recording for your terminal/agent host in System Settings > Privacy & Security > Screen Recording (only screenshot/OCR tools need it)",
            ),
        );
    }
    checks
}

#[cfg(not(target_os = "macos"))]
fn platform_checks() -> Vec<DoctorCheck> {
    vec![DoctorCheck::new(
        "os",
        Health::Warn,
        "os: unsupported (dunst-mcp live automation is macOS-only)",
    )
    .with_hint("fixture mode is still available with: dunst-mcp serve")]
}

fn print_doctor_text(binary: &str, approve_tool: bool, checks: &[DoctorCheck]) {
    println!("dunst-mcp doctor");
    println!("binary: {binary}");
    println!("recommended MCP command: dunst-mcp serve");
    println!("setup dry-run: dunst-mcp setup --client codex --dry-run");
    println!("setup apply: dunst-mcp setup --client codex --apply");
    println!("setup edit: dunst-mcp setup --client codex --edit");
    println!("setup migrate: dunst-mcp setup --client codex --migrate");
    println!(
        "approval tool: {}",
        if approve_tool {
            "enabled by environment"
        } else {
            "disabled by default"
        }
    );
    for check in checks {
        println!("{}", check.message);
        if let Some(hint) = &check.hint {
            println!("hint: {hint}");
        }
    }
}

fn print_doctor_json(binary: &str, approve_tool: bool, checks: &[DoctorCheck], code: i32) {
    let status = match code {
        2 => "fail",
        1 => "warn",
        _ => "pass",
    };
    let rendered: Vec<serde_json::Value> = checks
        .iter()
        .map(|check| {
            json!({
                "check": check.label,
                "status": check.status.as_str(),
                "message": check.message,
                "hint": check.hint,
            })
        })
        .collect();
    let out = json!({
        "tool": "doctor",
        "binary": binary,
        "approve_tool_enabled": approve_tool,
        "status": status,
        "exit_code": code,
        "checks": rendered,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

/// Reports whether `path` is a regular file with an executable bit set.
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.is_file() && meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                meta.is_file()
            }
        })
        .unwrap_or(false)
}

/// Resolves `command` to an existing executable, honoring `PATH` for bare names.
///
/// A command containing a path separator is treated as a filesystem path
/// (absolute, or relative to the current directory); a bare name is looked up in
/// each `PATH` entry. Returns [`None`] when nothing executable matches.
fn resolve_command_path(command: &str) -> Option<PathBuf> {
    let candidate = Path::new(command);
    if candidate.components().count() > 1 {
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join(command);
        is_executable(&candidate).then_some(candidate)
    })
}

fn mcp_command_starts_server(command: &str, args: &[String]) -> bool {
    let command_name = std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command);
    command_name == "mcp-dunst.sh"
        || command_name == "dunst-mcp" && args.iter().any(|arg| arg == "serve")
}

fn parse_claude_dunst_config(text: &str) -> Result<Option<(String, Vec<String>)>, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("json parse failed: {err}"))?;
    let Some(server) = root
        .get("mcpServers")
        .and_then(|servers| servers.get("dunst"))
    else {
        return Ok(None);
    };
    let command = server
        .get("command")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "mcpServers.dunst.command missing or not a string".to_string())?;
    let args = server
        .get("args")
        .map(json_string_array)
        .transpose()?
        .unwrap_or_default();
    Ok(Some((command.to_string(), args)))
}

fn parse_codex_dunst_config(text: &str) -> Result<Option<(String, Vec<String>)>, String> {
    let mut in_dunst = false;
    let mut command = None;
    let mut args = None;

    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_dunst = line == "[mcp_servers.dunst]";
            continue;
        }
        if !in_dunst {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "command" => {
                command = Some(parse_toml_string(value.trim())?);
            }
            "args" => {
                let parsed: serde_json::Value = serde_json::from_str(value.trim())
                    .map_err(|err| format!("args parse failed: {err}"))?;
                args = Some(json_string_array(&parsed)?);
            }
            _ => {}
        }
    }

    match command {
        Some(command) => Ok(Some((command, args.unwrap_or_default()))),
        None if text.contains("[mcp_servers.dunst]") => {
            Err("mcp_servers.dunst.command missing".into())
        }
        None => Ok(None),
    }
}

fn parse_toml_string(value: &str) -> Result<String, String> {
    serde_json::from_str(value).map_err(|err| format!("command parse failed: {err}"))
}

fn json_string_array(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| "args must be an array".to_string())?;
    array
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| "args entries must be strings".to_string())
        })
        .collect()
}

/// Chooses the MCP launch command written into a client config.
///
/// The project ships `scripts/mcp-dunst.sh` as the committed stdio entry point,
/// so inside a checkout (or with `--dev-wrapper`) the wrapper is written to match
/// that convention; elsewhere the installed `dunst-mcp serve` is used. This keeps
/// `setup --apply` from silently rewriting the project's own committed config to a
/// binary that may be absent from `PATH`.
fn desired_command(dev_wrapper: bool, wrapper_present: bool) -> (String, Vec<String>) {
    if dev_wrapper || wrapper_present {
        (DEV_WRAPPER.to_string(), Vec::new())
    } else {
        ("dunst-mcp".to_string(), vec!["serve".to_string()])
    }
}

/// Everything `setup` computed for one run, rendered as text or JSON.
struct SetupReport<'a> {
    mode: SetupMode,
    client: SetupClient,
    path: &'a Path,
    command: &'a str,
    args: &'a [String],
    existing: Option<&'a str>,
    desired: &'a str,
    merged: &'a str,
    changed: bool,
    wrote: bool,
    backup: Option<&'a Path>,
}

fn run_setup(args: SetupArgs) -> i32 {
    let mode = setup_mode(&args);
    // Match the committed project convention (the wrapper) when operating inside a
    // checkout, falling back to the installed binary elsewhere.
    let wrapper_present = Path::new(DEV_WRAPPER).exists();
    let (command, command_args) = desired_command(args.dev_wrapper, wrapper_present);
    let path = args
        .config
        .clone()
        .unwrap_or_else(|| default_setup_path(args.client));
    let desired = render_setup_config(args.client, &command, &command_args);
    let existing = std::fs::read_to_string(&path).ok();
    let merged = match merge_setup_config(args.client, existing.as_deref(), &command, &command_args)
    {
        Ok(merged) => merged,
        Err(err) => {
            eprintln!("setup: {err}");
            return 1;
        }
    };
    let changed = existing.as_deref() != Some(merged.as_str());

    // Migrate is a rewrite, never a first write: the entry must already exist
    // (that is what `--apply` is for).
    if mode == SetupMode::Migrate {
        if existing.is_none() {
            eprintln!(
                "setup: cannot migrate missing config {}; run setup --apply to create it",
                path.display()
            );
            return 1;
        }
        if !existing_config_has_dunst(args.client, existing.as_deref().unwrap_or("")) {
            eprintln!(
                "setup: cannot migrate {}; no existing dunst server entry was found",
                path.display()
            );
            return 1;
        }
    }

    let mut wrote = false;
    let mut backup = None;
    if matches!(mode, SetupMode::Apply | SetupMode::Migrate) {
        match perform_write(&path, &merged, changed) {
            Ok(created_backup) => {
                wrote = true;
                backup = created_backup;
            }
            Err(err) => {
                eprintln!("setup: {err}");
                return 1;
            }
        }
    }

    let report = SetupReport {
        mode,
        client: args.client,
        path: &path,
        command: &command,
        args: &command_args,
        existing: existing.as_deref(),
        desired: &desired,
        merged: &merged,
        changed,
        wrote,
        backup: backup.as_deref(),
    };
    if args.json {
        print_setup_json(&report);
    } else {
        print_setup_text(&report);
    }
    0
}

fn print_setup_text(report: &SetupReport) {
    println!("dunst-mcp setup");
    println!("mode: {}", report.mode.as_str());
    println!("client: {}", report.client.as_str());
    println!("path: {}", report.path.display());
    println!("command: {} {:?}", report.command, report.args);
    println!(
        "status: {}",
        if report.changed {
            "changes pending"
        } else {
            "already up to date"
        }
    );

    match report.mode {
        SetupMode::DryRun => {
            println!("\n# Desired config");
            print_block(report.desired);
            println!("\n# Merged result");
            print_block(report.merged);
        }
        SetupMode::Edit => {
            println!("\n# Current config");
            match report.existing {
                Some(text) => print_block(text),
                None => println!("(missing)"),
            }
            println!("\n# Merged result");
            print_block(report.merged);
        }
        SetupMode::Apply | SetupMode::Migrate => {
            if report.wrote {
                println!("written: {}", report.path.display());
            }
            if let Some(backup) = report.backup {
                println!("backup: {}", backup.display());
            }
            // Setup writes config but does not validate it; point the operator at
            // doctor so a fresh registration is immediately checkable.
            println!("next: run `dunst-mcp doctor` to validate the environment");
        }
    }
}

fn print_setup_json(report: &SetupReport) {
    let out = json!({
        "tool": "setup",
        "mode": report.mode.as_str(),
        "client": report.client.as_str(),
        "path": report.path.display().to_string(),
        "command": report.command,
        "args": report.args,
        "changed": report.changed,
        "status": if report.changed { "changes pending" } else { "already up to date" },
        "wrote": report.wrote,
        "backup": report.backup.map(|backup| backup.display().to_string()),
        "current": report.existing,
        "desired": report.desired,
        "merged": report.merged,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

/// Prints `text`, guaranteeing a trailing newline so the next heading is clean.
fn print_block(text: &str) {
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SetupMode {
    DryRun,
    Apply,
    Edit,
    Migrate,
}

impl SetupMode {
    fn as_str(self) -> &'static str {
        match self {
            SetupMode::DryRun => "dry-run",
            SetupMode::Apply => "apply",
            SetupMode::Edit => "edit",
            SetupMode::Migrate => "migrate",
        }
    }
}

impl SetupClient {
    fn as_str(self) -> &'static str {
        match self {
            SetupClient::Codex => "codex",
            SetupClient::Claude => "claude",
        }
    }
}

fn setup_mode(args: &SetupArgs) -> SetupMode {
    if args.apply {
        SetupMode::Apply
    } else if args.edit {
        SetupMode::Edit
    } else if args.migrate {
        SetupMode::Migrate
    } else {
        SetupMode::DryRun
    }
}

fn default_setup_path(client: SetupClient) -> PathBuf {
    match client {
        SetupClient::Codex => PathBuf::from(".codex/config.toml"),
        SetupClient::Claude => PathBuf::from(".mcp.json"),
    }
}

fn render_setup_config(client: SetupClient, command: &str, args: &[String]) -> String {
    match client {
        SetupClient::Codex => format!(
            "[mcp_servers.dunst]\ncommand = \"{}\"\nargs = {}\nstartup_timeout_sec = 120\n",
            escape_toml_string(command),
            json_string_list(args)
        ),
        SetupClient::Claude => {
            let value = json!({
                "mcpServers": {
                    "dunst": {
                        "command": command,
                        "args": args,
                    }
                }
            });
            format!("{}\n", serde_json::to_string_pretty(&value).unwrap())
        }
    }
}

fn merge_setup_config(
    client: SetupClient,
    existing: Option<&str>,
    command: &str,
    args: &[String],
) -> Result<String, String> {
    match client {
        SetupClient::Codex => Ok(merge_codex_config(existing.unwrap_or(""), command, args)),
        SetupClient::Claude => merge_claude_config(existing.unwrap_or("{}"), command, args),
    }
}

fn merge_codex_config(existing: &str, command: &str, args: &[String]) -> String {
    let desired = render_setup_config(SetupClient::Codex, command, args);
    if existing.trim().is_empty() {
        return desired;
    }

    let mut out = Vec::new();
    let mut inserted = false;
    let mut skipping_dunst = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if skipping_dunst {
                out.push(desired.trim_end().to_string());
                inserted = true;
                skipping_dunst = false;
            }
            if trimmed == "[mcp_servers.dunst]" {
                skipping_dunst = true;
                continue;
            }
        }
        if !skipping_dunst {
            out.push(line.to_string());
        }
    }
    if skipping_dunst || !inserted {
        if !out.is_empty() && !out.last().is_some_and(|line| line.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(desired.trim_end().to_string());
    }
    format!("{}\n", out.join("\n"))
}

fn merge_claude_config(existing: &str, command: &str, args: &[String]) -> Result<String, String> {
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).map_err(|err| format!("json parse failed: {err}"))?
    };
    if !root.is_object() {
        return Err("root JSON value must be an object".into());
    }
    if root.get("mcpServers").is_none() {
        root["mcpServers"] = json!({});
    }
    if !root["mcpServers"].is_object() {
        return Err("mcpServers must be an object".into());
    }
    root["mcpServers"]["dunst"] = json!({
        "command": command,
        "args": args,
    });
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&root).unwrap()
    ))
}

fn existing_config_has_dunst(client: SetupClient, existing: &str) -> bool {
    match client {
        SetupClient::Codex => parse_codex_dunst_config(existing)
            .map(|entry| entry.is_some())
            .unwrap_or(false),
        SetupClient::Claude => parse_claude_dunst_config(existing)
            .map(|entry| entry.is_some())
            .unwrap_or(false),
    }
}

/// Writes `merged` to `path`, backing up an existing file first when it changes.
///
/// Creates the parent directory as needed. When the file already exists and the
/// content differs, the previous contents are copied to `<path>.bak` before the
/// overwrite so an `apply`/`migrate` never loses hand-edited config irrecoverably.
///
/// # Errors
///
/// Returns the failing filesystem operation (directory creation, backup copy, or
/// write) rendered as a message string.
fn perform_write(path: &Path, merged: &str, changed: bool) -> Result<Option<PathBuf>, String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create {} failed: {err}", parent.display()))?;
    }
    // Only snapshot when we are about to change existing content: a fresh write
    // has nothing to preserve and an idempotent no-op should not churn a `.bak`.
    let backup = if changed && path.exists() {
        let backup = backup_path(path);
        std::fs::copy(path, &backup)
            .map_err(|err| format!("backup {} failed: {err}", backup.display()))?;
        Some(backup)
    } else {
        None
    };
    std::fs::write(path, merged)
        .map_err(|err| format!("write {} failed: {err}", path.display()))?;
    Ok(backup)
}

/// Appends `.bak` to `path`, preserving the original extension (`config.toml` ->
/// `config.toml.bak`, `.mcp.json` -> `.mcp.json.bak`).
fn backup_path(path: &Path) -> PathBuf {
    let mut raw = path.as_os_str().to_owned();
    raw.push(".bak");
    PathBuf::from(raw)
}

fn json_string_list(args: &[String]) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "[]".into())
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_config_detects_dunst_server_command() {
        let text = r#"{
          "mcpServers": {
            "dunst": { "command": "dunst-mcp", "args": ["serve"] }
          }
        }"#;

        let (command, args) = parse_claude_dunst_config(text).unwrap().unwrap();

        assert_eq!(command, "dunst-mcp");
        assert_eq!(args, vec!["serve"]);
        assert!(mcp_command_starts_server(&command, &args));
    }

    #[test]
    fn codex_config_accepts_project_wrapper_without_duplicate_args() {
        let text = r#"
        [mcp_servers.dunst]
        command = "scripts/mcp-dunst.sh"
        args = []
        startup_timeout_sec = 120
        "#;

        let (command, args) = parse_codex_dunst_config(text).unwrap().unwrap();

        assert_eq!(command, "scripts/mcp-dunst.sh");
        assert!(args.is_empty());
        assert!(mcp_command_starts_server(&command, &args));
    }

    #[test]
    fn installed_binary_without_serve_is_flagged() {
        assert!(!mcp_command_starts_server("dunst-mcp", &[]));
        assert!(!mcp_command_starts_server("dunst-mcp", &["demo".into()]));
        assert!(mcp_command_starts_server(
            "/usr/local/bin/dunst-mcp",
            &["serve".into()]
        ));
    }

    #[test]
    fn installed_claude_config_without_serve_is_invalid_for_doctor() {
        let text = r#"{
          "mcpServers": {
            "dunst": { "command": "dunst-mcp", "args": [] }
          }
        }"#;

        let (command, args) = parse_claude_dunst_config(text).unwrap().unwrap();

        assert_eq!(command, "dunst-mcp");
        assert!(args.is_empty());
        assert!(!mcp_command_starts_server(&command, &args));
    }

    #[test]
    fn desired_command_prefers_wrapper_inside_a_checkout() {
        // Inside a checkout (wrapper present) or with --dev-wrapper: write the
        // committed wrapper, matching the project's own .mcp.json convention.
        assert_eq!(
            desired_command(false, true),
            ("scripts/mcp-dunst.sh".to_string(), Vec::new())
        );
        assert_eq!(
            desired_command(true, false),
            ("scripts/mcp-dunst.sh".to_string(), Vec::new())
        );
        // Installed context (no wrapper on disk, not forced): use the binary.
        assert_eq!(
            desired_command(false, false),
            ("dunst-mcp".to_string(), vec!["serve".to_string()])
        );
    }

    #[test]
    fn backup_path_appends_bak_preserving_extension() {
        assert_eq!(
            backup_path(Path::new(".codex/config.toml")),
            PathBuf::from(".codex/config.toml.bak")
        );
        assert_eq!(
            backup_path(Path::new(".mcp.json")),
            PathBuf::from(".mcp.json.bak")
        );
    }

    #[test]
    fn health_orders_worst_last() {
        assert!(Health::Fail > Health::Warn);
        assert!(Health::Warn > Health::Pass);
        assert!(Health::Pass > Health::Info);
        let statuses = [Health::Info, Health::Fail, Health::Pass, Health::Warn];
        assert_eq!(statuses.into_iter().max(), Some(Health::Fail));
    }

    #[test]
    fn resolve_command_path_handles_paths_and_bare_names() {
        // A concrete executable path resolves to itself.
        let exe = std::env::current_exe().unwrap();
        assert!(resolve_command_path(exe.to_str().unwrap()).is_some());
        // A bare name with no PATH match does not resolve.
        assert!(resolve_command_path("dunst-definitely-not-installed-xyz").is_none());
    }
}
