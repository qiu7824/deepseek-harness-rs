//! Seeding of a continuable child's durable descriptor event. Rust port of
//! `packages/subagent/subagent/src/descriptor-seed.ts`.

use dsh_session::{Session, SessionEvent, SessionId, session_id};

use crate::descriptor::SubagentDescriptorData;

/// Build the child's creation seed: any inherited parent-history prefix
/// followed by one model-hidden, between-turn `descriptor` event.
pub fn seed_descriptor_turn(
    child_id: &SessionId,
    seed: Option<&[SessionEvent]>,
    descriptor: &SubagentDescriptorData,
) -> Result<Vec<SessionEvent>, String> {
    let staged = Session::create(
        session_id(child_id.as_str()),
        seed.map(|events| events.to_vec()),
        None,
    )?;
    staged.append(
        "subagent/descriptor",
        serde_json::to_value(descriptor).expect("descriptor json"),
        None,
    )?;
    Ok(staged.events().iter().cloned().collect())
}
