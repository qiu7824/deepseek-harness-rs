//! One authoritative Host task domain and a compact due-task index.
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use dsh_storage_domain::{Domain, DomainFacility, KvTable, define_domain, domain_table};
use parking_lot::RwLock;

use crate::calendar::parse_instant;
use crate::host_history::validate_task;
use crate::host_types::{HostScheduleTask, ScheduleError, TaskStatus};

#[derive(Debug, Clone)]
pub(crate) struct TaskIndex {
    pub id: String,
    pub session_id: String,
    pub scheduled_at: i64,
    pub active: bool,
    pub recurring: bool,
    pub revision: u64,
}

#[derive(Default)]
struct Index {
    tasks: indexmap::IndexMap<String, TaskIndex>,
    due: BTreeSet<(i64, String)>,
    revision: u64,
}

pub(crate) struct HostStore {
    domain: Arc<Domain>,
    tasks: Arc<dyn KvTable>,
    index: RwLock<Index>,
}

impl HostStore {
    pub async fn open(facility: &DomainFacility) -> Result<Arc<Self>, ScheduleError> {
        let specification = define_domain(
            "schedule",
            1,
            None,
            indexmap::indexmap! {
                "tasks".to_owned() => domain_table(Arc::new(|raw| {
                    validate_task(raw).map(|_| ()).map_err(|error| error.to_string())
                })),
            },
        )
        .map_err(storage_error)?;
        let domain = facility.open(&specification).await.map_err(storage_error)?;
        let tasks = domain.table("tasks");
        let store = Arc::new(Self {
            domain,
            tasks,
            index: RwLock::new(Index::default()),
        });
        // Build a timer index without cloning the complete prompt/history catalog.
        for key in store.tasks.keys() {
            let result = store.read(&key).and_then(|task| {
                let task = task.ok_or_else(|| {
                    ScheduleError::corrupt("Task disappeared during domain initialization.")
                })?;
                if key != task.record.id() {
                    return Err(ScheduleError::corrupt(format!(
                        "Stored task key {key:?} differs from its record id."
                    )));
                }
                store.publish(&task)
            });
            if let Err(error) = result {
                store.domain.close().await;
                return Err(error);
            }
        }
        Ok(store)
    }

    pub fn read(&self, id: &str) -> Result<Option<HostScheduleTask>, ScheduleError> {
        self.tasks.get(id).as_ref().map(validate_task).transpose()
    }

    pub fn snapshot(&self) -> Vec<TaskIndex> {
        let index = self.index.read();
        let mut values: Vec<_> = index.tasks.values().cloned().collect();
        values.sort_by(|left, right| {
            (left.scheduled_at, &left.id).cmp(&(right.scheduled_at, &right.id))
        });
        values
    }

    pub fn active_for_session(&self, session_id: &str) -> Vec<TaskIndex> {
        self.index
            .read()
            .tasks
            .values()
            .filter(|entry| entry.active && entry.session_id == session_id)
            .cloned()
            .collect()
    }

    pub fn matches(&self, expected: &TaskIndex) -> bool {
        self.index
            .read()
            .tasks
            .get(&expected.id)
            .is_some_and(|current| current.revision == expected.revision)
    }

    pub fn due(&self, now: i64, failed: &HashSet<String>) -> Vec<TaskIndex> {
        let index = self.index.read();
        let mut due: Vec<_> = index
            .due
            .iter()
            .take_while(|(target, _)| *target <= now)
            .filter(|(_, id)| !failed.contains(id))
            .filter_map(|(_, id)| index.tasks.get(id).cloned())
            .collect();
        due.sort_by_key(|entry| index.tasks.get_index_of(&entry.id));
        due
    }

    pub fn next_target(&self, failed: &HashSet<String>) -> Option<i64> {
        self.index
            .read()
            .due
            .iter()
            .find(|(_, id)| !failed.contains(id))
            .map(|(target, _)| *target)
    }

    pub async fn put(&self, task: &HostScheduleTask) -> Result<(), ScheduleError> {
        let raw = serde_json::to_value(task).map_err(|error| storage_error(error.to_string()))?;
        validate_task(&raw)?;
        self.tasks
            .put(task.record.id(), raw)
            .await
            .map_err(storage_error)?;
        self.publish(task)
    }

    fn publish(&self, task: &HostScheduleTask) -> Result<(), ScheduleError> {
        let target = parse_instant(task.record.scheduled_at())?;
        let mut index = self.index.write();
        if let Some(previous) = index
            .tasks
            .get(task.record.id())
            .map(|entry| (entry.scheduled_at, entry.id.clone()))
        {
            index.due.remove(&previous);
        }
        index.revision = index.revision.wrapping_add(1);
        let entry = TaskIndex {
            id: task.record.id().to_owned(),
            session_id: task.session_id.clone(),
            scheduled_at: target,
            active: task.status == TaskStatus::Active,
            recurring: task.record.is_recurring(),
            revision: index.revision,
        };
        if entry.active {
            index.due.insert((entry.scheduled_at, entry.id.clone()));
        }
        index.tasks.insert(entry.id.clone(), entry);
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<bool, ScheduleError> {
        let deleted = self.tasks.delete(id).await.map_err(storage_error)?;
        if deleted {
            let mut index = self.index.write();
            if let Some(previous) = index.tasks.shift_remove(id) {
                index.due.remove(&(previous.scheduled_at, previous.id));
            }
        }
        Ok(deleted)
    }

    pub async fn close(&self) {
        self.domain.close().await;
    }
}

fn storage_error(message: impl Into<String>) -> ScheduleError {
    ScheduleError::new("persistence_uncertain", message)
}
