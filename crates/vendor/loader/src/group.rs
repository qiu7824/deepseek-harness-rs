//! Loader entry groups (port of `src/config/group.ts`).

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{ArcValue, Context, EventOptions, Plugin, PluginError};
use parking_lot::Mutex;
use serde_json::Value;

use crate::entry::{Entry, EntryOptions};
use crate::loader::{LoaderCore, LoaderError};
use crate::tree::EntryTree;

/// Runtime owner for a list of child loader entries.
pub struct EntryGroup {
    /// The group context (tree ctx for the root group, entry ctx for
    /// subgroups).
    pub ctx: Context,
    /// The tree this group belongs to (shared with subgroups).
    pub tree: Arc<EntryTree>,
    /// The owning entry when this group is a subgroup.
    pub owner: Mutex<Option<Arc<Entry>>>,
    /// Serialized child entry options in order.
    pub data: Mutex<Vec<EntryOptions>>,
}

impl EntryGroup {
    pub fn new(ctx: Context, tree: Arc<EntryTree>, owner: Option<Arc<Entry>>) -> Arc<Self> {
        let group = Arc::new(Self {
            ctx,
            tree,
            owner: Mutex::new(owner.clone()),
            data: Mutex::new(Vec::new()),
        });
        if let Some(entry) = owner {
            *entry.subgroup.lock() = Some(group.clone());
        }
        group
    }

    /// The entry that owns this subgroup, if any (TS walks `ctx.fiber.entry`).
    pub fn owner_entry(&self) -> Option<Arc<Entry>> {
        self.owner.lock().clone()
    }

    /// Create (or re-home) one entry in this group (TS `create`).
    pub async fn create(
        self: &Arc<Self>,
        mut options: EntryOptions,
    ) -> Result<String, LoaderError> {
        let id = self.tree.ensure_id(&mut options);
        let existing = self.tree.store.lock().get(&id).cloned();
        let entry = match &existing {
            Some(entry) => entry.clone(),
            None => {
                let entry = Entry::new(self.tree.core.clone(), self.ctx.clone());
                self.tree.store.lock().insert(id.clone(), entry.clone());
                entry
            }
        };
        let previous_parent = entry.parent.lock().clone();
        *entry.parent.lock() = Some(self.clone());
        match entry.replace_options(options).await {
            Ok(()) => {}
            Err(error) => {
                *entry.parent.lock() = previous_parent.clone();
                if previous_parent.is_none() {
                    self.tree.store.lock().remove(&id);
                }
                return Err(error);
            }
        }
        Ok(id)
    }

    /// Remove an entry option record from the group data list (TS `unlink`).
    pub fn unlink(&self, options: &EntryOptions) {
        let mut data = self.data.lock();
        if let Some(index) = data.iter().position(|item| item.id == options.id) {
            data.remove(index);
        }
    }

    /// Stop and remove an entry (TS `remove`).
    pub async fn remove(&self, id: &str, is_dispose: bool) -> Result<(), LoaderError> {
        let entry = self.tree.store.lock().get(id).cloned();
        let Some(entry) = entry else { return Ok(()) };
        let result = entry.dispose_fiber().await;
        if !is_dispose {
            let options = entry.options.lock().clone();
            self.unlink(&options);
        }
        self.tree.store.lock().remove(id);
        let options = entry.options.lock().clone();
        self.ctx.emit(
            "loader/partial-dispose",
            vec![cordis::arc(entry), cordis::arc(options), cordis::arc(false)],
        );
        result
    }

    /// Reconcile the group's entry list with a new config (TS `update`).
    pub async fn update(self: &Arc<Self>, config: Vec<EntryOptions>) -> Result<(), LoaderError> {
        let old_config = self.data.lock().clone();
        let mut seen = std::collections::HashSet::new();
        for options in &config {
            if options.id.is_empty() {
                continue; // ids are ensured below via create()
            }
            if !seen.insert(options.id.clone()) {
                return Err(LoaderError::Import(format!(
                    "duplicate loader entry id: {}",
                    options.id
                )));
            }
        }
        let old_map: HashMap<String, EntryOptions> = old_config
            .iter()
            .cloned()
            .map(|options| (options.id.clone(), options))
            .collect();
        let new_map: HashMap<String, EntryOptions> = config
            .iter()
            .cloned()
            .map(|options| (options.id.clone(), options))
            .collect();

        let mut errors: Vec<LoaderError> = Vec::new();
        for options in config.clone() {
            if let Err(error) = self.create(options).await {
                errors.push(error);
            }
        }
        if errors.len() == 1 {
            return Err(errors.remove(0));
        }
        if errors.len() > 1 {
            return Err(LoaderError::Aggregate(errors));
        }
        for id in old_map.keys() {
            if !new_map.contains_key(id)
                && let Err(error) = self.remove(id, true).await
            {
                errors.push(error);
            }
        }
        if !errors.is_empty() {
            return Err(LoaderError::Aggregate(errors));
        }
        *self.data.lock() = config;
        Ok(())
    }

    /// Stop every child entry (TS `stop`).
    pub async fn stop(self: &Arc<Self>) -> Result<(), LoaderError> {
        let ids: Vec<String> = self
            .data
            .lock()
            .iter()
            .map(|options| options.id.clone())
            .collect();
        let mut errors = Vec::new();
        for id in ids {
            if let Err(error) = self.remove(&id, true).await {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(LoaderError::Aggregate(errors))
        }
    }
}

/// Plugin that mounts a nested loader entry group (TS `Group`).
pub struct GroupPlugin {
    pub core: Arc<LoaderCore>,
}

#[async_trait::async_trait]
impl Plugin for GroupPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("group")
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let entry = self.core.entry_of(&ctx.fiber).ok_or_else(|| {
            PluginError::new(cordis::arc(
                "group plugin requires a loader entry".to_string(),
            ))
        })?;
        let tree = entry
            .parent
            .lock()
            .as_ref()
            .map(|g| g.tree.clone())
            .ok_or_else(|| {
                PluginError::new(cordis::arc("group entry has no parent tree".to_string()))
            })?;
        let group = EntryGroup::new(ctx.clone(), tree, Some(entry.clone()));
        *entry.subgroup.lock() = Some(group.clone());

        // internal/update: reconcile child entries when the group config
        // changes (TS `Group` ctor listener).
        let group_for_listener = group.clone();
        ctx.on(
            "internal/update",
            Arc::new(move |_ctx: &Context, args: Vec<ArcValue>| {
                let group = group_for_listener.clone();
                Box::pin(async move {
                    let next_result = match args
                        .last()
                        .and_then(|v| cordis::downcast::<cordis::NextFn>(v))
                    {
                        Some(next) => next.call().await,
                        None => cordis::arc(()),
                    };
                    if let Some(config) = args.first().and_then(|v| cordis::downcast::<Value>(v))
                        && let Ok(entries) =
                            serde_json::from_value::<Vec<EntryOptions>>(config.clone())
                        && let Err(error) = group.update(entries).await
                    {
                        return Some(cordis::arc(PluginError::new(cordis::arc(
                            error.to_string(),
                        ))));
                    }
                    Some(next_result)
                })
            }),
            EventOptions::default(),
        )
        .await;

        let raw_config = cordis::downcast::<Value>(&config)
            .cloned()
            .unwrap_or(Value::Null);
        let entries: Vec<EntryOptions> = serde_json::from_value(raw_config).unwrap_or_default();
        group
            .update(entries)
            .await
            .map_err(|error| PluginError::new(cordis::arc(error.to_string())))?;
        Ok(())
    }
}
