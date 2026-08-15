//! `LocalSpillStore`: the host-filesystem implementation of the
//! `dsh-spill` storage seam. Persists a tool's oversized text to a private,
//! session-scoped file (see [`crate::store`] for the traversal-safe naming
//! and exclusive owner-only write) and returns a path locator plus local
//! read/grep retrieval guidance. Rust port of
//! `packages/spill/spill-local/src/index.ts`.

use std::sync::Arc;

use cordis::Context;

use dsh_spill::{SaveTextSpill, SpillRef, SpillStore, spill_locator};

use crate::store::{SaveTextOptions, private_root, save_text_file};

/// Plugin config (all optional — omitted `root` uses the lazy private
/// default).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Root directory for spill files. Omitted uses a lazily-created private
    /// (0700) per-process directory under the OS temp dir — the safe default
    /// for a local deployment. Set it to keep spill files under a known
    /// location.
    pub root: Option<String>,
}

/// Local-filesystem spill backend. Files land under
/// `<root>/session-<hash>/…` with unpredictable names, an exclusive
/// owner-only (0600) write, and a private (0700) root — a spilled tool
/// result must not be readable by other local users or redirectable via a
/// planted symlink.
pub struct LocalSpillStore {
    /// Resolved absolute spill root (config `root`, else the private
    /// default), fixed at construction.
    root: String,
}

impl LocalSpillStore {
    /// Construct the backend, resolve the root, and register the service as
    /// `ctx.spillStore` (the TS `super(ctx)` + constructor collapse).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let root = match config.root {
            Some(root) => std::path::absolute(&root)
                .map_err(|error| format!("cannot resolve spill root '{root}': {error}"))?
                .to_string_lossy()
                .into_owned(),
            None => private_root().to_string(),
        };
        let store = Arc::new(Self { root });
        let erased: Arc<dyn SpillStore> = store.clone();
        ctx.register_service(erased);
        Ok(store)
    }

    /// The resolved absolute spill root (diagnostic surface).
    pub fn root(&self) -> &str {
        &self.root
    }
}

#[async_trait::async_trait]
impl SpillStore for LocalSpillStore {
    async fn save_text(&self, input: &SaveTextSpill) -> Result<SpillRef, String> {
        let saved = save_text_file(SaveTextOptions {
            root: self.root.clone(),
            session_id: input.owner.session_id.to_string(),
            suggested_name: input.suggested_name.clone(),
            content: input.content.clone(),
        })
        .await
        .map_err(|error| format!("cannot save spill text: {error}"))?;
        Ok(SpillRef {
            locator: spill_locator(saved.path),
            bytes: saved.bytes,
            retrieval_hint: "Use read with offset/limit, or grep this path to search within it."
                .to_string(),
        })
    }
}
