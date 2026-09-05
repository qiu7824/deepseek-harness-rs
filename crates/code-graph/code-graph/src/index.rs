//! `build_graph(root)` + the shared, lazily-built [`CodeIndex`] the graph tools hold.
//! Ported from production `graph/indexer.rs` (the build + call-resolution; the
//! background/incremental indexer + CPU throttling are replaced by a simpler
//! build-once-then-rebuild-on-mtime-change cache — correct first, optimize later).

use super::graph::{CodeGraph, Edge, EdgeKind, SymbolId, SymbolKind, SymbolNode, Visibility};
use super::imports::{Import, ImportResolver};
use super::lang::Lang;
use super::symbols::{Symbol, extract_symbols_from_tree, parse_tree};
use ignore::WalkBuilder;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::time::UNIX_EPOCH;
use tree_sitter::{Query, QueryCursor, QueryCursorOptions, StreamingIterator, Tree};

/// Map a tree-sitter node-kind string to a [`SymbolKind`]. From production
/// `classify_symbol_kind`.
fn classify_symbol_kind(ts: &str) -> SymbolKind {
    match ts {
        "function_item"
        | "function_definition"
        | "function_declaration"
        | "func_literal"
        | "arrow_function"
        | "function_expression" => SymbolKind::Function,
        "method_definition" | "method_declaration" => SymbolKind::Method,
        "struct_item" | "struct_specifier" | "struct_type" => SymbolKind::Struct,
        "class_definition" | "class_declaration" | "class_specifier" | "class" => SymbolKind::Class,
        "trait_item" => SymbolKind::Trait,
        "interface_declaration" | "interface_type" => SymbolKind::Interface,
        "enum_item" | "enum_declaration" | "enum_specifier" => SymbolKind::Enum,
        "const_item" | "const_declaration" => SymbolKind::Constant,
        "let_declaration" | "variable_declaration" | "static_item" => SymbolKind::Variable,
        "mod_item" | "module" => SymbolKind::Module,
        "use_declaration" | "import_statement" | "import_declaration" => SymbolKind::Import,
        "type_item" | "type_alias_declaration" => SymbolKind::TypeAlias,
        "impl_item" => SymbolKind::Other("impl".to_string()),
        other => SymbolKind::Other(other.to_string()),
    }
}

#[derive(Clone)]
struct RawCall {
    caller_name: String,
    /// The exact definition byte offset also separates same-named, same-line callers.
    caller_byte: usize,
    callee_name: String,
    line: usize,
}

/// Byte-accurate enclosing-function lookup built once per file. Each boundary
/// update is O(log symbols); each call lookup is O(log boundaries).
struct CallerIndex(Vec<(usize, Option<usize>)>);
impl CallerIndex {
    fn new(symbols: &[Symbol], stop: &dyn Fn() -> bool) -> Option<Self> {
        use std::cmp::Reverse;
        let mut events = Vec::new();
        for (index, symbol) in symbols.iter().enumerate() {
            if stop() {
                return None;
            }
            if matches!(
                classify_symbol_kind(&symbol.kind),
                SymbolKind::Function | SymbolKind::Method
            ) {
                events.push((symbol.start_byte, true, index));
                events.push((symbol.end_byte, false, index));
            }
        }
        events.sort_unstable();
        let mut active = std::collections::BTreeSet::new();
        let mut boundaries = Vec::new();
        let mut at = 0;
        while at < events.len() {
            if stop() {
                return None;
            }
            let position = events[at].0;
            while at < events.len() && events[at].0 == position {
                let (_, start, index) = events[at];
                let symbol = &symbols[index];
                let key = (symbol.start_byte, Reverse(symbol.end_byte), index);
                if start {
                    active.insert(key);
                } else {
                    active.remove(&key);
                }
                at += 1;
            }
            let owner = active.last().map(|(_, _, index)| *index);
            if boundaries
                .last()
                .is_none_or(|(_, previous)| *previous != owner)
            {
                boundaries.push((position, owner));
            }
        }
        Some(Self(boundaries))
    }
    fn at(&self, byte: usize) -> Option<usize> {
        let offset = self.0.partition_point(|(start, _)| *start <= byte);
        offset.checked_sub(1).and_then(|index| self.0[index].1)
    }
}

fn extract_calls(
    source: &str,
    lang: Lang,
    tree: &Tree,
    syms: &[Symbol],
    stop: &dyn Fn() -> bool,
) -> Option<Vec<RawCall>> {
    if stop() {
        return None;
    }
    let Some(q_src) = lang.calls_query() else {
        return Some(Vec::new());
    };
    let grammar = lang.grammar();
    let query = Query::new(&grammar, q_src).ok()?;
    let callee_idx = query.capture_index_for_name("callee")?;
    let owners = CallerIndex::new(syms, stop)?;
    let mut cursor = QueryCursor::new();
    let mut calls = Vec::new();
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        if stop() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let mut matches = cursor.matches_with_options(
        &query,
        tree.root_node(),
        source.as_bytes(),
        QueryCursorOptions::new().progress_callback(&mut progress),
    );
    loop {
        if stop() {
            return None;
        }
        matches.advance();
        let Some(m) = matches.get() else {
            break;
        };
        for cap in m.captures {
            if cap.index != callee_idx {
                continue;
            }
            let Some(caller) = owners.at(cap.node.start_byte()).map(|index| &syms[index]) else {
                continue;
            };
            let callee_name = source[cap.node.start_byte()..cap.node.end_byte()].to_string();
            if caller.name != callee_name {
                calls.push(RawCall {
                    caller_name: caller.name.clone(),
                    caller_byte: caller.start_byte,
                    callee_name,
                    line: cap.node.start_position().row + 1,
                });
            }
        }
    }
    if stop() { None } else { Some(calls) }
}

fn parse_file(
    path: &Path,
    source: &str,
    stop: &dyn Fn() -> bool,
) -> Option<(Vec<SymbolNode>, Vec<RawCall>)> {
    if stop() {
        return None;
    }
    let component = path
        .extension()
        .is_some_and(|x| x == "vue" || x == "svelte");
    let masked = component.then(|| super::imports::script_source(source));
    let source = masked.as_deref().unwrap_or(source);
    let lang = if component {
        Lang::TypeScript
    } else {
        Lang::detect(path)?
    };
    if !lang.is_indexed() {
        return None;
    }
    let tree = parse_tree(source, lang, stop)?;
    let raw = extract_symbols_from_tree(source, lang, &tree, stop)?;
    let mut nodes = Vec::with_capacity(raw.len());
    for symbol in &raw {
        if stop() {
            return None;
        }
        nodes.push(SymbolNode {
            id: CodeGraph::make_id_at(path, &symbol.name, symbol.start_byte),
            name: symbol.name.clone(),
            kind: classify_symbol_kind(&symbol.kind),
            visibility: Visibility::Unknown,
            file: path.to_path_buf(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            signature: None,
        });
    }
    let calls = extract_calls(source, lang, &tree, &raw, stop)?;
    Some((nodes, calls))
}

fn bounded_source(reader: impl Read) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    reader.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

/// Extensions walked into the graph (matches production's INDEXED set + variants).
const INDEXED_EXTS: &[&str] = &[
    "rs", "py", "js", "jsx", "mjs", "cjs", "ts", "mts", "tsx", "go", "java", "c", "h", "cc", "cpp",
    "cxx", "hpp", "hh", "vue", "svelte",
];

/// A walked source file + the inputs to its staleness fingerprint.
struct Walked {
    path: PathBuf,
    /// mtime in NANOSECONDS — coarse whole seconds would miss a same-second edit and
    /// serve a stale graph.
    mtime_ns: u128,
    /// file length — defends against a same-instant edit whose mtime didn't move (content
    /// length almost always changes on a real edit).
    len: u64,
}

/// Walk `root` (assumed already canonical) for indexable source files + staleness inputs.
const MAX_FILES: usize = 10_000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub indexed_files: usize,
    pub parsed_files: usize,
    pub reused_files: usize,
    pub skipped_files: usize,
    pub unresolved_calls: usize,
    pub partial: bool,
    pub reasons: Vec<String>,
    pub elapsed_ms: u64,
}

fn collect_files(
    root: &Path,
    cancel: &AtomicBool,
    started: Instant,
    stats: &mut IndexStats,
) -> Result<Vec<Walked>, String> {
    let mut out = Vec::new();
    let mut bytes = 0u64;
    for entry in WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            !entry.file_type().is_some_and(|t| t.is_dir())
                || !matches!(
                    entry.file_name().to_str(),
                    Some("node_modules" | ".git" | "target" | "dist" | ".dsh")
                )
        })
        .build()
    {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if started.elapsed().as_secs() >= 15 {
            stats.partial = true;
            stats.reasons.push("索引达到时间预算".into());
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                stats.skipped_files += 1;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let p = entry.path();
        if !p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| INDEXED_EXTS.contains(&e))
        {
            continue;
        }
        let md = match entry.metadata() {
            Ok(md) => md,
            Err(_) => {
                stats.skipped_files += 1;
                continue;
            }
        };
        if md.len() > MAX_FILE_BYTES {
            stats.skipped_files += 1;
            continue;
        }
        if out.len() >= MAX_FILES || bytes.saturating_add(md.len()) > MAX_TOTAL_BYTES {
            stats.partial = true;
            stats.reasons.push("索引达到文件数量或总字节预算".into());
            break;
        }
        bytes += md.len();
        out.push(Walked {
            path: p.to_path_buf(),
            mtime_ns: md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            len: md.len(),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn fingerprint(files: &[Walked]) -> u64 {
    let mut h = DefaultHasher::new();
    for w in files {
        w.path.hash(&mut h);
        w.mtime_ns.hash(&mut h);
        w.len.hash(&mut h);
    }
    h.finish()
}

fn top_component(p: &Path, root: &Path) -> Option<std::ffi::OsString> {
    p.strip_prefix(root)
        .ok()?
        .components()
        .next()
        .map(|c| c.as_os_str().to_os_string())
}

/// Resolve a callee name to a symbol id, preferring closer candidates (production
/// scoring): same file (4) > same dir (2) > same top-level component (1) > any (0).
/// (Import-based score 3 is omitted — like production, we do not parse imports yet.)
/// Ties are broken DETERMINISTICALLY by the smallest (file, start_line) — production's
/// tie-break depends on HashMap iteration order, which is not reproducible.
fn resolve_callee(
    g: &CodeGraph,
    callee: &str,
    caller_file: &Path,
    root: &Path,
    imported: &[(Import, PathBuf)],
) -> Option<(SymbolId, String)> {
    let mut imports = Vec::new();
    for (import, path) in imported {
        for (local, original) in &import.bindings {
            if local == callee || local == "*" {
                let name = if original == "*" { callee } else { original };
                imports.extend(g.find_by_name(name).into_iter().filter(|n| n.file == *path));
            }
        }
    }
    imports.sort_by_key(|n| n.id);
    imports.dedup_by_key(|n| n.id);
    if imports.len() == 1 {
        return Some((imports[0].id, "import".into()));
    }
    if imports.len() > 1 {
        return None;
    }
    let score = |n: &SymbolNode| -> i32 {
        if n.file == caller_file {
            4
        } else if n.file.parent().is_some() && n.file.parent() == caller_file.parent() {
            2
        } else if top_component(&n.file, root).is_some()
            && top_component(&n.file, root) == top_component(caller_file, root)
        {
            1
        } else {
            0
        }
    };
    let candidates = g.find_by_name(callee);
    let best_score = candidates.iter().map(|n| score(n)).max()?;
    let candidates = candidates
        .into_iter()
        .filter(|n| score(n) == best_score)
        .collect::<Vec<_>>();
    // Same-name ambiguity is not a resolved call, even when its ordering is deterministic.
    (candidates.len() == 1).then(|| {
        (
            candidates[0].id,
            if best_score == 4 {
                "lexical"
            } else {
                "name-match"
            }
            .into(),
        )
    })
}

#[derive(Clone)]
struct ParsedFile {
    mtime_ns: u128,
    len: u64,
    nodes: Vec<SymbolNode>,
    calls: Vec<RawCall>,
    imports: Vec<Import>,
}

struct CachedWorkspace {
    fingerprint: u64,
    graph: Arc<CodeGraph>,
    files: HashMap<PathBuf, Arc<ParsedFile>>,
}

pub struct IndexUpdate {
    pub graph: Arc<CodeGraph>,
    pub stats: IndexStats,
    pub changed: bool,
}

pub fn build_graph(root: &Path) -> CodeGraph {
    CodeIndex::new().get(root).as_ref().clone()
}

/// Workspace-local parsed files are reused; only relationships are resolved again after a change.
#[derive(Default)]
pub struct CodeIndex {
    cache: Mutex<HashMap<PathBuf, Arc<Mutex<Option<CachedWorkspace>>>>>,
}
impl CodeIndex {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, root: &Path) -> Arc<CodeGraph> {
        self.update(root, &AtomicBool::new(false))
            .map(|u| u.graph)
            .unwrap_or_default()
    }
    pub fn update(&self, root: &Path, cancel: &AtomicBool) -> Result<IndexUpdate, String> {
        let started = Instant::now();
        let root = super::canonical(root);
        if !root.is_dir() {
            return Err("工作区目录不可读取".into());
        }
        let seat = {
            let mut cache = self.cache.lock().unwrap();
            if cache.len() >= 8 && !cache.contains_key(&root) {
                if let Some(key) = cache
                    .iter()
                    .find(|(_, value)| Arc::strong_count(value) == 1)
                    .map(|(key, _)| key.clone())
                {
                    cache.remove(&key);
                }
            }
            cache.entry(root.clone()).or_default().clone()
        };
        let mut cached = seat.lock().unwrap();
        let mut stats = IndexStats::default();
        let walked = collect_files(&root, cancel, started, &mut stats)?;
        let mut config = DefaultHasher::new();
        fingerprint(&walked).hash(&mut config);
        for name in ["tsconfig.json", "jsconfig.json"] {
            if root
                .join(name)
                .metadata()
                .is_ok_and(|m| m.len() <= 512 * 1024)
            {
                super::imports::config_source(&root.join(name)).hash(&mut config);
            }
        }
        let fp = config.finish();
        if !stats.partial {
            if let Some(previous) = cached.as_ref().filter(|c| c.fingerprint == fp) {
                stats.indexed_files = previous.files.len();
                stats.reused_files = previous.files.len();
                stats.unresolved_calls = previous.graph.unresolved_calls;
                stats.partial = stats.skipped_files > 0;
                stats.elapsed_ms = started.elapsed().as_millis() as u64;
                return Ok(IndexUpdate {
                    graph: previous.graph.clone(),
                    stats,
                    changed: false,
                });
            }
        }
        let mut files = HashMap::new();
        for file in &walked {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if started.elapsed().as_secs() >= 15 {
                stats.partial = true;
                stats.reasons.push("解析达到时间预算".into());
                break;
            }
            if let Some(parsed) = cached
                .as_ref()
                .and_then(|c| c.files.get(&file.path))
                .filter(|p| p.mtime_ns == file.mtime_ns && p.len == file.len)
            {
                files.insert(file.path.clone(), parsed.clone());
                stats.reused_files += 1;
                continue;
            }
            let source = match std::fs::File::open(&file.path).and_then(bounded_source) {
                Ok(Some(source)) => source,
                _ => {
                    stats.skipped_files += 1;
                    continue;
                }
            };
            let stop = || cancel.load(Ordering::Relaxed) || started.elapsed().as_secs() >= 15;
            if let Some((nodes, calls)) = parse_file(&file.path, &source, &stop) {
                let body = if file
                    .path
                    .extension()
                    .is_some_and(|x| x == "vue" || x == "svelte")
                {
                    super::imports::script_source(&source)
                } else {
                    source
                };
                let Some(imports) = super::imports::imports_with_budget(&body, &stop) else {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    stats.partial = true;
                    stats.reasons.push("导入解析达到时间预算".into());
                    break;
                };
                files.insert(
                    file.path.clone(),
                    Arc::new(ParsedFile {
                        mtime_ns: file.mtime_ns,
                        len: file.len,
                        nodes,
                        calls,
                        imports,
                    }),
                );
                stats.parsed_files += 1;
            } else {
                stats.skipped_files += 1;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if stop() {
                stats.partial = true;
                stats.reasons.push("单文件解析达到时间预算".into());
                break;
            }
        }
        let known = files.keys().cloned().collect::<HashSet<_>>();
        let resolver = ImportResolver::new(&root);
        let mut graph = CodeGraph::new();
        for (path, parsed) in &files {
            for node in &parsed.nodes {
                if graph.nodes.len() >= 50_000 {
                    stats.partial = true;
                    break;
                }
                graph.add_symbol(node.clone());
            }
            graph
                .file_mtimes
                .insert(path.clone(), (parsed.mtime_ns / 1_000_000_000) as u64);
        }
        let mut edge_count = 0usize;
        for (path, parsed) in &files {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if started.elapsed().as_secs() >= 15 || edge_count >= 50_000 {
                stats.partial = true;
                stats.reasons.push("关系解析达到时间或数量预算".into());
                break;
            }
            let imports = parsed
                .imports
                .iter()
                .filter_map(|i| {
                    resolver
                        .resolve(path, &i.specifier, &known)
                        .map(|target| (i.clone(), target))
                })
                .collect::<Vec<_>>();
            for (import, target) in &imports {
                graph.imports.push(super::graph::FileImport {
                    source: path.clone(),
                    target: target.clone(),
                    line: import.line,
                });
            }
            for call in &parsed.calls {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                if edge_count >= 50_000 || started.elapsed().as_secs() >= 15 {
                    stats.partial = true;
                    break;
                }
                let caller = CodeGraph::make_id_at(path, &call.caller_name, call.caller_byte);
                if graph.node(caller).is_none() {
                    continue;
                }
                if let Some((callee, resolution)) =
                    resolve_callee(&graph, &call.callee_name, path, &root, &imports)
                {
                    graph.add_edge(
                        caller,
                        Edge {
                            to: callee,
                            kind: EdgeKind::Calls,
                            line: call.line,
                            resolution,
                        },
                    );
                    edge_count += 1;
                } else {
                    graph.unresolved_calls += 1;
                }
            }
        }
        stats.indexed_files = files.len();
        stats.unresolved_calls = graph.unresolved_calls;
        stats.partial |= stats.skipped_files > 0;
        if stats.skipped_files > 0 {
            stats.reasons.push(format!(
                "{} 个文件过大、不可读或无法解析",
                stats.skipped_files
            ));
        }
        stats.elapsed_ms = started.elapsed().as_millis() as u64;
        let graph = Arc::new(graph);
        // Partial indexes must be eligible for a later completion pass.
        *cached = Some(CachedWorkspace {
            fingerprint: if stats.partial { 0 } else { fp },
            files,
            graph: graph.clone(),
        });
        Ok(IndexUpdate {
            graph,
            stats,
            changed: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_typescript_and_vue_bound_functions_create_unambiguous_edges() {
        for extension in ["js", "ts", "tsx", "vue"] {
            let d = tempfile::tempdir().unwrap();
            let source = "export function helper() { return 1; } export class Marker {} export const first = () => helper(), second = function internal() { helper(); }; export function run() { first(); second(); }";
            let source = if extension == "vue" {
                format!(
                    "<template>first() 中文</template>\n<script lang=\"ts\">\n{source}\n</script>"
                )
            } else {
                source.into()
            };
            std::fs::write(d.path().join(format!("entry.{extension}")), source).unwrap();
            let graph = build_graph(d.path());
            assert_eq!(graph.nodes.len(), 5, "{extension}: {:?}", graph.nodes);
            for name in ["helper", "Marker", "first", "second", "run"] {
                assert_eq!(graph.find_by_name(name).len(), 1, "{extension}: {name}");
            }
            assert_eq!(graph.find_by_name("Marker")[0].kind, SymbolKind::Class);
            for name in ["helper", "first", "second", "run"] {
                assert_eq!(graph.find_by_name(name)[0].kind, SymbolKind::Function);
            }
            let mut edges = HashSet::new();
            for (caller, calls) in &graph.edges_out {
                for call in calls {
                    edges.insert((
                        graph.node(*caller).unwrap().name.as_str(),
                        graph.node(call.to).unwrap().name.as_str(),
                    ));
                }
            }
            assert_eq!(
                edges,
                HashSet::from([
                    ("first", "helper"),
                    ("second", "helper"),
                    ("run", "first"),
                    ("run", "second")
                ]),
                "{extension}"
            );
            assert_eq!(
                graph.unresolved_calls, 0,
                "duplicate exports must not create false ambiguity"
            );
            if extension == "vue" {
                assert_eq!(graph.find_by_name("run")[0].start_line, 3);
            }
        }
    }

    #[test]
    fn same_named_same_line_callers_keep_distinct_ids_and_ambiguous_targets() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("entry.js"), "function alpha() {} function beta() {} function wrap() { const dup = () => alpha(); { const dup = function local() { beta(); }; dup(); } dup(); }").unwrap();
        let graph = build_graph(d.path());
        let duplicates = graph.find_by_name("dup");
        assert_eq!(duplicates.len(), 2);
        assert_ne!(duplicates[0].id, duplicates[1].id);
        assert_eq!(duplicates[0].start_line, duplicates[1].start_line);
        let targets = duplicates
            .iter()
            .map(|node| {
                let calls = graph.callees(node.id).unwrap();
                assert_eq!(calls.len(), 1);
                graph.node(calls[0].to).unwrap().name.as_str()
            })
            .collect::<HashSet<_>>();
        assert_eq!(targets, HashSet::from(["alpha", "beta"]));
        assert_eq!(
            graph.unresolved_calls, 2,
            "same-name scope ambiguity stays explicit"
        );
    }

    #[test]
    fn caller_ranges_distinguish_adjacent_and_nested_functions_on_one_line() {
        let source = "fn helper() {} fn first() { helper(); fn nested() { helper(); } nested(); } fn second() { first(); }";
        let (_, calls) = parse_file(Path::new("same_line.rs"), source, &|| false).unwrap();
        let relationships = calls
            .iter()
            .map(|call| (call.caller_name.as_str(), call.callee_name.as_str()))
            .collect::<HashSet<_>>();
        assert!(relationships.contains(&("first", "helper")));
        assert!(relationships.contains(&("nested", "helper")));
        assert!(relationships.contains(&("first", "nested")));
        assert!(relationships.contains(&("second", "first")));
        assert!(!relationships.contains(&("second", "helper")));
    }

    #[test]
    fn one_dense_file_observes_cancellation_inside_tree_sitter_work() {
        let source = (0..20_000)
            .map(|i| format!("fn item_{i}() {{ helper(); }}\n"))
            .collect::<String>();
        let checks = std::cell::Cell::new(0usize);
        let stop = || {
            checks.set(checks.get() + 1);
            checks.get() > 20
        };
        assert!(parse_file(Path::new("dense.rs"), &source, &stop).is_none());
        assert!(checks.get() >= 21);
        assert!(
            checks.get() < 50,
            "tree-sitter should stop promptly after its cancellation callback"
        );
    }

    #[test]
    fn interval_lookup_handles_many_symbols_without_scanning_them_per_call() {
        let symbols = (0..20_000)
            .map(|index| Symbol {
                name: format!("f{index}"),
                kind: "function_item".into(),
                start_line: index + 1,
                end_line: index + 1,
                start_byte: index * 20,
                end_byte: index * 20 + 15,
            })
            .collect::<Vec<_>>();
        let index = CallerIndex::new(&symbols, &|| false).unwrap();
        assert_eq!(index.0.len(), 40_000);
        for value in (0..20_000).rev() {
            assert_eq!(index.at(value * 20 + 3), Some(value));
            assert_eq!(index.at(value * 20 + 18), None);
        }
    }

    #[test]
    fn read_limit_is_enforced_even_when_a_file_grows_after_metadata() {
        let mut bytes = std::io::Cursor::new(vec![b'x'; MAX_FILE_BYTES as usize * 2]);
        assert!(bounded_source(&mut bytes).unwrap().is_none());
        assert_eq!(bytes.position(), MAX_FILE_BYTES + 1);
    }

    #[test]
    fn edits_reparse_only_changed_files_and_delete_old_relationships() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.rs"), "fn run(){ helper(); }").unwrap();
        std::fs::write(d.path().join("b.rs"), "fn helper(){}").unwrap();
        let index = CodeIndex::new();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            index.update(d.path(), &cancel).unwrap().stats.parsed_files,
            2
        );
        std::fs::write(d.path().join("a.rs"), "fn run(){ let n = 2; }").unwrap();
        let next = index.update(d.path(), &cancel).unwrap();
        assert_eq!(next.stats.parsed_files, 1);
        assert_eq!(next.stats.reused_files, 1);
        assert!(next.graph.edges_out.is_empty());
        std::fs::remove_file(d.path().join("b.rs")).unwrap();
        let next = index.update(d.path(), &cancel).unwrap();
        assert!(next.graph.find_by_name("helper").is_empty());
        assert!(next.changed);
        assert!(!index.update(d.path(), &cancel).unwrap().changed);
    }

    #[test]
    fn vue_script_offsets_and_import_aliases_resolve_without_name_guessing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("src")).unwrap();
        std::fs::write(
            d.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            d.path().join("src/util.ts"),
            "export function helper(){ return 1; }",
        )
        .unwrap();
        std::fs::write(
            d.path().join("src/other.ts"),
            "export function helper(){ return 2; }",
        )
        .unwrap();
        std::fs::write(d.path().join("src/App.vue"), "<template>helper() 中文</template>\n<script setup lang=\"ts\">\nimport { helper as calculate } from '@/util';\nfunction run() { calculate(); }\n</script>").unwrap();
        let graph = build_graph(d.path());
        let run = graph.find_by_name("run")[0];
        assert_eq!(run.start_line, 4);
        let edge = &graph.callees(run.id).unwrap()[0];
        assert_eq!(edge.resolution, "import");
        assert!(graph.node(edge.to).unwrap().file.ends_with("util.ts"));
        assert_eq!(graph.imports.len(), 1);
        assert_eq!(graph.imports[0].line, 3);
    }

    #[test]
    fn cancellation_keeps_previous_index_and_workspaces_are_isolated() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("a.rs"), "fn alpha(){}").unwrap();
        std::fs::write(b.path().join("b.rs"), "fn beta(){}").unwrap();
        let index = CodeIndex::new();
        let initial = index.get(a.path());
        assert!(index.get(b.path()).find_by_name("alpha").is_empty());
        assert!(index.update(a.path(), &AtomicBool::new(true)).is_err());
        assert!(Arc::ptr_eq(&initial, &index.get(a.path())));
    }

    #[test]
    fn builds_cross_file_call_edges() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("a.rs"),
            "fn helper() {}\nfn main() {\n    helper();\n}\n",
        )
        .unwrap();
        let g = build_graph(d.path());
        let main = g.find_by_name("main").into_iter().next().expect("main");
        let helper = g.find_by_name("helper").into_iter().next().expect("helper");
        // main → helper edge exists
        let callees = g.callees(main.id).expect("callees");
        assert!(
            callees.iter().any(|e| e.to == helper.id),
            "main should call helper"
        );
        // reverse: helper has main as caller
        assert!(
            g.callers(helper.id)
                .unwrap()
                .iter()
                .any(|e| e.to == main.id)
        );
    }

    #[test]
    fn resolves_calls_across_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("util.rs"), "pub fn compute() -> i32 { 42 }\n").unwrap();
        std::fs::write(
            d.path().join("main.rs"),
            "fn run() {\n    let _ = compute();\n}\n",
        )
        .unwrap();
        let g = build_graph(d.path());
        let run = g.find_by_name("run").into_iter().next().expect("run");
        let compute = g
            .find_by_name("compute")
            .into_iter()
            .next()
            .expect("compute");
        assert!(
            g.callees(run.id)
                .unwrap()
                .iter()
                .any(|e| e.to == compute.id),
            "run → compute across files"
        );
    }

    #[test]
    fn self_calls_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("r.rs"),
            "fn recur(n: i32) {\n    if n > 0 { recur(n - 1); }\n}\n",
        )
        .unwrap();
        let g = build_graph(d.path());
        let recur = g.find_by_name("recur").into_iter().next().expect("recur");
        assert!(
            g.callees(recur.id).map(|e| e.is_empty()).unwrap_or(true),
            "self-call must be skipped"
        );
    }

    #[test]
    fn same_second_edit_triggers_rebuild() {
        // Overwriting the SAME file (likely the same wall-clock second) must rebuild —
        // the fingerprint uses nanos + length, not coarse seconds.
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        let idx = CodeIndex::new();
        let g1 = idx.get(d.path());
        assert!(g1.find_by_name("two").is_empty());
        std::fs::write(&f, "fn one() {}\nfn two() {}\n").unwrap();
        let g2 = idx.get(d.path());
        assert!(
            !g2.find_by_name("two").is_empty(),
            "same-second edit must rebuild (nanos/len changed)"
        );
    }

    #[test]
    fn ambiguous_same_name_is_not_reported_as_a_resolved_call() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a_util.rs"), "pub fn dup() {}\n").unwrap();
        std::fs::write(d.path().join("z_util.rs"), "pub fn dup() {}\n").unwrap();
        std::fs::write(d.path().join("main.rs"), "fn run() { dup(); }\n").unwrap();
        let g = build_graph(d.path());
        let run = g.find_by_name("run")[0];
        assert!(g.callees(run.id).is_none());
        assert_eq!(g.unresolved_calls, 1);
    }

    #[test]
    fn index_caches_then_rebuilds_on_change() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.rs"), "fn one() {}\n").unwrap();
        let idx = CodeIndex::new();
        let g1 = idx.get(d.path());
        let g2 = idx.get(d.path());
        assert!(
            Arc::ptr_eq(&g1, &g2),
            "unchanged repo → cached graph reused"
        );
        assert!(g1.find_by_name("two").is_empty());
        // change the repo (new mtime via a new file) → rebuild
        std::fs::write(d.path().join("b.rs"), "fn two() {}\n").unwrap();
        let g3 = idx.get(d.path());
        assert!(!Arc::ptr_eq(&g1, &g3), "changed repo → rebuilt");
        assert!(
            !g3.find_by_name("two").is_empty(),
            "rebuilt graph sees new symbol"
        );
    }

    #[test]
    fn caller_attribution_is_per_file() {
        // Two files each define a function named `handler`, each calling a DISTINCT callee.
        // The old resolver picked the first same-named symbol as caller, so both edges hung
        // off ONE handler. Caller id must be reconstructed exactly, per file.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("a.rs"),
            "fn handler() {\n    alpha();\n}\nfn alpha() {}\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("b.rs"),
            "fn handler() {\n    beta();\n}\nfn beta() {}\n",
        )
        .unwrap();
        let g = build_graph(d.path());

        let a_handler = g
            .find_by_name("handler")
            .into_iter()
            .find(|n| n.file.ends_with("a.rs"))
            .expect("a.rs handler");
        let b_handler = g
            .find_by_name("handler")
            .into_iter()
            .find(|n| n.file.ends_with("b.rs"))
            .expect("b.rs handler");
        let alpha = g.find_by_name("alpha").into_iter().next().expect("alpha");
        let beta = g.find_by_name("beta").into_iter().next().expect("beta");

        let a_callees = g.callees(a_handler.id).cloned().unwrap_or_default();
        let b_callees = g.callees(b_handler.id).cloned().unwrap_or_default();

        assert!(
            a_callees.iter().any(|e| e.to == alpha.id),
            "a.rs::handler → alpha"
        );
        assert!(
            !a_callees.iter().any(|e| e.to == beta.id),
            "a.rs::handler must NOT call beta"
        );
        assert!(
            b_callees.iter().any(|e| e.to == beta.id),
            "b.rs::handler → beta"
        );
        assert!(
            !b_callees.iter().any(|e| e.to == alpha.id),
            "b.rs::handler must NOT call alpha"
        );
    }
}
