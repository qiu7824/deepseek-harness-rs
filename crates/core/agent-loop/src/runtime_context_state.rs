//! Replayable named runtime facts and dependency-aware delta projection.
use dsh_llm::{
    ContentBlock, ContextForm, ContextSnapshotSection, MessageSource, ProducerMessageSource,
    UserMessage, create_user_message,
};
use dsh_session::{SessionEvent, SurfaceOp, is_replacement_surface_event};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

pub(super) const SOURCE: &str = "@deepseek-ai/dsh-system-prompt";
const CLEARED: &str =
    "Current runtime context: none. Earlier runtime-context snapshots and changes no longer apply.";

#[derive(Clone)]
struct Retained {
    base_id: String,
    last_id: String,
    text: Option<String>,
    sections: Option<BTreeMap<String, String>>,
    dependencies: HashSet<u64>,
}
#[derive(Default)]
pub(super) struct ProjectionState {
    pub seen: bool,
    retained: Option<Retained>,
}

pub(super) fn owned_message(event: &SessionEvent) -> Option<UserMessage> {
    if event.type_ != "user/message" {
        return None;
    }
    let message: UserMessage = serde_json::from_value(event.data.clone()).ok()?;
    (message.source.plugin_name() == Some(SOURCE)).then_some(message)
}
fn named(sections: &[ContextSnapshotSection]) -> Option<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for section in sections {
        if section.name.is_empty()
            || result
                .insert(section.name.clone(), section.text.clone())
                .is_some()
        {
            return None;
        }
    }
    Some(result)
}
fn text(message: &UserMessage) -> Option<String> {
    match message.content.as_slice() {
        [ContentBlock::Text { text }] => Some(text.clone()),
        _ => None,
    }
}
fn change_text(changed: &[ContextSnapshotSection], removed: &[String]) -> String {
    let mut body = "Runtime context changes. Replace only the named sections below; keep all other runtime facts in effect.".to_string();
    for section in changed {
        body.push_str(&format!("\n\n### {}\n{}", section.name, section.text));
    }
    if !removed.is_empty() {
        body.push_str(&format!(
            "\n\nRemoved runtime sections (no longer active): {}",
            serde_json::to_string(removed).unwrap()
        ));
    }
    body
}
impl ProjectionState {
    /// A replacement can itself be a user/message (including compaction).
    /// Invalidate its old dependencies before classifying the new producer.
    pub fn replace(&mut self, event: &SessionEvent) {
        if !is_replacement_surface_event(event) {
            return;
        }
        let Some(retained) = &self.retained else {
            return;
        };
        let referenced = event
            .source_event_seqs
            .as_ref()
            .is_some_and(|seqs| seqs.iter().any(|seq| retained.dependencies.contains(seq)));
        let overlapping = match event.surface_op {
            Some(SurfaceOp::Replace { start, end }) if start <= end => retained
                .dependencies
                .iter()
                .any(|seq| *seq >= start && *seq <= end),
            Some(SurfaceOp::Replace { .. }) => true,
            _ => false,
        };
        if referenced || overlapping {
            self.retained = None;
        }
    }

    pub fn observe(&mut self, seq: u64, message: UserMessage) {
        self.seen = true;
        let source = serde_json::to_value(&message.source).unwrap_or(Value::Null);
        if source["contextDeltaVersion"] == 1 {
            let Some(previous) = self.retained.as_ref() else {
                return;
            };
            let valid = source["contextBaseId"].as_str() == Some(&previous.base_id)
                && source["contextPreviousId"].as_str() == Some(&previous.last_id);
            let removed =
                serde_json::from_value::<Vec<String>>(source["removedSections"].clone()).ok();
            let changed =
                serde_json::from_value::<Vec<ContextSnapshotSection>>(source["sections"].clone())
                    .ok()
                    .filter(|sections| {
                        removed.as_ref().is_some_and(|removed| {
                            text(&message).as_deref()
                                == Some(change_text(sections, removed).as_str())
                        })
                    })
                    .and_then(|sections| named(&sections));
            let Some((changed, removed)) = changed.zip(removed).filter(|(changed, removed)| {
                valid && removed.iter().all(|name| !changed.contains_key(name))
            }) else {
                self.retained = None;
                return;
            };
            let previous = self.retained.as_mut().unwrap();
            let Some(sections) = previous.sections.as_mut() else {
                self.retained = None;
                return;
            };
            for name in removed {
                sections.remove(&name);
            }
            sections.extend(changed);
            previous.last_id = message.id.as_str().into();
            previous.text = None;
            previous.dependencies.insert(seq);
            return;
        }
        let body = text(&message);
        let sections =
            serde_json::from_value::<Vec<ContextSnapshotSection>>(source["sections"].clone())
                .ok()
                .filter(|sections| {
                    body.as_deref()
                        == Some(dsh_system_prompt::join_context_sections(sections).as_str())
                })
                .and_then(|sections| named(&sections))
                .or_else(|| (body.as_deref() == Some(CLEARED)).then(BTreeMap::new));
        self.retained = Some(Retained {
            base_id: message.id.as_str().into(),
            last_id: message.id.as_str().into(),
            text: body,
            sections,
            dependencies: HashSet::from([seq]),
        });
    }

    pub fn project(
        &self,
        current: &str,
        sections: &[ContextSnapshotSection],
    ) -> Option<UserMessage> {
        if !self.seen && current.is_empty() {
            return None;
        }
        let snapshot = if current.is_empty() { CLEARED } else { current };
        if self
            .retained
            .as_ref()
            .and_then(|state| state.text.as_deref())
            == Some(snapshot)
        {
            return None;
        }
        if !sections.is_empty() && current == dsh_system_prompt::join_context_sections(sections) {
            if let Some((previous, known, new)) = self
                .retained
                .as_ref()
                .and_then(|previous| previous.sections.as_ref().map(|known| (previous, known)))
                .zip(named(sections))
                .map(|((previous, known), new)| (previous, known, new))
            {
                let changed: Vec<_> = sections
                    .iter()
                    .filter(|section| known.get(&section.name) != Some(&section.text))
                    .cloned()
                    .collect();
                let removed: Vec<_> = known
                    .keys()
                    .filter(|name| !new.contains_key(*name))
                    .cloned()
                    .collect();
                if changed.is_empty() && removed.is_empty() {
                    return None;
                }
                let body = change_text(&changed, &removed);
                let fields=json!({"form":"notice","contextDeltaVersion":1,"contextBaseId":previous.base_id,"contextPreviousId":previous.last_id,"sections":changed,"removedSections":removed,"summary":format!("{} 项更新，{} 项移除",changed.len(),removed.len())}).as_object().unwrap().clone();
                return Some(create_user_message(
                    vec![ContentBlock::Text { text: body }],
                    MessageSource::Producer(ProducerMessageSource {
                        kind: "runtime-context".into(),
                        fields,
                    }),
                ));
            }
        }
        Some(create_user_message(
            vec![ContentBlock::Text {
                text: snapshot.into(),
            }],
            MessageSource::Plugin {
                plugin: SOURCE.into(),
                form: (!sections.is_empty()).then_some(ContextForm::Snapshot),
                sections: (!sections.is_empty()).then(|| sections.to_vec()),
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        ))
    }
}
