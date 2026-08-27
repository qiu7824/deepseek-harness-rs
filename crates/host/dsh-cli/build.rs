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
}
