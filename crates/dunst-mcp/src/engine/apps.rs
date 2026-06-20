use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{normalize_match, LaunchableApp};

#[cfg(target_os = "macos")]
pub(super) fn app_search_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

#[cfg(target_os = "macos")]
pub(super) fn collect_app_bundles(
    dir: &Path,
    depth: usize,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<PathBuf>,
    max: usize,
) {
    if depth > 3 || out.len() >= max {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= max {
            break;
        }
        let path = entry.path();
        let is_app = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("app"));
        if is_app {
            let key = path.to_string_lossy().to_string();
            if seen.insert(key) {
                out.push(path);
            }
            continue;
        }
        if path.is_dir() {
            collect_app_bundles(&path, depth + 1, seen, out, max);
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn launchable_app_from_bundle(
    path: &Path,
    running: &BTreeSet<String>,
) -> Option<LaunchableApp> {
    let info_path = path.join("Contents/Info.plist");
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&info_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let info: Value = serde_json::from_slice(&output.stdout).ok()?;
    launchable_app_from_info_json(path, &info, running)
}

pub(super) fn launchable_app_from_info_json(
    path: &Path,
    info: &Value,
    running: &BTreeSet<String>,
) -> Option<LaunchableApp> {
    let bundle_name = path.file_stem()?.to_string_lossy().to_string();
    let display_name = info_string(info, "CFBundleDisplayName")
        .or_else(|| info_string(info, "CFBundleName"))
        .unwrap_or_else(|| bundle_name.clone());
    let executable = info_string(info, "CFBundleExecutable").map(|exe| {
        path.join("Contents/MacOS")
            .join(exe)
            .to_string_lossy()
            .to_string()
    });
    let running = running.contains(&normalize_match(&display_name))
        || running.contains(&normalize_match(&bundle_name));
    Some(LaunchableApp {
        name: bundle_name,
        display_name,
        bundle_id: info_string(info, "CFBundleIdentifier"),
        version: info_string(info, "CFBundleShortVersionString")
            .or_else(|| info_string(info, "CFBundleVersion")),
        category: info_string(info, "LSApplicationCategoryType"),
        description: info_string(info, "CFBundleGetInfoString")
            .or_else(|| info_string(info, "NSHumanReadableCopyright")),
        path: path.to_string_lossy().to_string(),
        executable,
        running,
    })
}

fn info_string(info: &Value, key: &str) -> Option<String> {
    info.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
