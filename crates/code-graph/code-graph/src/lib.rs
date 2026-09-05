//! Native code graph engine ported from AtomCode.
//!
//! The browser only renders graph snapshots. Parsing, call resolution, traversal,
//! dependency analysis, and cache invalidation stay in the Rust Host.

pub mod background;
pub mod graph;
mod imports;
pub mod index;
pub mod lang;
pub mod symbols;

use graph::{CodeGraph, EdgeKind, SymbolKind, SymbolNode};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use background::BackgroundIndex;
pub use index::{CodeIndex, build_graph};

const MAX_ROWS: usize = 50_000;

pub(crate) fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSymbol {
    pub id: String,
    pub name: String,
    pub path: String,
    pub line: usize,
    pub end_line: usize,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphReference {
    pub name: String,
    pub path: String,
    pub line: usize,
    pub kind: &'static str,
    pub target: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCall {
    pub resolution: String,
    pub name: String,
    pub path: String,
    pub line: usize,
    pub kind: &'static str,
    pub source: String,
    pub target: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphDependency {
    pub name: String,
    pub path: String,
    pub line: usize,
    pub kind: &'static str,
    pub source: String,
    pub target: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    pub files: usize,
    pub symbols: Vec<GraphSymbol>,
    pub references: Vec<GraphReference>,
    pub calls: Vec<GraphCall>,
    pub deps: Vec<GraphDependency>,
    pub truncated: bool,
    pub engine: &'static str,
}

fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn symbol_id(node: &SymbolNode) -> String {
    format!("{:016x}", node.id)
}

fn kind_name(kind: &SymbolKind) -> String {
    match kind {
        SymbolKind::Other(value) => value.clone(),
        value => format!("{value:?}").to_ascii_lowercase(),
    }
}

impl GraphSnapshot {
    pub fn from_graph(graph: &CodeGraph, root: &Path) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut nodes: Vec<_> = graph.nodes.values().collect();
        nodes.sort_by(|a, b| {
            (&a.file, a.start_line, &a.name).cmp(&(&b.file, b.start_line, &b.name))
        });
        let id_by_raw: HashMap<_, _> = nodes
            .iter()
            .map(|node| (node.id, symbol_id(node)))
            .collect();
        let mut symbols = Vec::new();
        let mut calls = Vec::new();
        let mut references = Vec::new();
        let mut deps = Vec::new();
        let mut files = HashSet::<PathBuf>::new();
        let mut dependency_pairs = HashSet::<(PathBuf, PathBuf)>::new();
        let mut truncated = false;

        for node in &nodes {
            files.insert(node.file.clone());
            if symbols.len() >= MAX_ROWS {
                truncated = true;
                break;
            }
            symbols.push(GraphSymbol {
                id: id_by_raw[&node.id].clone(),
                name: node.name.clone(),
                path: display_path(&node.file, &root),
                line: node.start_line,
                end_line: node.end_line,
                kind: kind_name(&node.kind),
            });
        }

        let mut sources: Vec<_> = graph.edges_out.iter().collect();
        sources.sort_by_key(|(source, _)| **source);
        for (source, edges) in sources {
            let Some(source_node) = graph.node(*source) else {
                continue;
            };
            let Some(source_id) = id_by_raw.get(source) else {
                continue;
            };
            for edge in edges {
                let Some(target_node) = graph.node(edge.to) else {
                    continue;
                };
                let Some(target_id) = id_by_raw.get(&edge.to) else {
                    continue;
                };
                if calls.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                let kind = match edge.kind {
                    EdgeKind::Calls => "call",
                    EdgeKind::References => "reference",
                    EdgeKind::Imports => "import",
                    EdgeKind::Inherits => "inherits",
                    EdgeKind::Implements => "implements",
                };
                calls.push(GraphCall {
                    resolution: edge.resolution.clone(),
                    name: format!("{} → {}", source_node.name, target_node.name),
                    path: display_path(&source_node.file, &root),
                    line: edge.line,
                    kind,
                    source: source_id.clone(),
                    target: target_id.clone(),
                });
                references.push(GraphReference {
                    name: target_node.name.clone(),
                    path: display_path(&source_node.file, &root),
                    line: edge.line,
                    kind: "call-reference",
                    target: target_id.clone(),
                });
                if source_node.file != target_node.file
                    && dependency_pairs.insert((source_node.file.clone(), target_node.file.clone()))
                {
                    let source_path = display_path(&source_node.file, &root);
                    let target_path = display_path(&target_node.file, &root);
                    deps.push(GraphDependency {
                        name: format!("{source_path} → {target_path}"),
                        path: source_path.clone(),
                        line: edge.line,
                        kind: "call-dependency",
                        source: source_path,
                        target: target_path,
                    });
                }
            }
        }
        for edge in &graph.imports {
            let source = display_path(&edge.source, &root);
            let target = display_path(&edge.target, &root);
            deps.push(GraphDependency {
                name: format!("{source} → {target}"),
                path: source.clone(),
                line: edge.line,
                kind: "import",
                source,
                target,
            });
        }
        Self {
            files: graph.file_mtimes.len().max(files.len()),
            symbols,
            references,
            calls,
            deps,
            truncated,
            engine: "rust-tree-sitter",
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn snapshot_exposes_native_cross_file_graph() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("dep.rs"), "pub fn dep() {}\n").unwrap();
        std::fs::write(root.path().join("main.rs"), "fn run() { dep(); }\n").unwrap();
        let graph = build_graph(root.path());
        let snapshot = GraphSnapshot::from_graph(&graph, root.path());
        assert_eq!(snapshot.engine, "rust-tree-sitter");
        assert!(snapshot.symbols.iter().any(|row| row.name == "run"));
        assert!(snapshot.calls.iter().any(|row| row.name == "run → dep"));
        assert!(snapshot.deps.iter().any(|row| row.target == "dep.rs"));
    }
}
