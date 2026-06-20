use super::{
    base64_encode, char_keycode, is_axis_token, is_press_key_name, launchable_app_from_info_json,
    layout_sensitive_hotkey_message, looks_like_clock, parse_combo, parse_value,
    typed_target_value_matches_expected, Engine, TEMP_COUNTER,
};
use dunst_core::{GraphDiff, NodeChange};
use serde_json::json;
use std::{collections::BTreeSet, path::Path, sync::atomic::Ordering};

#[test]
fn base64_matches_known_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

#[test]
fn parse_combo_reads_modifiers_and_key() {
    assert_eq!(parse_combo("cmd+l"), Some((0x0010_0000, 0x25)));
    assert_eq!(parse_combo("cmd+shift+t"), Some((0x0012_0000, 0x11)));
    assert_eq!(parse_combo("ctrl+a"), Some((0x0004_0000, 0x00)));
    assert_eq!(parse_combo("enter"), Some((0, 0x24)));
    assert_eq!(parse_combo("cmd+ "), None); // no key
}

#[test]
fn cmd_a_is_rejected_as_layout_sensitive() {
    let message = layout_sensitive_hotkey_message("cmd+a").unwrap();
    assert!(message.contains("keyboard-layout sensitive"));
    assert!(layout_sensitive_hotkey_message("ctrl+a").is_none());
    assert!(layout_sensitive_hotkey_message("cmd+l").is_none());
    assert!(layout_sensitive_hotkey_message("cmd+shift+a").is_none());
}

#[test]
fn press_key_whitelist_includes_navigation_keys() {
    for key in ["Home", "End", "PageUp", "PageDown", "page_up", "page_down"] {
        assert!(is_press_key_name(key), "{key} should be accepted");
    }
    assert!(!is_press_key_name("definitely-not-a-real-key"));
}

#[test]
fn typed_verification_rejects_partial_target_value() {
    let diff = GraphDiff {
        changes: vec![NodeChange::Changed {
            id: "field_title".into(),
            field: "value".into(),
            before: "old".into(),
            after: "nce - partial".into(),
        }],
    };

    assert!(!typed_target_value_matches_expected(
        "field_title",
        "Freelance - full",
        &diff,
        None,
    ));
}

#[test]
fn typed_verification_accepts_exact_target_value() {
    let diff = GraphDiff {
        changes: vec![NodeChange::Changed {
            id: "field_title".into(),
            field: "value".into(),
            before: "old".into(),
            after: "Freelance - full".into(),
        }],
    };

    assert!(typed_target_value_matches_expected(
        "field_title",
        "Freelance - full",
        &diff,
        None,
    ));
}

#[test]
fn char_and_axis_helpers() {
    assert_eq!(char_keycode('a'), Some(0x00));
    assert_eq!(char_keycode('Z'), Some(0x06));
    assert_eq!(char_keycode('='), Some(0x18));
    assert!(looks_like_clock("13:00 UTC+2"));
    assert!(!looks_like_clock("clôture"));
    assert!(is_axis_token("09:30"));
    assert!(is_axis_token("11"));
    assert!(!is_axis_token("À la clôture de 17:35"));
    assert_eq!(parse_value("8 220,00"), Some(8220.0));
    assert_eq!(parse_value("8161,84'"), Some(8161.84));
}

#[cfg(target_os = "macos")]
#[test]
fn select_file_script_handles_native_panel_process_variants() {
    let script = Engine::select_file_osascript_lines().join("\n");

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

#[cfg(target_os = "macos")]
#[test]
fn select_file_script_compiles_as_applescript() {
    let script = format!("{}\n", Engine::select_file_osascript_lines().join("\n"));
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

#[test]
fn launchable_app_info_is_derived_from_plist_json() {
    let info = json!({
        "CFBundleDisplayName": "Demo Browser",
        "CFBundleName": "Demo",
        "CFBundleIdentifier": "com.example.demo",
        "CFBundleShortVersionString": "1.2.3",
        "LSApplicationCategoryType": "public.app-category.productivity",
        "CFBundleExecutable": "DemoExec",
        "CFBundleGetInfoString": "Demo description"
    });
    let mut running = BTreeSet::new();
    running.insert("demo browser".to_string());
    let app = launchable_app_from_info_json(Path::new("/Applications/Demo.app"), &info, &running)
        .expect("plist json maps to launchable app");
    assert_eq!(app.name, "Demo");
    assert_eq!(app.display_name, "Demo Browser");
    assert_eq!(app.bundle_id.as_deref(), Some("com.example.demo"));
    assert_eq!(app.version.as_deref(), Some("1.2.3"));
    assert_eq!(
        app.category.as_deref(),
        Some("public.app-category.productivity")
    );
    assert_eq!(app.description.as_deref(), Some("Demo description"));
    assert!(app
        .executable
        .as_deref()
        .unwrap()
        .ends_with("Contents/MacOS/DemoExec"));
    assert!(app.running);
}
