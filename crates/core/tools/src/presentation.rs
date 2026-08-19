//! Tool render-intent vocabulary: the provider-neutral types a tool
//! declares via `presentCall`/`presentResult` to say how one of its calls
//! renders in a UI. Rust port of `packages/core/tools/src/presentation.ts`.

use dsh_llm::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Category of a tool call, used by a UI to pick an icon or treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Fetch,
    Other,
}

/// A file location a tool reads or modifies, so a capable UI can follow
/// along.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLocation {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

/// A single-file change a tool is about to make, for a UI that renders
/// inline diffs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    /// Prior content, or `null` for a new file / an overwrite.
    #[serde(rename = "oldText")]
    pub old_text: Option<String>,
    /// Content after the change.
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// Provider-neutral pending-call presentation, discriminated by `card`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "card", rename_all = "camelCase")]
pub enum ToolCallView {
    /// The default card: a titled tool-call row.
    Generic {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<ToolCallKind>,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "rawInput")]
        raw_input: Option<JsonValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<FileLocation>>,
    },
    /// A call that IS a shell command running in a working directory.
    Terminal {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    /// A call that creates or modifies files, rendered as an inline diff.
    Diff {
        title: String,
        diffs: Vec<FileDiff>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<FileLocation>>,
    },
}

/// One numbered line of a file carried by [`ToolResultView::Read`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileLine {
    /// 1-based line number in the file.
    pub number: u64,
    /// The line text without its trailing newline.
    pub text: String,
}

/// One matched line inside a [`SearchFileMatches`] group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLineMatch {
    #[serde(rename = "lineNumber")]
    pub line_number: u64,
    pub line: String,
}

/// One file's grouped content matches, in first-seen file order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileMatches {
    pub path: String,
    pub matches: Vec<SearchLineMatch>,
}

/// A completed search rendered as a search card (two `shape`-discriminated
/// variants; the TS `SearchMatchesResultView`/`SearchPathsResultView`
/// interfaces collapse into this enum's variants).
#[derive(Debug, Clone, PartialEq)]
pub enum SearchResultView {
    Matches {
        title: Option<String>,
        files: Vec<SearchFileMatches>,
        truncated: bool,
        total: u64,
    },
    Paths {
        title: Option<String>,
        paths: Vec<String>,
        truncated: bool,
        total: u64,
    },
}

impl Serialize for SearchResultView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        match self {
            SearchResultView::Matches {
                title,
                files,
                truncated,
                total,
            } => {
                map.serialize_entry("shape", "matches")?;
                if let Some(title) = title {
                    map.serialize_entry("title", title)?;
                }
                map.serialize_entry("files", files)?;
                map.serialize_entry("truncated", truncated)?;
                map.serialize_entry("total", total)?;
            }
            SearchResultView::Paths {
                title,
                paths,
                truncated,
                total,
            } => {
                map.serialize_entry("shape", "paths")?;
                if let Some(title) = title {
                    map.serialize_entry("title", title)?;
                }
                map.serialize_entry("paths", paths)?;
                map.serialize_entry("truncated", truncated)?;
                map.serialize_entry("total", total)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for SearchResultView {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("search result view must be an object"))?;
        match object.get("shape").and_then(JsonValue::as_str) {
            Some("matches") => Ok(SearchResultView::Matches {
                title: object
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                files: serde_json::from_value(
                    object
                        .get("files")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::Array(Vec::new())),
                )
                .map_err(serde::de::Error::custom)?,
                truncated: object
                    .get("truncated")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
                total: object.get("total").and_then(JsonValue::as_u64).unwrap_or(0),
            }),
            Some("paths") => Ok(SearchResultView::Paths {
                title: object
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                paths: serde_json::from_value(
                    object
                        .get("paths")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::Array(Vec::new())),
                )
                .map_err(serde::de::Error::custom)?,
                truncated: object
                    .get("truncated")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
                total: object.get("total").and_then(JsonValue::as_u64).unwrap_or(0),
            }),
            Some(other) => Err(serde::de::Error::custom(format!(
                "unknown search result shape {other:?}"
            ))),
            None => Err(serde::de::Error::custom(
                "search result view requires a shape",
            )),
        }
    }
}

/// A completed file read rendered as a line-numbered code view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResultView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub path: String,
    /// The 1-based first line the window requested.
    pub offset: u64,
    pub lines: Vec<ReadFileLine>,
    #[serde(rename = "totalLines")]
    pub total_lines: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentBlock>>,
}

/// One citeable source in a completed web-search result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSource {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "publishedAt"
    )]
    pub published_at: Option<String>,
}

/// A completed web retrieval rendered as a structured card, `kind`-tagged.
#[derive(Debug, Clone, PartialEq)]
pub enum WebResultView {
    Search {
        title: Option<String>,
        sources: Vec<WebSource>,
        answer: Option<String>,
        truncated: bool,
    },
    Fetch {
        title: Option<String>,
        url: String,
        status_code: u64,
        truncated: bool,
    },
}

impl Serialize for WebResultView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        match self {
            WebResultView::Search {
                title,
                sources,
                answer,
                truncated,
            } => {
                map.serialize_entry("kind", "search")?;
                if let Some(title) = title {
                    map.serialize_entry("title", title)?;
                }
                map.serialize_entry("sources", sources)?;
                if let Some(answer) = answer {
                    map.serialize_entry("answer", answer)?;
                }
                map.serialize_entry("truncated", truncated)?;
            }
            WebResultView::Fetch {
                title,
                url,
                status_code,
                truncated,
            } => {
                map.serialize_entry("kind", "fetch")?;
                if let Some(title) = title {
                    map.serialize_entry("title", title)?;
                }
                map.serialize_entry("url", url)?;
                map.serialize_entry("statusCode", status_code)?;
                map.serialize_entry("truncated", truncated)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for WebResultView {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("web result view must be an object"))?;
        match object.get("kind").and_then(JsonValue::as_str) {
            Some("search") => Ok(WebResultView::Search {
                title: object
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                sources: serde_json::from_value(
                    object
                        .get("sources")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::Array(Vec::new())),
                )
                .map_err(serde::de::Error::custom)?,
                answer: object
                    .get("answer")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                truncated: object
                    .get("truncated")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
            }),
            Some("fetch") => Ok(WebResultView::Fetch {
                title: object
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                url: object
                    .get("url")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string)
                    .unwrap_or_default(),
                status_code: object
                    .get("statusCode")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0),
                truncated: object
                    .get("truncated")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false),
            }),
            Some(other) => Err(serde::de::Error::custom(format!(
                "unknown web result kind {other:?}"
            ))),
            None => Err(serde::de::Error::custom("web result view requires a kind")),
        }
    }
}

/// How a tool wants the COMPLETED call shown, discriminated by `card`.
///
/// Nested discriminants (`search`'s `shape` and `web`'s `kind`) are
/// serialized by hand because serde's internally-tagged enums cannot nest.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultView {
    /// The default completed card.
    Generic {
        title: Option<String>,
        content: Option<Vec<ContentBlock>>,
    },
    /// The completed state of a terminal call.
    Terminal {
        title: Option<String>,
        output: Option<String>,
        exit_code: Option<i64>,
        signal: Option<String>,
    },
    /// A completed file mutation rendered as an inline diff card.
    Diff {
        title: Option<String>,
        diffs: Vec<FileDiff>,
    },
    /// A completed search card.
    Search(SearchResultView),
    /// A completed file read card.
    Read(ReadResultView),
    /// A completed web retrieval card.
    Web(WebResultView),
}

impl Serialize for ToolResultView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            ToolResultView::Search(inner) => {
                let mut map = serde_json::Map::new();
                map.insert("card".to_string(), JsonValue::String("search".to_string()));
                let nested = serde_json::to_value(inner).map_err(serde::ser::Error::custom)?;
                if let JsonValue::Object(nested) = nested {
                    for (key, value) in nested {
                        map.insert(key, value);
                    }
                }
                map.serialize(serializer)
            }
            ToolResultView::Web(inner) => {
                let mut map = serde_json::Map::new();
                map.insert("card".to_string(), JsonValue::String("web".to_string()));
                let nested = serde_json::to_value(inner).map_err(serde::ser::Error::custom)?;
                if let JsonValue::Object(nested) = nested {
                    for (key, value) in nested {
                        map.insert(key, value);
                    }
                }
                map.serialize(serializer)
            }
            ToolResultView::Read(inner) => {
                let mut map = serde_json::Map::new();
                map.insert("card".to_string(), JsonValue::String("read".to_string()));
                let nested = serde_json::to_value(inner).map_err(serde::ser::Error::custom)?;
                if let JsonValue::Object(nested) = nested {
                    for (key, value) in nested {
                        map.insert(key, value);
                    }
                }
                map.serialize(serializer)
            }
            _ => {
                let mut map = serializer.serialize_map(None)?;
                match self {
                    ToolResultView::Generic { title, content } => {
                        map.serialize_entry("card", "generic")?;
                        if let Some(title) = title {
                            map.serialize_entry("title", title)?;
                        }
                        if let Some(content) = content {
                            map.serialize_entry("content", content)?;
                        }
                    }
                    ToolResultView::Terminal {
                        title,
                        output,
                        exit_code,
                        signal,
                    } => {
                        map.serialize_entry("card", "terminal")?;
                        if let Some(title) = title {
                            map.serialize_entry("title", title)?;
                        }
                        if let Some(output) = output {
                            map.serialize_entry("output", output)?;
                        }
                        if let Some(exit_code) = exit_code {
                            map.serialize_entry("exitCode", exit_code)?;
                        }
                        if let Some(signal) = signal {
                            map.serialize_entry("signal", signal)?;
                        }
                    }
                    ToolResultView::Diff { title, diffs } => {
                        map.serialize_entry("card", "diff")?;
                        if let Some(title) = title {
                            map.serialize_entry("title", title)?;
                        }
                        map.serialize_entry("diffs", diffs)?;
                    }
                    ToolResultView::Search(_)
                    | ToolResultView::Web(_)
                    | ToolResultView::Read(_) => {
                        unreachable!()
                    }
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ToolResultView {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("tool result view must be an object"))?;
        match object.get("card").and_then(JsonValue::as_str) {
            Some("generic") => Ok(ToolResultView::Generic {
                title: object
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                content: object
                    .get("content")
                    .map(|value| {
                        serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)
                    })
                    .transpose()?,
            }),
            Some("terminal") => Ok(ToolResultView::Terminal {
                title: object
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                output: object
                    .get("output")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                exit_code: object.get("exitCode").and_then(JsonValue::as_i64),
                signal: object
                    .get("signal")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
            }),
            Some("diff") => Ok(ToolResultView::Diff {
                title: object
                    .get("title")
                    .and_then(JsonValue::as_str)
                    .map(str::to_string),
                diffs: serde_json::from_value(
                    object
                        .get("diffs")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::Array(Vec::new())),
                )
                .map_err(serde::de::Error::custom)?,
            }),
            Some("search") => Ok(ToolResultView::Search(
                serde_json::from_value(value).map_err(serde::de::Error::custom)?,
            )),
            Some("read") => Ok(ToolResultView::Read(
                serde_json::from_value(value).map_err(serde::de::Error::custom)?,
            )),
            Some("web") => Ok(ToolResultView::Web(
                serde_json::from_value(value).map_err(serde::de::Error::custom)?,
            )),
            Some(other) => Err(serde::de::Error::custom(format!(
                "unknown tool result card {other:?}"
            ))),
            None => Err(serde::de::Error::custom("tool result view requires a card")),
        }
    }
}

/// The completed outcome handed to `ToolDefinition.presentResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    /// The final model-facing content (or the rendered error text on failure).
    pub content: Vec<ContentBlock>,
    /// Whether the call failed.
    #[serde(rename = "isError")]
    pub is_error: bool,
    /// The tool-private presentation payload projected by its output
    /// declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonValue>,
}
