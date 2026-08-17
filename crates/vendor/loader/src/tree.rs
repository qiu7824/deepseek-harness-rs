//! Loader entry tree (port of `src/config/tree.ts`).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cordis::{Context, Plugin};
use parking_lot::Mutex;

use crate::entry::{Entry, EntryOptions};
use crate::group::EntryGroup;
use crate::loader::{LoaderCore, LoaderError};

static NEXT_ENTRY_ID: AtomicU64 = AtomicU64::new(0);

/// Mutable tree of loader entries. Persistence is supplied by a pluggable
/// `write` backend (the include plugin provides the file-backed tree).
pub struct EntryTree {
    pub sep: &'static str,
    /// The tree context (loader plugin ctx for the root tree).
    pub ctx: Context,
    pub core: Arc<LoaderCore>,
    /// The root group (late-bound: the group and tree reference each other).
    pub root: std::sync::OnceLock<Arc<EntryGroup>>,
    /// All entries of this tree, by id.
    pub store: Mutex<HashMap<String, Arc<Entry>>>,
    /// Owner entry id when this tree is nested (include-provided).
    pub owner_entry_id: Option<String>,
    /// Persistence callback (TS abstract `write()`; loader root = no-op).
    write_fn: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Opaque owner handle slot (the include plugin stores itself here so
    /// hosts can reach `refresh()`; HMR will use the same slot).
    pub extras: Mutex<Option<Arc<dyn std::any::Any + Send + Sync>>>,
}

impl EntryTree {
    pub fn new(ctx: Context, core: Arc<LoaderCore>) -> Arc<Self> {
        let owner_entry = core.entry_of(&ctx.fiber);
        let owner_entry_id = owner_entry.as_ref().map(|entry| entry.id());
        let tree = Arc::new(Self {
            sep: ":",
            ctx: ctx.clone(),
            core,
            root: std::sync::OnceLock::new(),
            store: Mutex::new(HashMap::new()),
            owner_entry_id,
            write_fn: Mutex::new(None),
            extras: Mutex::new(None),
        });
        let group = EntryGroup::new(ctx, tree.clone(), None);
        let _ = tree.root.set(group);
        if let Some(owner) = owner_entry {
            *owner.subtree.lock() = Some(tree.clone());
        }
        tree
    }

    /// The root group.
    pub fn root_group(&self) -> Arc<EntryGroup> {
        self.root.get().expect("root group initialized").clone()
    }

    /// Set the persistence callback (TS subclasses override `write()`).
    pub fn set_write_backend(&self, write: Arc<dyn Fn() + Send + Sync>) {
        *self.write_fn.lock() = Some(write);
    }

    /// Persist the current tree state (no-op when no backend is installed).
    pub fn write(&self) {
        if let Some(write) = self.write_fn.lock().clone() {
            write();
        }
    }

    /// Iterate entries in this tree (TS `entries()`; nested subtrees are
    /// iterated once the include plugin lands).
    pub fn entries(&self) -> Vec<Arc<Entry>> {
        self.store.lock().values().cloned().collect()
    }

    /// Wait until the tree has no live fibers and rethrow failures
    /// (TS `await()`).
    pub async fn await_ready(&self) -> Result<(), LoaderError> {
        for _round in 0..1000 {
            let fibers: Vec<Arc<cordis::FiberCore>> = self
                .entries()
                .into_iter()
                .filter_map(|entry| entry.fiber.lock().clone())
                .collect();
            if fibers.is_empty() {
                break;
            }
            let mut errors: Vec<LoaderError> = Vec::new();
            for fiber in fibers {
                if let Err(error) = fiber.settle().await {
                    errors.push(LoaderError::Import(error.message()));
                }
            }
            if errors.len() == 1 {
                return Err(errors.remove(0));
            }
            if errors.len() > 1 {
                return Err(LoaderError::Aggregate(errors));
            }
        }
        self.ctx
            .reflect
            .notify(&self.ctx, vec!["loader".to_string()]);
        Ok(())
    }

    /// Allocate a unique entry id when missing (TS `ensureId`).
    pub fn ensure_id(&self, options: &mut EntryOptions) -> String {
        if options.id.is_empty() {
            loop {
                let id = format!("{:08x}", NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed));
                if !self.store.lock().contains_key(&id) {
                    options.id = id;
                    break;
                }
            }
        }
        options.id.clone()
    }

    /// Resolve an entry by id, including nested ids separated by the tree
    /// separator (TS `resolve`).
    pub fn resolve(&self, id: &str) -> Result<Arc<Entry>, LoaderError> {
        let mut parts: Vec<&str> = id.split(self.sep).collect();
        let final_part = parts.pop().unwrap_or_default();
        let mut chain: Vec<Arc<EntryTree>> = Vec::new();
        let mut current: &EntryTree = self;
        for part in parts {
            let entry = current
                .store
                .lock()
                .get(part)
                .cloned()
                .ok_or_else(|| LoaderError::Import(format!("cannot resolve entry {id}")))?;
            let subtree = entry
                .subtree
                .lock()
                .clone()
                .ok_or_else(|| LoaderError::Import(format!("cannot resolve entry {id}")))?;
            chain.push(subtree);
            current = chain.last().expect("subtree just pushed");
        }
        current
            .store
            .lock()
            .get(final_part)
            .cloned()
            .ok_or_else(|| LoaderError::Import(format!("cannot resolve entry {id}")))
    }
    /// Resolve a group by owner entry id (TS `resolveGroup`).
    pub fn resolve_group(&self, id: Option<&str>) -> Result<Arc<EntryGroup>, LoaderError> {
        match id {
            None => Ok(self.root_group()),
            Some(id) => {
                let entry = self.resolve(id)?;
                let subgroup = entry.subgroup.lock().clone();
                subgroup.ok_or_else(|| LoaderError::Import(format!("entry {id} is not a group")))
            }
        }
    }

    /// Create an entry in the root group or a nested group (TS `create`).
    pub async fn create(
        &self,
        options: EntryOptions,
        parent: Option<&str>,
        position: Option<usize>,
    ) -> Result<String, LoaderError> {
        let group = self.resolve_group(parent)?;
        let id = group.create(options).await?;
        let entry = self.resolve(&id)?;
        let options = entry.options.lock().clone();
        let mut data = group.data.lock();
        let len = data.len();
        let position = position.unwrap_or(len);
        data.insert(position.min(len), options);
        drop(data);
        group.tree.write();
        Ok(id)
    }

    /// Stop and remove an entry from its parent group (TS `remove`).
    pub async fn remove(&self, id: &str) -> Result<(), LoaderError> {
        let entry = self.resolve(id)?;
        let parent = entry
            .parent
            .lock()
            .clone()
            .ok_or_else(|| LoaderError::Import(format!("entry {id} has no parent group")))?;
        // Never extend a lock guard across an await (same-thread reentrant
        // deadlock): extract the id first.
        let entry_id = entry.options.lock().id.clone();
        parent.remove(&entry_id, false).await?;
        parent.tree.write();
        Ok(())
    }

    /// Import a plugin from the static registry (TS `EntryTree.import`).
    pub fn import(&self, name: &str) -> Result<Arc<dyn Plugin>, LoaderError> {
        self.core.import(name)
    }
}
