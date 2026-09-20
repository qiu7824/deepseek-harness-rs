//! Installed tool discovery shared by foreground commands, probes and PTYs.
//! No shell initialization scripts are executed and no machine settings change.

#[cfg(windows)]
fn msvc() -> &'static Vec<(String, String)> {
    static ENV: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        let target = match std::env::consts::ARCH {
            "aarch64" => "aarch64-pc-windows-msvc",
            "x86" => "i686-pc-windows-msvc",
            _ => "x86_64-pc-windows-msvc",
        };
        find_msvc_tools::find_tool(target, "link.exe")
            .map(|tool| {
                tool.env()
                    .into_iter()
                    .filter_map(|(name, value)| {
                        let name = name.to_string_lossy().to_ascii_uppercase();
                        matches!(name.as_str(), "PATH" | "LIB" | "LIBPATH" | "INCLUDE")
                            .then(|| (name, value.to_string_lossy().into_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

pub fn environment() -> Vec<(String, String)> {
    #[cfg(windows)]
    {
        msvc()
            .iter()
            .filter(|(name, _)| name == "PATH" || std::env::var_os(name).is_none())
            .cloned()
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
