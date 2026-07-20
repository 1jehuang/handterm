use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    println!("cargo:rerun-if-env-changed=HANDTERM_GIT_COMMIT");

    if let Some(reference) = current_git_reference() {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }

    let git_commit = std::env::var("HANDTERM_GIT_COMMIT")
        .ok()
        .or_else(detect_git_commit)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=HANDTERM_GIT_COMMIT={git_commit}");
}

fn current_git_reference() -> Option<String> {
    let head = std::fs::read_to_string(".git/HEAD").ok()?;
    let reference = head.trim().strip_prefix("ref: ")?;
    (!reference.is_empty()).then(|| reference.to_string())
}

fn detect_git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    if commit.is_empty() {
        None
    } else {
        Some(commit.to_string())
    }
}
