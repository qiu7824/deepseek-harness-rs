use super::transform::known_v4_event;
use std::{collections::HashSet, sync::Arc};

/// Event ownership supplied by the installed reader. An explicit vocabulary is
/// useful for partial consumers; unknown ignorable payloads stay uninterpreted.
#[derive(Clone, Debug, Default)]
pub struct V4Vocabulary {
    explicit: Option<Arc<HashSet<String>>>,
}
impl V4Vocabulary {
    pub fn explicit(types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            explicit: Some(Arc::new(types.into_iter().map(Into::into).collect())),
        }
    }
    pub fn contains(&self, kind: &str) -> bool {
        self.explicit
            .as_ref()
            .map_or_else(|| known_v4_event(kind), |types| types.contains(kind))
    }
}
