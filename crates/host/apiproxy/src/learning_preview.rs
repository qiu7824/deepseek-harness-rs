//! Read-only, bounded metadata for experience previews of detached sessions.
use dsh_session::{SessionEvent, SessionId};
use dsh_session_persistence::{SessionPersistenceApi, SessionReadWindowRequest};
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc, time::Duration};

#[derive(Clone, Default)]
pub(crate) struct HistoryBasis {
    pub cwd: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub tool_source: &'static str,
    pub model_source: &'static str,
    pub limited: bool,
}

pub(crate) fn from_events(cwd: String, events: &[SessionEvent], limited: bool) -> HistoryBasis {
    let mut basis = HistoryBasis {
        cwd,
        tool_source: "unavailable",
        model_source: "unknown",
        limited,
        ..HistoryBasis::default()
    };
    let selected = events
        .iter()
        .rev()
        .find(|event| event.type_ == "model/selection" && route(&event.data).is_some());
    let requested = events
        .iter()
        .rev()
        .find_map(|event| match event.type_.as_str() {
            "request/context" => route(&event.data),
            "request/header" => route(&event.data["header"]["config"]),
            _ => None,
        });
    let route = selected.and_then(|event| route(&event.data)).or(requested);
    if let Some((provider, model)) = route {
        basis.provider = Some(provider);
        basis.model = Some(model);
        basis.model_source = if selected.is_some() {
            "stored-selection"
        } else {
            "last-request"
        };
    }
    if let Some(header) = events
        .iter()
        .rev()
        .find(|event| event.type_ == "request/header")
        .and_then(|event| event.data.get("header"))
        .filter(|header| header.is_object())
    {
        basis.tool_source = "last-request";
        if let Some(tools) = header.get("tools").and_then(Value::as_array) {
            basis.limited |= tools.len() > 256;
            basis.tools = tools
                .iter()
                .take(256)
                .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                .filter(|name| !name.is_empty() && name.len() <= 256)
                .map(str::to_string)
                .collect();
        }
    }
    basis
}
fn route(value: &Value) -> Option<(String, String)> {
    let provider = value.get("provider")?.as_str()?;
    let model = value.get("model")?.as_str()?;
    if provider.is_empty() || model.is_empty() || provider.len() > 512 || model.len() > 512 {
        return None;
    }
    Some((provider.into(), model.into()))
}

#[derive(Default)]
pub(crate) struct HistoryCache {
    // Serialize cache misses as well as snapshot checks. Concurrent UI polls
    // cannot multiply history decode memory, and no raw messages are cached.
    entries: tokio::sync::Mutex<VecDeque<(String, String, HistoryBasis)>>,
}
impl HistoryCache {
    pub async fn read(
        &self,
        persistence: Arc<dyn SessionPersistenceApi>,
        id: &SessionId,
    ) -> Result<HistoryBasis, String> {
        tokio::time::timeout(Duration::from_secs(5), self.read_inner(persistence, id))
            .await
            .map_err(|_| "历史经验预览读取超时，请稍后重试".to_string())?
    }
    async fn read_inner(
        &self,
        persistence: Arc<dyn SessionPersistenceApi>,
        id: &SessionId,
    ) -> Result<HistoryBasis, String> {
        let mut entries = self.entries.lock().await;
        let snapshot = persistence
            .read_snapshot(id)
            .await
            .map_err(|_| "无法读取持久化会话索引".to_string())?
            .ok_or("未找到持久化会话")?;
        let revision = snapshot.revision.to_string();
        if let Some(index) = entries.iter().position(|(key, cached_revision, _)| {
            key == id.as_str() && cached_revision == &revision
        }) {
            let entry = entries.remove(index).expect("cached entry");
            let basis = entry.2.clone();
            entries.push_back(entry);
            return Ok(basis);
        }
        let cwd = snapshot
            .header
            .cwd
            .clone()
            .filter(|cwd| !cwd.trim().is_empty())
            .ok_or("会话没有工作区")?;
        // Legacy JSONL backends may materialize their complete log internally.
        // Large legacy artifacts stay explicit/unknown instead of risking an
        // unbounded read every five-second preview poll.
        let oversized_legacy = persistence
            .locate(&snapshot.header)
            .is_some_and(|location| {
                location.path.ends_with(".jsonl")
                    && std::fs::metadata(&location.path)
                        .is_ok_and(|metadata| metadata.len() > 16 * 1024 * 1024)
            });
        let basis = if oversized_legacy {
            from_events(cwd, &[], true)
        } else {
            let window = persistence
                .read_window(
                    id,
                    SessionReadWindowRequest {
                        before_seq: None,
                        max_messages: 64,
                        max_events: 4096,
                    },
                )
                .await
                .map_err(|_| "无法读取有界会话历史".to_string())?;
            from_events(
                cwd,
                &window.events,
                window.has_more || window.oversized_event_count.is_some(),
            )
        };
        let after = persistence
            .read_snapshot(id)
            .await
            .map_err(|_| "无法核对会话历史版本".to_string())?;
        if after
            .as_ref()
            .is_none_or(|after| after.revision != snapshot.revision)
        {
            return Err("会话历史正在更新，请稍后重试".into());
        }
        entries.retain(|(key, _, _)| key != id.as_str());
        if entries.len() >= 32 {
            entries.pop_front();
        }
        entries.push_back((id.to_string(), revision, basis.clone()));
        Ok(basis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(kind: &str, seq: usize, data: Value) -> SessionEvent {
        serde_json::from_value(json!({"type":kind,"seq":seq,"time":seq,"data":data})).unwrap()
    }
    #[test]
    fn stored_selection_and_last_header_are_distinct_read_only_evidence() {
        let events = vec![
            event(
                "request/header",
                1,
                json!({"header":{"config":{"provider":"a","model":"a"},"tools":[{"name":"old"}]}}),
            ),
            event(
                "request/header",
                2,
                json!({"header":{"config":{"provider":"b","model":"b"},"tools":[{"name":"glob"}]}}),
            ),
            event("model/selection", 3, json!({"provider":"c","model":"c"})),
            event("tool/result", 4, json!({"name":"untrusted-tool"})),
        ];
        let basis = from_events("workspace".into(), &events, true);
        assert_eq!(basis.provider.as_deref(), Some("c"));
        assert_eq!(basis.model_source, "stored-selection");
        assert_eq!(basis.tools, vec!["glob"]);
        assert_eq!(basis.tool_source, "last-request");
        assert!(basis.limited);
    }
    #[test]
    fn missing_header_does_not_invent_current_tools_or_route() {
        let basis = from_events(
            "workspace".into(),
            &[event("tool/call", 1, json!({"name":"glob"}))],
            false,
        );
        assert!(basis.tools.is_empty());
        assert_eq!(basis.tool_source, "unavailable");
        assert!(basis.provider.is_none());
        let basis = from_events(
            "workspace".into(),
            &[event(
                "request/context",
                2,
                json!({"provider":"p","model":"m"}),
            )],
            false,
        );
        assert_eq!(basis.model.as_deref(), Some("m"));
        assert_eq!(basis.model_source, "last-request");
        assert!(basis.tools.is_empty());
    }
}
