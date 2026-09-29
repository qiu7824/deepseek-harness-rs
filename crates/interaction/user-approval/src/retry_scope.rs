//! Timed-out approvals are not reopened until the user or turn advances.
use dsh_session::Session;
use std::collections::BTreeMap;

pub(crate) fn category(tool: &str) -> &str {
    match tool {
        "pwsh" | "bash" | "terminal" | "execute_native" | "run_script" => "shell-execution",
        "write" | "write_file" | "edit" | "edit_file" | "str_replace_editor" | "office_write"
        | "file_manage" | "rename_file" | "delete_file" => "file-mutation",
        "read" | "read_file" | "read_image" | "office_read" => "file-read",
        "uu_terminal" => "remote-execution",
        other => other,
    }
}

pub(crate) fn timed_out(session: &Session, tool: &str) -> Result<bool, String> {
    let mut pending = BTreeMap::new();
    let mut blocked = false;
    let category = category(tool);
    let start = session
        .find_event_rev(|event| {
            event.type_ == "turn/start"
                || event.type_ == "user/message" && event.data["source"]["kind"] == "user"
        })?
        .map_or(0, |event| event.seq.get());
    session.visit_events(start, None, |event| {
        if event.type_ == "turn/start"
            || event.type_ == "user/message" && event.data["source"]["kind"] == "user"
        {
            pending.clear();
            blocked = false;
        } else if event.type_ == "approval/asked" {
            if let (Some(id), Some(tool)) =
                (event.data["id"].as_str(), event.data["toolName"].as_str())
            {
                pending.insert(id.to_owned(), self::category(tool).to_owned());
            }
        } else if event.type_ == "approval/decided"
            && let Some(id) = event.data["id"].as_str()
        {
            if pending.remove(id).as_deref() == Some(category)
                && event.data["outcome"] == "timed-out"
            {
                blocked = true;
            }
        }
        Ok(true)
    })?;
    Ok(blocked)
}
