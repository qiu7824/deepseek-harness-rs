#[allow(dead_code)]
#[path = "../../../crates/host/dsh-cli/build_identity.rs"]
mod build_identity;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../../crates/host/dsh-cli/build_identity.rs");
    build_identity::emit_named(
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()),
        "DSH_NATIVE",
    );
    let source = std::env::var("DSH_BUILD_SOURCE_ID").unwrap_or_default();
    let source = if source.len() == 64 && source.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        source.as_str()
    } else {
        "unknown"
    };
    println!("cargo:rustc-env=DSH_NATIVE_SOURCE_SHA256={source}");
}
