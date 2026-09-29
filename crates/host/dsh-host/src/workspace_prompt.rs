//! Keep client maintenance instructions local to an actual source workspace.
use std::path::Path;

const DOCUMENT_GUIDANCE: &str = "For document or table tasks, use exactly the objects and row count requested by the user. Read Office content through structured document tools and copy original strings, including phone numbers, verbatim. Claim source consistency only after an actual item-by-item comparison. If execution is blocked, promptly provide any useful text result and explain the blocker. Do not switch to a remote terminal or browser to bypass a local permission failure; those capabilities remain available for tasks that actually require them.";

fn source_workspace(cwd: Option<&str>, source: Option<&Path>) -> bool {
    let Some((cwd, source)) = cwd
        .and_then(|cwd| std::fs::canonicalize(cwd).ok())
        .zip(source.and_then(|root| std::fs::canonicalize(root).ok()))
    else {
        return false;
    };
    cwd.starts_with(&source)
        && (cwd == source || cwd.join("Cargo.toml").is_file() || cwd.join("package.json").is_file())
}
pub(super) fn source_hint(cwd: Option<&str>, source: Option<&Path>) -> String {
    if !source_workspace(cwd, source) {
        return String::new();
    }
    format!(
        "The DeepSeek Harness implementation checkout for this session is {}. Use the session workspace as the working directory; this location is for inspecting or extending DSH itself.",
        source.unwrap().display()
    )
}
pub(super) fn client_hint(cwd: Option<&str>, source: Option<&Path>, port: u16) -> String {
    let base = "This conversation is hosted by DeepSeek Harness. The client provides no implicit screenshot or DOM context.";
    if !source_workspace(cwd, source) {
        return format!("{base}\n{DOCUMENT_GUIDANCE}");
    }
    format!(
        "{base} The existing Web GUI is http://127.0.0.1:{port}. Client-plugin HMR reloads changes only while pnpm run dev:web rebuilds bundles from this checkout; verify that watcher before promising automatic updates. Changes to the apps/web shell or plain packages require rebuilding their Web artifacts and verifying this existing URL after a refresh. Starting another server does not update this GUI. The apps/web Vite entry requires the Host-injected window.__DSH_BOOT__. Start another managed preview server only when the user's task calls for one, and verify its exact URL."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_sessions_do_not_inherit_the_hosts_source_workspace() {
        let root =
            std::env::temp_dir().join(format!("dsh-workspace-prompt-{}", uuid::Uuid::new_v4()));
        let source = root.join("harness");
        let docs = root.join("documents");
        let nested_docs = source.join("docs");
        let crate_dir = source.join("crates/host");
        for folder in [&source, &docs, &nested_docs, &crate_dir] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(crate_dir.join("Cargo.toml"), "[package]\nname='fixture'").unwrap();
        for cwd in [
            Some(docs.to_str().unwrap()),
            Some(nested_docs.to_str().unwrap()),
            None,
        ] {
            assert!(source_hint(cwd, Some(&source)).is_empty());
            let text = client_hint(cwd, Some(&source), 58080);
            for unwanted in ["HMR", "pnpm", "apps/web", source.to_str().unwrap()] {
                assert!(!text.contains(unwanted), "{unwanted}");
            }
            assert!(
                text.contains("row count")
                    && text.contains("phone numbers")
                    && text.contains("item-by-item")
                    && text.contains("useful text result")
            );
        }
        assert!(client_hint(Some(source.to_str().unwrap()), Some(&source), 58080).contains("HMR"));
        assert!(
            source_hint(Some(crate_dir.to_str().unwrap()), Some(&source))
                .contains(source.to_str().unwrap())
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
