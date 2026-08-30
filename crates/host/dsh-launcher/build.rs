fn main() {
    #[cfg(windows)]
    {
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packaging/windows/deepseek-black.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(icon.to_string_lossy().as_ref());
        resource.set("ProductName", "DeepSeek Harness-rs");
        resource.set("FileDescription", "DeepSeek Harness-rs ZSUI Launcher");
        resource.set("OriginalFilename", "dsh-launcher.exe");
        resource.set("InternalName", "dsh-launcher");
        resource
            .compile()
            .expect("compile Windows launcher resources");
    }
    println!("cargo:rerun-if-changed=../../../packaging/windows/deepseek-black.ico");
}
