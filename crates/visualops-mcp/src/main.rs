//! VisualOps MCP — entrypoint.
//!
//! Subcommands:
//!   demo        Run the AX-first pipeline on the bundled Notes fixture
//!               (device-free) and narrate scene -> affordance -> risk-gating
//!               -> audit. Proves the thesis without macOS.
//!   serve       Start the MCP stdio server (wired during integration).
//!
//! The real macOS backend (`visualops-platform`) is swapped in for a live
//! target once WP-A lands; the engine code is identical.

mod engine;
mod serve;

use engine::Engine;
use visualops_core::mock::{MockPerceptor, RecordingExecutor};
use visualops_core::{ActionResult, SemanticAction, Target};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "demo".into());
    let code = match mode.as_str() {
        "demo" => run_demo(),
        "serve" => run_serve(),
        other => {
            eprintln!("unknown mode '{other}' (expected: demo | serve)");
            2
        }
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
    let target = Target { pid: 1363, window_id: 105 };
    let mut eng = match Engine::new(perceptor, Box::new(RecordingExecutor::default()), target) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine init failed: {e}");
            return 1;
        }
    };

    let g = eng.scene_graph();
    println!("# VisualOps demo — Notes (fixture, AX-only)\n");
    println!("scene graph: {} nodes, {} root(s), window \"{}\"\n", g.nodes.len(), g.roots.len(), g.window.title);

    // 1) Resolve a benign action by LABEL, not coordinates.
    section("1. find_element(\"Nouvelle note\") + affordances");
    if let Some(n) = pick(&eng, "Nouvelle note", None) {
        let id = n.id.clone();
        let bbox = n.bbox;
        let aff = eng.affordance_graph().affordances.get(&id).cloned();
        println!("  -> id={id}  role={:?}  bbox={:?}", role_of(&eng, &id), bbox);
        if let Some(a) = &aff {
            println!("     actions={:?}  risk={:?} (approval={})", a.actions, a.risk.level, a.risk.requires_approval);
        }
        section("2. click_element(\"btn_nouvelle_note\") — low risk, proceeds");
        match eng.click_element(&id, Some("create a new note")) {
            Ok(entry) => println!("  -> result={:?}", entry.result),
            Err(e) => println!("  -> error: {e}"),
        }
    } else {
        println!("  (not found — is visualops-graph implemented?)");
        return 1;
    }

    // 2) A destructive action is GATED until approved.
    section("3. click_element on \"Supprimer\" — high risk, DENIED pending approval");
    if let Some(n) = pick(&eng, "Supprimer", None) {
        let id = n.id.clone();
        if let Some(a) = eng.affordance_graph().affordances.get(&id) {
            println!("  risk={:?} approval={} reasons={:?}", a.risk.level, a.risk.requires_approval, a.risk.reasons);
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
fn run_serve() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let pid = flag(&args, "--pid").and_then(|s| s.parse::<i32>().ok());
    let window = flag(&args, "--window").and_then(|s| s.parse::<u32>().ok());

    let engine = match (pid, window) {
        (Some(pid), Some(window_id)) => {
            use visualops_platform::MacosBackend;
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
            eprintln!("visualops-mcp: no --pid/--window; serving the Notes fixture.");
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
                Target { pid: 1363, window_id: 105 },
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

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn section(t: &str) {
    println!("\n\x1b[1m{t}\x1b[0m");
}

/// Pick the best `find_element` match, optionally requiring an affordance.
fn pick<'a>(eng: &'a Engine, query: &str, requires: Option<SemanticAction>) -> Option<&'a visualops_core::SceneNode> {
    eng.find_element(query).into_iter().find(|n| match requires {
        None => true,
        Some(act) => eng
            .affordance_graph()
            .affordances
            .get(&n.id)
            .map(|a| a.actions.contains(&act))
            .unwrap_or(false),
    })
}

fn role_of(eng: &Engine, id: &str) -> Option<visualops_core::Role> {
    eng.scene_graph().get(id).map(|n| n.role)
}
