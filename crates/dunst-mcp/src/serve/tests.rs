use super::*;
use dunst_core::mock::{MockPerceptor, RecordingExecutor};
use dunst_core::{Perceptor, RawAxNode, Target, WindowRef};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn engine() -> Engine {
    engine_with_window(105)
}

fn engine_with_window(window_id: u32) -> Engine {
    let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
    let exec = Box::<RecordingExecutor>::default();
    Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1363,
            window_id,
        },
    )
    .unwrap()
}

fn engine_with_pid(pid: i32) -> Engine {
    let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
    let exec = Box::<RecordingExecutor>::default();
    Engine::new(
        perceptor,
        exec,
        Target {
            pid,
            window_id: 105,
        },
    )
    .unwrap()
}

struct CountingPerceptor {
    roots: Vec<RawAxNode>,
    window: WindowRef,
    captures: Arc<AtomicUsize>,
}

impl Perceptor for CountingPerceptor {
    fn capture(&self, _target: &Target) -> dunst_core::Result<Vec<RawAxNode>> {
        self.captures.fetch_add(1, Ordering::SeqCst);
        Ok(self.roots.clone())
    }

    fn window_ref(&self, _target: &Target) -> dunst_core::Result<WindowRef> {
        Ok(self.window.clone())
    }
}

fn engine_with_capture_counter() -> (Engine, Arc<AtomicUsize>) {
    let roots: Vec<RawAxNode> =
        serde_json::from_str(include_str!("../../../dunst-core/fixtures/notes.json")).unwrap();
    let captures = Arc::new(AtomicUsize::new(0));
    let perceptor = Box::new(CountingPerceptor {
        roots,
        window: WindowRef {
            pid: 1363,
            window_id: 105,
            app_name: "Notes".into(),
            title: "Notes – Aucune note".into(),
        },
        captures: captures.clone(),
    });
    let exec = Box::<RecordingExecutor>::default();
    let eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1363,
            window_id: 105,
        },
    )
    .unwrap();
    (eng, captures)
}

/// Drive `handle_tool_call` exactly as the stdio loop does.
fn call(engine: &mut Engine, name: &str, arguments: Value) -> Value {
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    handle_tool_call_safely(engine, json!(1), &req)
}

fn is_error(resp: &Value) -> bool {
    resp["result"]["isError"].as_bool().unwrap_or(false)
}

fn text(resp: &Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn text_json(resp: &Value) -> Value {
    serde_json::from_str(&text(resp)).expect("tool text is JSON")
}

// Table-driven invariants of the dispatcher (audit #4): a malformed call must
// become a clean `isError:true` JSON-RPC result, never a panic or a success.

mod catalog;
mod dispatcher;
mod raw_tools;
mod responses;
