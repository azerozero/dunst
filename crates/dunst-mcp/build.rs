use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../dunst-platform/src");
    println!("cargo:rerun-if-changed=../dunst-vision/src");
    println!("cargo:rerun-if-changed=../dunst-graph/src");
    println!("cargo:rerun-if-changed=../dunst-core/src");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let git_sha = command_stdout("git", &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    let git_dirty = match Command::new("git")
        .args(["diff", "--quiet", "--ignore-submodules", "HEAD", "--"])
        .status()
    {
        Ok(status) if status.success() => "false",
        Ok(_) => "true",
        Err(_) => "unknown",
    };
    let built_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=DUNST_BUILD_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=DUNST_BUILD_GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=DUNST_BUILD_TIME_UNIX={built_unix}");
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
