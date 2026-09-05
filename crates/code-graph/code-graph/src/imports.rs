use regex::Regex;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub(crate) struct Import {
    pub specifier: String,
    pub bindings: Vec<(String, String)>,
    pub line: usize,
}

pub(crate) fn config_source(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(512 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 512 * 1024 {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Keep byte positions and line numbers while parsing only Vue/Svelte script bodies.
pub(crate) fn script_source(source: &str) -> String {
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    let pattern =
        SCRIPT.get_or_init(|| Regex::new(r"(?is)<script\b[^>]*>(.*?)</script\s*>").unwrap());
    let mut masked = source
        .as_bytes()
        .iter()
        .map(|b| if *b == b'\n' || *b == b'\r' { *b } else { b' ' })
        .collect::<Vec<_>>();
    for capture in pattern.captures_iter(source) {
        let body = capture.get(1).unwrap();
        masked[body.start()..body.end()].copy_from_slice(body.as_str().as_bytes());
    }
    String::from_utf8(masked).unwrap_or_default()
}

pub(crate) fn imports(source: &str) -> Vec<Import> {
    imports_with_budget(source, &|| false).unwrap_or_default()
}

pub(crate) fn imports_with_budget(source: &str, stop: &dyn Fn() -> bool) -> Option<Vec<Import>> {
    static IMPORT: OnceLock<Regex> = OnceLock::new();
    let pattern = IMPORT.get_or_init(|| Regex::new(r#"(?m)^[ \t]*(?:import|export)[ \t]+(?:(?:type[ \t]+)?(\{[^}]*\}|\*[ \t]+as[ \t]+[\w$]+|[\w$]+(?:[ \t]*,[ \t]*\{[^}]*\})?)[ \t\r\n]+from[ \t\r\n]+)?["']([^"']+)["']"#).unwrap());
    let mut result = Vec::new();
    let mut offset = 0;
    let mut line = 1;
    for cap in pattern.captures_iter(source) {
        if stop() {
            return None;
        }
        let start = cap.get(0).unwrap().start();
        line += source[offset..start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
        offset = start;
        let clause = cap.get(1).map(|x| x.as_str()).unwrap_or("");
        let named = clause
            .split_once('{')
            .and_then(|(_, tail)| tail.split_once('}'))
            .map(|(names, _)| names)
            .unwrap_or("");
        let mut bindings = Vec::new();
        for name in named.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if stop() {
                return None;
            }
            let pair = name.split_whitespace().collect::<Vec<_>>();
            if let Some(original) = pair.first() {
                let local = if pair.len() >= 3 && pair[1] == "as" {
                    pair[2]
                } else {
                    original
                };
                bindings.push((local.to_string(), original.to_string()));
            }
        }
        if clause.trim_start().starts_with('*') {
            bindings.push(("*".into(), "*".into()));
        }
        result.push(Import {
            specifier: cap[2].to_string(),
            bindings,
            line,
        });
    }
    if stop() { None } else { Some(result) }
}

#[derive(Default)]
pub(crate) struct ImportResolver {
    base: PathBuf,
    aliases: Vec<(String, Vec<String>)>,
}

impl ImportResolver {
    pub fn new(root: &Path) -> Self {
        let mut resolver = Self {
            base: root.to_path_buf(),
            aliases: vec![],
        };
        for name in ["tsconfig.json", "jsconfig.json"] {
            let path = root.join(name);
            if path.metadata().is_ok_and(|m| m.len() <= 512 * 1024) {
                if let Some(text) = config_source(&path) {
                    if let Ok(value) =
                        serde_json::from_str::<serde_json::Value>(&json_comments(&text))
                    {
                        if let Some(options) = value.get("compilerOptions") {
                            resolver.base = root.join(
                                options
                                    .get("baseUrl")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("."),
                            );
                            if let Some(paths) = options.get("paths").and_then(|v| v.as_object()) {
                                resolver.aliases = paths
                                    .iter()
                                    .filter_map(|(alias, values)| {
                                        Some((
                                            alias.clone(),
                                            values
                                                .as_array()?
                                                .iter()
                                                .filter_map(|v| v.as_str().map(str::to_string))
                                                .collect(),
                                        ))
                                    })
                                    .collect();
                            }
                        }
                    }
                }
                break;
            }
        }
        resolver
    }

    pub fn resolve(
        &self,
        file: &Path,
        specifier: &str,
        known: &HashSet<PathBuf>,
    ) -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if specifier.starts_with('.') {
            candidates.push(file.parent()?.join(specifier));
        } else {
            for (alias, replacements) in &self.aliases {
                let value = if let Some((prefix, suffix)) = alias.split_once('*') {
                    specifier
                        .strip_prefix(prefix)
                        .and_then(|v| v.strip_suffix(suffix))
                } else if alias == specifier {
                    Some("")
                } else {
                    None
                };
                if let Some(value) = value {
                    candidates.extend(
                        replacements
                            .iter()
                            .map(|replacement| self.base.join(replacement.replace('*', value))),
                    );
                }
            }
        }
        for base in candidates {
            let mut variants = vec![base.clone()];
            for ext in ["ts", "tsx", "js", "jsx", "mjs", "vue", "svelte"] {
                variants.push(PathBuf::from(format!("{}.{}", base.display(), ext)));
                variants.push(base.join(format!("index.{ext}")));
            }
            // TypeScript permits a .js import to name its .ts source.
            if base.extension().is_some_and(|x| x == "js") {
                variants.push(base.with_extension("ts"));
            }
            for path in variants {
                if let Ok(real) = path.canonicalize() {
                    if known.contains(&real) {
                        return Some(real);
                    }
                }
            }
        }
        None
    }
}

/// Remove JSONC comments without interpreting comment markers inside string values.
fn json_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let (mut at, mut quoted, mut escape) = (0, false, false);
    while at < bytes.len() {
        if quoted {
            if escape {
                escape = false;
            } else if bytes[at] == b'\\' {
                escape = true;
            } else if bytes[at] == b'"' {
                quoted = false;
            }
        } else if bytes[at] == b'"' {
            quoted = true;
        } else if bytes.get(at..at + 2) == Some(b"//") {
            while at < bytes.len() && bytes[at] != b'\n' {
                out[at] = b' ';
                at += 1;
            }
            continue;
        } else if bytes.get(at..at + 2) == Some(b"/*") {
            out[at] = b' ';
            out[at + 1] = b' ';
            at += 2;
            while at < bytes.len() && bytes.get(at..at + 2) != Some(b"*/") {
                if bytes[at] != b'\n' {
                    out[at] = b' ';
                }
                at += 1;
            }
            if at + 1 < bytes.len() {
                out[at] = b' ';
                out[at + 1] = b' ';
                at += 2;
            }
            continue;
        }
        at += 1;
    }
    // JSONC permits trailing commas. The closing delimiters cannot occur here in a string
    // immediately following an unquoted comma, so process with the same string state.
    let (mut quoted, mut escape) = (false, false);
    for at in 0..out.len() {
        if quoted {
            if escape {
                escape = false;
            } else if out[at] == b'\\' {
                escape = true;
            } else if out[at] == b'"' {
                quoted = false;
            }
        } else if out[at] == b'"' {
            quoted = true;
        } else if out[at] == b','
            && out[at + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| matches!(b, b'}' | b']'))
        {
            out[at] = b' ';
        }
    }
    String::from_utf8(out).unwrap_or_default()
}
