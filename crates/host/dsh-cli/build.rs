use std::{env, path::PathBuf, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    #[cfg(windows)]
    {
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packaging/windows/deepseek-black.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(icon.to_string_lossy().as_ref());
        resource.set("ProductName", "DeepSeek Harness-rs");
        resource.set("FileDescription", "DeepSeek Harness-rs Web Agent");
        resource.set("OriginalFilename", "deepseek harness-rs.exe");
        resource.set("InternalName", "deepseek-harness-rs");
        resource.compile().expect("compile Windows resources");
    }
    println!("cargo:rerun-if-changed=../../../packaging/windows/deepseek-black.ico");
    let revision = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_none_or(|status| !status.is_empty());
    println!("cargo:rustc-env=DSH_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=DSH_BUILD_DIRTY={dirty}");
    println!("cargo:rerun-if-changed=build.rs");
    for entry in ["HEAD", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", entry]) {
            let path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join(path);
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }
}
