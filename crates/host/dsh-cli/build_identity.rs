//! Git identity for both normal checkouts and managed worktrees sharing a target.
use std::{path::Path, process::Command};

fn git(manifest: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(manifest).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn emit(manifest: &Path) {
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    // Release orchestration supplies this identity so a shared Cargo cache
    // cannot reuse a build-script result from another frozen source tree.
    println!("cargo:rerun-if-env-changed=DSH_BUILD_SOURCE_ID");
    // Keep a relative dependency too: absolute paths from the previous worktree
    // remain unchanged when Cargo is invoked in a different worktree.
    for (depth, parent) in manifest.ancestors().enumerate() {
        if parent.join(".git").exists() {
            println!("cargo:rerun-if-changed={}{}", "../".repeat(depth), ".git");
            break;
        }
    }
    let revision = git(manifest, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(manifest, &["status", "--porcelain", "--untracked-files=no"])
        .is_none_or(|status| !status.is_empty());
    println!("cargo:rustc-env=DSH_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=DSH_BUILD_DIRTY={dirty}");
    let mut refs = vec!["HEAD".to_owned(), "index".to_owned(), "packed-refs".to_owned()];
    if let Some(reference) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) { refs.push(reference); }
    for reference in refs {
        if let Some(path) = git(manifest, &["rev-parse", "--path-format=absolute", "--git-path", &reference]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
