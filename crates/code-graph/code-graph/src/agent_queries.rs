//! Bounded evidence queries over the shared workspace snapshot.
use crate::{GraphCall, GraphSnapshot, GraphSymbol};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Default)]
pub struct AgentQuery {
    pub action: String,
    pub query: String,
    pub symbol: String,
    pub to: String,
    pub path: String,
    pub files: Vec<String>,
    pub limit: usize,
    pub depth: usize,
}

fn in_scope(path: &str, scope: &str) -> bool {
    scope.is_empty()
        || path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn symbol(node: &GraphSymbol) -> Value {
    json!({"id":node.id,"name":node.name,"path":node.path,"line":node.line,"endLine":node.end_line,"kind":node.kind,"evidence":"tree-sitter-definition"})
}
fn edge(call: &GraphCall) -> Value {
    json!({"source":call.source,"target":call.target,"path":call.path,"line":call.line,"relation":call.kind,
        "resolution":call.resolution,"confidence":if call.resolution.contains("heuristic")||call.resolution.contains("name") {"heuristic"}else{"static-candidate"}})
}
fn resolve<'a>(nodes: &[&'a GraphSymbol], selector: &str) -> Vec<&'a GraphSymbol> {
    if selector.is_empty() {
        return Vec::new();
    }
    let exact = nodes
        .iter()
        .copied()
        .filter(|node| node.id == selector || format!("{}#{}", node.path, node.name) == selector)
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }
    nodes
        .iter()
        .copied()
        .filter(|node| node.name == selector)
        .collect()
}
fn selection_error(nodes: &[&GraphSymbol], limit: usize) -> Value {
    json!({"status":if nodes.is_empty(){"not_found"}else{"ambiguous"},"candidates":nodes.iter().take(limit).map(|node|symbol(node)).collect::<Vec<_>>(),
        "limited":nodes.len()>limit,"guidance":"Use a returned symbol id or path#name. An empty static result does not prove the absence of runtime relationships."})
}

pub fn query(
    snapshot: &GraphSnapshot,
    request: &AgentQuery,
    stop: &dyn Fn() -> bool,
) -> Result<Value, String> {
    if stop() {
        return Err("code query cancelled".into());
    }
    let limit = if request.limit == 0 {
        12
    } else {
        request.limit.min(50)
    };
    let depth = if request.depth == 0 {
        2
    } else {
        request.depth.min(4)
    };
    let path = request.path.replace('\\', "/");
    let path = path.trim_matches('/');
    if path.split('/').any(|piece| piece == "..") {
        return Err("code query path must remain inside the workspace".into());
    }
    let nodes = snapshot
        .symbols
        .iter()
        .filter(|node| in_scope(&node.path, path))
        .collect::<Vec<_>>();
    let by_id = nodes
        .iter()
        .map(|node| (node.id.as_str(), *node))
        .collect::<HashMap<_, _>>();
    let relations = snapshot
        .calls
        .iter()
        .filter(|call| {
            by_id.contains_key(call.source.as_str()) && by_id.contains_key(call.target.as_str())
        })
        .collect::<Vec<_>>();
    let calls = relations
        .iter()
        .copied()
        .filter(|call| call.kind == "call")
        .collect::<Vec<_>>();
    let mut result = match request.action.as_str() {
        "context" => {
            let text = request.query.to_lowercase();
            let words = text
                .split(|ch: char| ch.is_whitespace() || matches!(ch, '/' | '\\' | ':' | '_' | '-'))
                .filter(|word| !word.is_empty())
                .collect::<Vec<_>>();
            let mut ranked = Vec::new();
            for node in &nodes {
                if stop() {
                    return Err("code query cancelled".into());
                }
                let name = node.name.to_lowercase();
                let file = node.path.to_lowercase();
                let score = if node.id == request.query {
                    1000
                } else if name == text && !text.is_empty() {
                    900
                } else if file == text && !text.is_empty() {
                    800
                } else if !text.is_empty() && name.contains(&text) {
                    600
                } else {
                    words
                        .iter()
                        .map(|word| {
                            if name.contains(word) {
                                80
                            } else if file.contains(word) {
                                30
                            } else {
                                0
                            }
                        })
                        .sum()
                };
                if score > 0 || text.is_empty() {
                    ranked.push((score, *node))
                }
            }
            ranked.sort_by(|a, b| {
                b.0.cmp(&a.0).then_with(|| {
                    (&a.1.path, a.1.line, &a.1.id).cmp(&(&b.1.path, b.1.line, &b.1.id))
                })
            });
            json!({"status":"ready","candidates":ranked.iter().take(limit).map(|(score,node)|{
                let mut value=symbol(node);value["matchScore"]=json!(score);value["callerCount"]=json!(calls.iter().filter(|call|call.target==node.id).count());value["calleeCount"]=json!(calls.iter().filter(|call|call.source==node.id).count());value
            }).collect::<Vec<_>>(),"matched":ranked.len(),"limited":ranked.len()>limit})
        }
        "callers" | "callees" => {
            let selected = resolve(&nodes, &request.symbol);
            if selected.len() != 1 {
                return Ok(selection_error(&selected, limit));
            }
            let incoming = request.action == "callers";
            let mut adjacency: HashMap<&str, Vec<&GraphCall>> = HashMap::new();
            for call in &calls {
                adjacency
                    .entry(if incoming { &call.target } else { &call.source })
                    .or_default()
                    .push(call)
            }
            let mut seen = HashSet::from([selected[0].id.as_str()]);
            let mut queue = VecDeque::from([(selected[0].id.as_str(), 0)]);
            let mut found = Vec::new();
            let mut evidence = Vec::new();
            let mut limited = false;
            while let Some((id, level)) = queue.pop_front() {
                if stop() {
                    return Err("code query cancelled".into());
                }
                if level >= depth {
                    continue;
                }
                for call in adjacency.get(id).into_iter().flatten() {
                    let next = if incoming {
                        call.source.as_str()
                    } else {
                        call.target.as_str()
                    };
                    if seen.insert(next) {
                        if found.len() >= limit {
                            limited = true;
                            break;
                        }
                        let mut value = symbol(by_id[next]);
                        value["depth"] = json!(level + 1);
                        found.push(value);
                        evidence.push(edge(call));
                        queue.push_back((next, level + 1));
                    }
                }
                if limited {
                    break;
                }
            }
            json!({"status":"ready","symbol":symbol(selected[0]),"symbols":found,"relations":evidence,"limited":limited,"depth":depth})
        }
        "path" => {
            let from = resolve(&nodes, &request.symbol);
            let to = resolve(&nodes, &request.to);
            if from.len() != 1 {
                return Ok(selection_error(&from, limit));
            }
            if to.len() != 1 {
                return Ok(selection_error(&to, limit));
            }
            let from = from[0];
            let to = to[0];
            let mut adjacency: HashMap<&str, Vec<&GraphCall>> = HashMap::new();
            for call in &calls {
                adjacency.entry(&call.source).or_default().push(call)
            }
            let mut queue = VecDeque::from([(from.id.as_str(), 0)]);
            let mut seen = HashSet::from([from.id.as_str()]);
            let mut previous: HashMap<&str, &GraphCall> = HashMap::new();
            let mut found = false;
            let mut limited = false;
            while let Some((id, hops)) = queue.pop_front() {
                if stop() {
                    return Err("code query cancelled".into());
                }
                if id == to.id {
                    found = true;
                    break;
                }
                if hops >= 20 {
                    limited = true;
                    continue;
                }
                for call in adjacency.get(id).into_iter().flatten() {
                    if seen.len() >= 10000 {
                        limited = true;
                        break;
                    }
                    if seen.insert(call.target.as_str()) {
                        previous.insert(&call.target, call);
                        queue.push_back((&call.target, hops + 1))
                    }
                }
            }
            let mut chain = Vec::new();
            let mut id = to.id.as_str();
            if found {
                while id != from.id {
                    let call = previous[id];
                    chain.push(edge(call));
                    id = &call.source
                }
                chain.reverse()
            }
            json!({"status":if found{"ready"}else{"not_found"},"from":symbol(from),"to":symbol(to),"path":chain,"limited":limited,"maxHops":20})
        }
        "impact" => {
            let selected = if request.symbol.is_empty() {
                nodes
                    .iter()
                    .copied()
                    .filter(|node| {
                        request
                            .files
                            .iter()
                            .any(|file| in_scope(&node.path, &file.replace('\\', "/")))
                    })
                    .collect::<Vec<_>>()
            } else {
                let selected = resolve(&nodes, &request.symbol);
                if selected.len() != 1 {
                    return Ok(selection_error(&selected, limit));
                }
                selected
            };
            if selected.is_empty() && request.files.is_empty() {
                return Ok(selection_error(&selected, limit));
            }
            let mut changed = selected
                .iter()
                .map(|node| node.path.as_str())
                .collect::<HashSet<_>>();
            changed.extend(request.files.iter().map(String::as_str));
            let mut affected = Vec::new();
            let mut evidence = Vec::new();
            let mut limited = false;
            let mut frontier = selected
                .iter()
                .map(|node| node.id.as_str())
                .collect::<HashSet<_>>();
            let mut visited = frontier.clone();
            for level in 0..depth {
                let mut next = HashSet::new();
                for call in &relations {
                    if stop() {
                        return Err("code query cancelled".into());
                    }
                    if frontier.contains(call.target.as_str())
                        && visited.insert(call.source.as_str())
                    {
                        if affected.len() >= limit {
                            limited = true;
                            break;
                        }
                        let node = by_id[call.source.as_str()];
                        changed.insert(&node.path);
                        let mut value = symbol(node);
                        value["depth"] = json!(level + 1);
                        affected.push(value);
                        evidence.push(edge(call));
                        next.insert(call.source.as_str());
                    }
                }
                if limited || next.is_empty() {
                    break;
                }
                frontier = next;
            }
            let mut dependencies = Vec::new();
            for dependency in &snapshot.deps {
                if stop() {
                    return Err("code query cancelled".into());
                }
                if in_scope(&dependency.source, path)
                    && changed.contains(dependency.target.as_str())
                {
                    if dependencies.len() >= limit {
                        limited = true;
                        break;
                    }
                    dependencies.push(json!({"path":dependency.source,"line":dependency.line,"target":dependency.target,"relation":dependency.kind,"evidence":"static-dependency"}));
                }
            }
            let mut tests = HashSet::new();
            let mut test_rows = Vec::new();
            let mut configuration_candidates = Vec::new();
            let mut configuration_seen = HashSet::new();
            for file in &request.files {
                let name = file.rsplit('/').next().unwrap_or(file).to_lowercase();
                let configured = matches!(
                    name.as_str(),
                    "package.json"
                        | "tsconfig.json"
                        | "jsconfig.json"
                        | "cargo.toml"
                        | "cargo.lock"
                        | "go.mod"
                        | "go.sum"
                        | "pyproject.toml"
                        | "requirements.txt"
                        | "pom.xml"
                        | "build.gradle"
                        | "cmakelists.txt"
                ) || name.ends_with(".csproj");
                if !configured {
                    continue;
                }
                let directory = file
                    .rsplit_once('/')
                    .map(|(directory, _)| directory)
                    .unwrap_or("");
                for node in &nodes {
                    if stop() {
                        return Err("code query cancelled".into());
                    }
                    if in_scope(&node.path, directory)
                        && configuration_seen.insert(node.path.as_str())
                    {
                        if configuration_candidates.len() >= limit {
                            limited = true;
                            break;
                        }
                        changed.insert(node.path.as_str());
                        configuration_candidates.push(json!({"path":node.path,"line":1,"configuration":file,"confidence":"heuristic","evidence":"configuration-scope-candidate"}));
                    }
                }
            }
            for node in &nodes {
                let lower = node.path.to_lowercase();
                if (lower.contains("test") || lower.contains("spec"))
                    && tests.insert(node.path.as_str())
                    && (changed.contains(node.path.as_str())
                        || selected
                            .iter()
                            .any(|selected| lower.contains(selected.name.to_lowercase().as_str())))
                {
                    if test_rows.len() >= limit {
                        limited = true;
                        break;
                    }
                    test_rows.push(json!({"path":node.path,"line":node.line,"evidence":"test-path-candidate","confidence":"heuristic"}));
                }
            }
            json!({"status":"ready","roots":selected.iter().take(limit).map(|node|symbol(node)).collect::<Vec<_>>(),"files":request.files,"affected":affected,"relations":evidence,"dependentFiles":dependencies,"configurationCandidates":configuration_candidates,"testCandidates":test_rows,"missingDefinitions":selected.is_empty(),"limited":limited||selected.len()>limit,"depth":depth})
        }
        _ => return Err("unknown code query action".into()),
    };
    result["partial"] = json!(snapshot.truncated);
    result["engine"] = json!(snapshot.engine);
    result["scope"] = json!(path);
    result["guidance"] = json!(
        "Read the referenced source before editing. Static/name-based relationships and test candidates require verification; missing edges do not prove independence."
    );
    // Output stays useful inside an Agent request even for extreme identifiers.
    while serde_json::to_vec(&result)
        .map_err(|error| error.to_string())?
        .len()
        > 48 * 1024
    {
        let candidate = result
            .as_object_mut()
            .unwrap()
            .values_mut()
            .filter_map(Value::as_array_mut)
            .max_by_key(|items| items.len());
        match candidate {
            Some(items) if !items.is_empty() => {
                items.pop();
            }
            _ => return Err("code evidence exceeds the output budget".into()),
        }
        result["limited"] = json!(true);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, GraphSnapshot) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("main.rs"),
            "fn start(){step();} fn step(){finish();} fn finish(){}\n",
        )
        .unwrap();
        std::fs::create_dir(directory.path().join("tests")).unwrap();
        std::fs::write(
            directory.path().join("tests/finish_spec.rs"),
            "fn check_finish(){}\n",
        )
        .unwrap();
        let graph = crate::build_graph(directory.path());
        let snapshot = GraphSnapshot::from_graph(&graph, directory.path());
        (directory, snapshot)
    }
    #[test]
    fn definitions_traversals_paths_and_impact_carry_real_source_evidence() {
        let (_directory, snapshot) = fixture();
        let context = query(
            &snapshot,
            &AgentQuery {
                action: "context".into(),
                query: "start".into(),
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        let id = context["candidates"][0]["id"].as_str().unwrap();
        assert_eq!(context["candidates"][0]["path"], "main.rs");
        let calls = query(
            &snapshot,
            &AgentQuery {
                action: "callees".into(),
                symbol: id.into(),
                depth: 2,
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(calls["symbols"].as_array().unwrap().len(), 2);
        assert!(
            calls["relations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edge| edge["path"] == "main.rs" && edge["line"].as_u64().unwrap() > 0)
        );
        let path = query(
            &snapshot,
            &AgentQuery {
                action: "path".into(),
                symbol: "start".into(),
                to: "finish".into(),
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(path["path"].as_array().unwrap().len(), 2);
        let impact = query(
            &snapshot,
            &AgentQuery {
                action: "impact".into(),
                symbol: "finish".into(),
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(impact["affected"].as_array().unwrap().len(), 2);
        assert_eq!(impact["testCandidates"][0]["path"], "tests/finish_spec.rs");
        assert_eq!(impact["testCandidates"][0]["confidence"], "heuristic");
    }
    #[test]
    fn ambiguous_names_limits_scope_and_cancellation_do_not_invent_targets() {
        let (_directory, mut snapshot) = fixture();
        let mut duplicate = snapshot
            .symbols
            .iter()
            .find(|node| node.name == "finish")
            .unwrap()
            .clone();
        duplicate.id = "different".into();
        duplicate.path = "other.rs".into();
        snapshot.symbols.push(duplicate);
        let ambiguous = query(
            &snapshot,
            &AgentQuery {
                action: "callers".into(),
                symbol: "finish".into(),
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(ambiguous["status"], "ambiguous");
        assert_eq!(ambiguous["candidates"].as_array().unwrap().len(), 2);
        let scoped = query(
            &snapshot,
            &AgentQuery {
                action: "callers".into(),
                symbol: "finish".into(),
                path: "main.rs".into(),
                limit: 1,
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(scoped["symbols"].as_array().unwrap().len(), 1);
        assert_eq!(scoped["limited"], true);
        assert!(query(&snapshot, &AgentQuery::default(), &|| true).is_err());
        assert!(
            query(
                &snapshot,
                &AgentQuery {
                    path: "../outside".into(),
                    ..Default::default()
                },
                &|| false
            )
            .is_err()
        );
    }
    #[test]
    fn configuration_and_deleted_file_inputs_are_explicit_candidates() {
        let (_directory, snapshot) = fixture();
        let config = query(
            &snapshot,
            &AgentQuery {
                action: "impact".into(),
                files: vec!["Cargo.toml".into()],
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        assert!(
            !config["configurationCandidates"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            config["configurationCandidates"][0]["confidence"],
            "heuristic"
        );
        let deleted = query(
            &snapshot,
            &AgentQuery {
                action: "impact".into(),
                files: vec!["deleted.rs".into()],
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(deleted["missingDefinitions"], true);
    }
}
