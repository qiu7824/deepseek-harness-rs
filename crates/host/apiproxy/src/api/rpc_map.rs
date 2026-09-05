//! RPC method registry. The map registers only client-request methods
//! (respond is a client-response, so it is absent); map keys are the wire
//! path segments (POST /api/session.list). Rust port of
//! `packages/host/apiproxy/src/api/rpc-map.ts`.
//!
//! The TS map keys signatures as the single source of truth; the Rust
//! contract layer carries the closed method-name table (lexically sorted —
//! the TS declaration is grouped by domain, the set is identical), while
//! the carrier layer (later milestone) owns the per-method dispatch.

/// Every client-request method, lexically sorted (62 methods).
pub const CLIENT_REQUEST_METHODS: &[&str] = &[
    "agentPreset.copy",
    "agentPreset.list",
    "agentPreset.openDocument",
    "agentPreset.read",
    "agentPreset.remove",
    "agentPreset.select",
    "capabilities.list",
    "capabilities.serverRemove",
    "capabilities.serverSave",
    "capabilities.serverTest",
    "capabilities.serverToggle",
    "capabilities.skillRead",
    "capabilities.skillRemove",
    "capabilities.skillSave",
    "capabilities.skillToggle",
    "commands.execute",
    "commands.list",
    "credentials.describe",
    "credentials.set",
    "credentials.unset",
    "goal.clear",
    "goal.complete",
    "goal.create",
    "goal.edit",
    "goal.pause",
    "goal.resume",
    "host.createDirectory",
    "host.describe",
    "host.listDirectory",
    "host.openPath",
    "host.pickDirectory",
    "llm.discoverModels",
    "llm.models",
    "llm.providers",
    "memory.categories",
    "memory.learningConfigure",
    "memory.learningConfirm",
    "memory.learningList",
    "memory.learningPreview",
    "memory.learningRemove",
    "memory.learningToggle",
    "memory.list",
    "memory.remove",
    "memory.upsert",
    "messageFeedback.delete",
    "messageFeedback.list",
    "messageFeedback.put",
    "pluginInventory.list",
    "pluginInventory.setEnabled",
    "session.attachment",
    "session.cancel",
    "session.create",
    "session.fork",
    "session.history",
    "session.list",
    "session.models",
    "session.prompt",
    "session.rename",
    "session.search",
    "session.selectModel",
    "session.updateQueue",
    "session.updateTodos",
    "settings.describe",
    "settings.mutate",
    "settings.openDocument",
    "settings.replace",
    "settings.update",
    "skill.list",
    "subagent.history",
    "subagent.interrupt",
    "subagent.list",
    "subagent.prompt",
    "workspace.archiveSession",
    "workspace.create",
    "workspace.delete",
    "workspace.deleteArchivedSession",
    "workspace.insertBefore",
    "workspace.insertSessionBefore",
    "workspace.list",
    "workspace.rename",
    "workspace.unarchiveSession",
];

/// Whether `method` is a registered client-request method (binary search
/// over the sorted table).
pub fn is_client_request_method(method: &str) -> bool {
    CLIENT_REQUEST_METHODS.binary_search(&method).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_methods_are_sorted_unique_and_include_todo_updates() {
        assert_eq!(CLIENT_REQUEST_METHODS.len(), 81);
        for method in [
            "memory.learningList",
            "memory.learningConfigure",
            "memory.learningToggle",
            "memory.learningRemove",
            "memory.learningConfirm",
            "memory.learningPreview",
        ] {
            assert!(is_client_request_method(method));
        }
        assert!(
            CLIENT_REQUEST_METHODS
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert!(is_client_request_method("session.updateTodos"));
        assert!(is_client_request_method("capabilities.serverTest"));
        assert!(is_client_request_method("capabilities.skillToggle"));
    }
}
