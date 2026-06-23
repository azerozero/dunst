use super::*;
use dunst_core::mock::MockPerceptor;
use dunst_core::{RiskLevel, SessionIdentity};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Executor that counts invocations, so we can assert a gated action never
/// reaches the OS.
struct CountingExecutor(Arc<AtomicUsize>);
impl ActionExecutor for CountingExecutor {
    fn perform(
        &self,
        _t: &Target,
        _n: &SceneNode,
        _a: SemanticAction,
        _arg: Option<&str>,
    ) -> dunst_core::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn engine_from_json(json: &str, app_name: &str, title: &str) -> Engine {
    let perceptor = Box::new(
        MockPerceptor::from_json(
            json,
            WindowRef {
                pid: 4242,
                window_id: 2424,
                app_name: app_name.into(),
                title: title.into(),
            },
        )
        .unwrap(),
    );
    let exec = Box::new(CountingExecutor(Arc::new(AtomicUsize::new(0))));
    Engine::new(
        perceptor,
        exec,
        Target {
            pid: 4242,
            window_id: 2424,
        },
    )
    .unwrap()
}

fn engine_with_counter() -> (Engine, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
    let exec = Box::new(CountingExecutor(calls.clone()));
    let eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1363,
            window_id: 105,
        },
    )
    .unwrap();
    (eng, calls)
}

#[test]
fn engine_new_normalizes_main_window_placeholder_target() {
    let calls = Arc::new(AtomicUsize::new(0));
    let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
    let exec = Box::new(CountingExecutor(calls));
    let eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1363,
            window_id: 0,
        },
    )
    .unwrap();

    assert_eq!(eng.target(), (1363, 105));
}

type RecordedCall = (String, SemanticAction, Option<String>);

/// Executor that records every `(id, action, argument)` it receives, so we
/// can assert exactly what the engine resolved an action to.
struct RecordingExecutor(Arc<Mutex<Vec<RecordedCall>>>);
impl ActionExecutor for RecordingExecutor {
    fn perform(
        &self,
        _t: &Target,
        n: &SceneNode,
        a: SemanticAction,
        arg: Option<&str>,
    ) -> dunst_core::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push((n.id.clone(), a, arg.map(str::to_owned)));
        Ok(())
    }
}

fn engine_with_recorder() -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
    let exec = Box::new(RecordingExecutor(calls.clone()));
    let eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1363,
            window_id: 105,
        },
    )
    .unwrap();
    (eng, calls)
}

/// An id from the affordance graph that exposes `Drag` and is *not* risk
/// gated, so the executor actually runs (rows/cells in the notes fixture).
fn non_gated_drag_source(eng: &Engine) -> String {
    eng.query_affordances(SemanticAction::Drag)
        .into_iter()
        .find(|id| {
            !eng.affordance_graph().affordances[id]
                .risk
                .requires_approval
        })
        .expect("a non-gated draggable source in the notes fixture")
}

fn id_for(eng: &Engine, query: &str) -> String {
    eng.find_element(query)
        .first()
        .map(|n| n.id.clone())
        .unwrap_or_else(|| panic!("no element for {query:?}"))
}

fn raw_node(
    ax_role: &str,
    label: Option<&str>,
    value: Option<&str>,
    frame: Option<Bbox>,
    ax_actions: &[&str],
    children: Vec<dunst_core::RawAxNode>,
) -> dunst_core::RawAxNode {
    dunst_core::RawAxNode {
        ax_role: ax_role.into(),
        label: label.map(str::to_owned),
        help: None,
        value: value.map(str::to_owned),
        ax_identifier: None,
        ax_actions: ax_actions.iter().map(|s| s.to_string()).collect(),
        frame,
        enabled: true,
        focused: false,
        children,
    }
}

fn test_bbox(x: f64, y: f64, w: f64, h: f64) -> Option<Bbox> {
    Some(Bbox { x, y, w, h })
}

fn engine_from_roots(
    roots: Vec<dunst_core::RawAxNode>,
    app_name: &str,
    title: &str,
) -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let perceptor = Box::new(MockPerceptor::new(
        roots,
        WindowRef {
            pid: 1,
            window_id: 1,
            app_name: app_name.into(),
            title: title.into(),
        },
    ));
    let exec = Box::new(RecordingExecutor(calls.clone()));
    let eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1,
            window_id: 1,
        },
    )
    .unwrap();
    (eng, calls)
}

fn engine_from_sequence(
    captures: Vec<Vec<dunst_core::RawAxNode>>,
    app_name: &str,
    title: &str,
) -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let perceptor = Box::new(SequencePerceptor::new(
        captures,
        WindowRef {
            pid: 1,
            window_id: 1,
            app_name: app_name.into(),
            title: title.into(),
        },
    ));
    let exec = Box::new(RecordingExecutor(calls.clone()));
    let eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1,
            window_id: 1,
        },
    )
    .unwrap();
    (eng, calls)
}

fn remove_tag_roots(count: usize) -> Vec<dunst_core::RawAxNode> {
    let children = (0..count)
        .map(|idx| {
            raw_node(
                "AXButton",
                Some("remove Platform Engineering"),
                None,
                test_bbox(20.0 + (idx as f64 * 32.0), 80.0, 24.0, 24.0),
                &["press"],
                vec![],
            )
        })
        .collect();
    vec![raw_node(
        "AXWindow",
        Some("Expertises"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        children,
    )]
}

fn checkbox_roots(value: &str) -> Vec<dunst_core::RawAxNode> {
    vec![raw_node(
        "AXWindow",
        Some("Expertises"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        vec![raw_node(
            "AXCheckBox",
            Some("DevOps"),
            Some(value),
            test_bbox(40.0, 80.0, 24.0, 24.0),
            &["press"],
            vec![],
        )],
    )]
}

struct SequencePerceptor {
    captures: Mutex<Vec<Vec<dunst_core::RawAxNode>>>,
    last: Mutex<Vec<dunst_core::RawAxNode>>,
    window: WindowRef,
}

impl SequencePerceptor {
    fn new(captures: Vec<Vec<dunst_core::RawAxNode>>, window: WindowRef) -> Self {
        Self {
            captures: Mutex::new(captures),
            last: Mutex::new(Vec::new()),
            window,
        }
    }
}

impl Perceptor for SequencePerceptor {
    fn capture(&self, _target: &Target) -> dunst_core::Result<Vec<dunst_core::RawAxNode>> {
        let next = {
            let mut captures = self.captures.lock().unwrap();
            if captures.is_empty() {
                None
            } else {
                Some(captures.remove(0))
            }
        };
        if let Some(roots) = next {
            *self.last.lock().unwrap() = roots.clone();
            Ok(roots)
        } else {
            Ok(self.last.lock().unwrap().clone())
        }
    }

    fn window_ref(&self, _target: &Target) -> dunst_core::Result<WindowRef> {
        Ok(self.window.clone())
    }
}

mod actions;
mod drag_type;
mod graph_views;
mod page_text;
mod raw_window;
