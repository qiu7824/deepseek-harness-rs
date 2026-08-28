use std::collections::HashSet;

/// In-process, session-scoped approval grants keyed by opaque policy values.
#[derive(Default)]
pub struct GrantStore {
    entries: parking_lot::Mutex<HashSet<(String, String)>>,
}

impl GrantStore {
    pub fn is_granted(&self, session_id: &str, key: &str) -> bool {
        self.entries
            .lock()
            .contains(&(session_id.to_string(), key.to_string()))
    }

    pub fn grant(&self, session_id: &str, key: &str) {
        self.entries
            .lock()
            .insert((session_id.to_string(), key.to_string()));
    }

    pub fn clear_session(&self, session_id: &str) {
        self.entries.lock().retain(|(id, _)| id != session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::GrantStore;

    #[test]
    fn grants_are_session_scoped() {
        let store = GrantStore::default();
        store.grant("session-a", "write:D:/workspace");
        assert!(store.is_granted("session-a", "write:D:/workspace"));
        assert!(!store.is_granted("session-b", "write:D:/workspace"));
        assert!(!store.is_granted("session-a", "write:D:/other"));
        store.clear_session("session-a");
        assert!(!store.is_granted("session-a", "write:D:/workspace"));
    }
}
