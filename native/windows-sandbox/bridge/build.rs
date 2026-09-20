use std::process::Command;
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn main() {
    println!(
        "cargo:rustc-env=DSH_NATIVE_REVISION={}",
        git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into())
    );
    println!(
        "cargo:rustc-env=DSH_NATIVE_DIRTY={}",
        git(&["status", "--porcelain", "--untracked-files=no"]).is_none_or(|s| !s.is_empty())
    );
    for file in ["HEAD", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", file]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
