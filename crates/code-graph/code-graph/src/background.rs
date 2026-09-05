use crate::{CodeIndex, GraphSnapshot, index::IndexStats};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Default)]
pub struct GraphQuery {
    pub search: String,
    pub selected: String,
    pub mode: String,
    pub path: String,
    pub stats_only: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphView {
    #[serde(flatten)]
    pub graph: GraphSnapshot,
    pub status: String,
    pub stats: IndexStats,
    pub updated_at: u64,
    pub error: Option<String>,
    pub total_symbols: usize,
    pub total_calls: usize,
    pub matched_symbols: usize,
    pub result_limited: bool,
}

struct Job {
    status: String,
    running: bool,
    checked: Instant,
    updated_at: u64,
    snapshot: Option<Arc<GraphSnapshot>>,
    stats: IndexStats,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
}

struct State {
    index: CodeIndex,
    jobs: Mutex<HashMap<PathBuf, Job>>,
    active: AtomicUsize,
}

#[derive(Clone)]
pub struct BackgroundIndex(Arc<State>);
impl Default for BackgroundIndex {
    fn default() -> Self {
        Self(Arc::new(State {
            index: CodeIndex::new(),
            jobs: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
        }))
    }
}

impl BackgroundIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// A cheap HTTP-side admission check. Filesystem walks and parsing run on bounded workers.
    pub fn request(&self, root: &Path, resume: bool) {
        let root = root.to_path_buf();
        let mut jobs = self.0.jobs.lock().unwrap();
        if let Some(job) = jobs.get(&root) {
            if job.running
                || (!resume && (job.status == "cancelled" || job.checked.elapsed().as_secs() < 4))
            {
                return;
            }
        }
        if jobs.len() >= 8 && !jobs.contains_key(&root) {
            if let Some(key) = jobs
                .iter()
                .filter(|(_, job)| !job.running)
                .min_by_key(|(_, job)| job.checked)
                .map(|(key, _)| key.clone())
            {
                jobs.remove(&key);
            } else {
                return;
            }
        }
        let job = jobs.entry(root.clone()).or_insert_with(|| Job {
            status: "queued".into(),
            running: false,
            checked: Instant::now(),
            updated_at: 0,
            snapshot: None,
            stats: IndexStats::default(),
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        if self.0.active.load(Ordering::Acquire) >= 2 {
            job.status = "queued".into();
            return;
        }
        job.running = true;
        job.status = if job.snapshot.is_some() {
            "checking"
        } else {
            "indexing"
        }
        .into();
        job.checked = Instant::now();
        job.cancel = Arc::new(AtomicBool::new(false));
        let cancel = job.cancel.clone();
        let needs_snapshot = job.snapshot.is_none();
        let state = self.0.clone();
        self.0.active.fetch_add(1, Ordering::AcqRel);
        drop(jobs);
        std::thread::spawn(move || {
            let result = state.index.update(&root, &cancel).map(|update| {
                let snapshot = (update.changed || needs_snapshot)
                    .then(|| Arc::new(GraphSnapshot::from_graph(&update.graph, &root)));
                (update, snapshot)
            });
            let mut jobs = state.jobs.lock().unwrap();
            if let Some(job) = jobs.get_mut(&root) {
                job.running = false;
                job.checked = Instant::now();
                if cancel.load(Ordering::Relaxed) {
                    job.status = "cancelled".into();
                } else {
                    match result {
                        Ok((update, snapshot)) => {
                            if let Some(snapshot) = snapshot {
                                job.snapshot = Some(snapshot);
                                job.updated_at = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as u64;
                            }
                            job.status = if update.stats.partial {
                                "partial"
                            } else {
                                "ready"
                            }
                            .into();
                            job.stats = update.stats;
                            job.error = None;
                        }
                        Err(error) => {
                            job.status = "failed".into();
                            job.error = Some(error);
                        }
                    }
                }
            }
            state.active.fetch_sub(1, Ordering::AcqRel);
        });
    }

    pub fn cancel(&self, root: &Path) {
        if let Some(job) = self.0.jobs.lock().unwrap().get_mut(root) {
            job.cancel.store(true, Ordering::Release);
            job.status = "cancelled".into();
        }
    }

    pub fn view(&self, root: &Path, query: &GraphQuery, resume: bool) -> GraphView {
        self.request(root, resume);
        let (snapshot, status, stats, updated_at, error) = {
            let jobs = self.0.jobs.lock().unwrap();
            let job = jobs.get(root);
            (
                job.and_then(|j| j.snapshot.clone()),
                job.map(|j| j.status.clone()).unwrap_or("queued".into()),
                job.map(|j| j.stats.clone()).unwrap_or_default(),
                job.map(|j| j.updated_at).unwrap_or(0),
                job.and_then(|j| j.error.clone()),
            )
        };
        // No graph strings, filtering or projection are copied under the shared
        // jobs lock. The immutable snapshot remains alive through this Arc.
        let empty = empty_graph(0);
        let snapshot = snapshot.as_deref().unwrap_or(&empty);
        let total_symbols = snapshot.symbols.len();
        let total_calls = snapshot.calls.len();
        if query.stats_only {
            return GraphView {
                graph: empty_graph(snapshot.files),
                status,
                stats,
                updated_at,
                error,
                total_symbols,
                total_calls,
                matched_symbols: 0,
                result_limited: false,
            };
        }
        let search = query.search.to_lowercase();
        let selected = &query.selected;
        let mut related = HashSet::new();
        let mut related_limited = false;
        if !selected.is_empty() {
            related.insert(selected.as_str());
            for _ in 0..if matches!(query.mode.as_str(), "chain" | "blast") {
                3
            } else {
                1
            } {
                let previous = related.clone();
                for call in &snapshot.calls {
                    let incoming =
                        query.mode != "callees" && previous.contains(call.target.as_str());
                    let outgoing =
                        query.mode != "callers" && previous.contains(call.source.as_str());
                    if incoming || outgoing {
                        for id in [&call.source, &call.target] {
                            if related.len() < 200 || related.contains(id.as_str()) {
                                related.insert(id.as_str());
                            } else {
                                related_limited = true;
                            }
                        }
                    }
                    if related_limited {
                        break;
                    }
                }
                if related_limited {
                    break;
                }
            }
        }
        let relationship_view = !selected.is_empty()
            && matches!(
                query.mode.as_str(),
                "callers" | "callees" | "chain" | "blast" | "references" | "read"
            );
        let selected_path = snapshot
            .symbols
            .iter()
            .find(|row| row.id == *selected)
            .map(|row| row.path.as_str())
            .unwrap_or("");
        let (symbols, matched_symbols) = take_rows(&snapshot.symbols, 250, |row| {
            if query.mode == "read" {
                return row.id == *selected;
            }
            (!relationship_view || related.contains(row.id.as_str()))
                && (query.path.is_empty() || row.path == query.path)
                && (search.is_empty()
                    || row.name.to_lowercase().contains(&search)
                    || row.path.to_lowercase().contains(&search)
                    || row.id == *selected)
        });
        let shown = symbols
            .iter()
            .map(|s| s.id.as_str())
            .collect::<HashSet<_>>();
        let (calls, matched_calls) = take_rows(&snapshot.calls, 300, |row| {
            if query.mode == "callers" || query.mode == "references" {
                row.target == *selected
            } else if query.mode == "callees" {
                row.source == *selected
            } else {
                shown.contains(row.source.as_str()) && shown.contains(row.target.as_str())
            }
        });
        let (references, matched_references) = take_rows(&snapshot.references, 300, |row| {
            if selected.is_empty() {
                shown.contains(row.target.as_str())
            } else {
                row.target == *selected
            }
        });
        let (deps, matched_deps) = take_rows(&snapshot.deps, 300, |row| {
            (selected_path.is_empty() || row.source == selected_path || row.target == selected_path)
                && (search.is_empty() || row.name.to_lowercase().contains(&search))
        });
        let result_limited = snapshot.truncated
            || related_limited
            || matched_symbols > symbols.len()
            || matched_calls > calls.len()
            || matched_references > references.len()
            || matched_deps > deps.len();
        GraphView {
            graph: GraphSnapshot {
                files: snapshot.files,
                symbols,
                calls,
                references,
                deps,
                truncated: snapshot.truncated,
                engine: snapshot.engine,
            },
            result_limited,
            status,
            stats,
            updated_at,
            error,
            total_symbols,
            total_calls,
            matched_symbols,
        }
    }
}

fn empty_graph(files: usize) -> GraphSnapshot {
    GraphSnapshot {
        files,
        symbols: vec![],
        references: vec![],
        calls: vec![],
        deps: vec![],
        truncated: false,
        engine: "rust-tree-sitter",
    }
}

/// Count matches without allocating them; clone only the bounded response rows.
fn take_rows<T: Clone>(rows: &[T], limit: usize, matches: impl Fn(&T) -> bool) -> (Vec<T>, usize) {
    let mut selected = Vec::with_capacity(limit.min(rows.len()));
    let mut count = 0;
    for row in rows {
        if matches(row) {
            count += 1;
            if selected.len() < limit {
                selected.push(row.clone());
            }
        }
    }
    (selected, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_projection_clones_only_returned_rows() {
        struct Counted(Arc<AtomicUsize>);
        impl Clone for Counted {
            fn clone(&self) -> Self {
                self.0.fetch_add(1, Ordering::Relaxed);
                Self(self.0.clone())
            }
        }
        let clones = Arc::new(AtomicUsize::new(0));
        let rows = (0..50_000)
            .map(|_| Counted(clones.clone()))
            .collect::<Vec<_>>();
        let (returned, total) = take_rows(&rows, 250, |_| true);
        assert_eq!(returned.len(), 250);
        assert_eq!(total, 50_000);
        assert_eq!(clones.load(Ordering::Relaxed), 250);
    }

    #[test]
    fn selecting_a_late_symbol_queries_full_snapshot_but_returns_one_row() {
        let root = PathBuf::from("projection-only-fixture");
        let index = BackgroundIndex::new();
        let mut snapshot = empty_graph(1);
        snapshot.symbols = (0..50_000)
            .map(|value| crate::GraphSymbol {
                id: format!("s{value}"),
                name: format!("f{value}"),
                path: "dense.rs".into(),
                line: value + 1,
                end_line: value + 1,
                kind: "function".into(),
            })
            .collect();
        index.0.jobs.lock().unwrap().insert(
            root.clone(),
            Job {
                status: "ready".into(),
                running: false,
                checked: Instant::now(),
                updated_at: 1,
                snapshot: Some(Arc::new(snapshot)),
                stats: IndexStats::default(),
                error: None,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        let view = index.view(
            &root,
            &GraphQuery {
                selected: "s49999".into(),
                mode: "read".into(),
                ..Default::default()
            },
            false,
        );
        assert_eq!(view.total_symbols, 50_000);
        assert_eq!(view.graph.symbols.len(), 1);
        assert_eq!(view.graph.symbols[0].name, "f49999");
        let status = index.view(
            &root,
            &GraphQuery {
                stats_only: true,
                ..Default::default()
            },
            false,
        );
        assert_eq!(status.total_symbols, 50_000);
        assert!(status.graph.symbols.is_empty());
    }
    #[test]
    fn async_index_keeps_workspace_results_and_can_pause_and_resume() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn run() { helper(); } fn helper() {}\n",
        )
        .unwrap();
        let index = BackgroundIndex::new();
        let query = GraphQuery::default();
        let start = Instant::now();
        loop {
            let view = index.view(dir.path(), &query, false);
            if view.status == "ready" {
                assert_eq!(view.total_symbols, 2);
                break;
            }
            assert!(start.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        index.cancel(dir.path());
        assert_eq!(index.view(dir.path(), &query, false).status, "cancelled");
        let resumed = index.view(dir.path(), &query, true);
        assert_eq!(resumed.total_symbols, 2);
        assert_ne!(resumed.status, "cancelled");
    }
}
