fn main() {
    #[cfg(windows)]
    {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packaging/windows/deepseek-black.ico");
        winresource::WindowsResource::new()
            .set_icon(&path.to_string_lossy())
            .set("ProductName", "DeepSeek Harness Desktop")
            .set("FileDescription", "DeepSeek Harness Native Desktop")
            .compile()
            .expect("desktop icon resource");
    }
    println!("cargo:rerun-if-changed=../../../packaging/windows/deepseek-black.ico");
}
