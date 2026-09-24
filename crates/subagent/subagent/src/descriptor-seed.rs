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
    let mut header = dsh_session::snapshot_session_header(child_id, None)?;
    header.is_seeded = seed.is_some_and(|events| !events.is_empty());
    let inherited =
        dsh_session::SessionLogOffset::new(seed.map_or(0, |events| events.len()) as u64)?;
    let staged = Session::create(
        session_id(child_id.as_str()),
        seed.map(|events| events.to_vec()),
        Some(&header),
        Some(inherited),
    )?;
    staged.append(
        "subagent/descriptor",
        serde_json::to_value(descriptor).expect("descriptor json"),
        None,
    )?;
    Ok(staged.events().iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_restore_marker_keeps_a_distinct_child_cut_and_parent_catalog() {
        let parent = Session::create(session_id("parent"), Some(vec![]), None, None).unwrap();
        let before = parent.events();
        let id = session_id("child");
        let descriptor = SubagentDescriptorData::OneShot {
            version: crate::descriptor::SUBAGENT_DESCRIPTOR_VERSION,
            provider: "test".into(),
            label: Some("child work".into()),
        };
        let seed = seed_descriptor_turn(&id, Some(before.as_ref()), &descriptor).unwrap();
        let cut = dsh_session::SessionLogOffset::new(before.len() as u64).unwrap();
        assert_eq!(
            seed[cut.get() as usize].data,
            serde_json::json!({"inherited":true})
        );
        assert_eq!(&seed[..before.len()], before.as_ref());
        let mut header = dsh_session::snapshot_session_header(&id, None).unwrap();
        header.is_seeded = true;
        header.origin = Some("subagent".into());
        header.parent_session = Some(parent.id().clone());
        let child = Session::create(id, Some(seed), Some(&header), Some(cut)).unwrap();
        let mut validator = dsh_session::format_v4::V4Validator::new(
            serde_json::to_value(child.header()).unwrap(),
            cut.get(),
        )
        .unwrap();
        for event in child.events().iter() {
            validator
                .push(&serde_json::to_value(event).unwrap())
                .unwrap();
        }
        validator.finish().unwrap();
        crate::descriptor::record_child_catalog(&parent, &child, &descriptor).unwrap();
        crate::descriptor::record_child_catalog(&parent, &child, &descriptor).unwrap();
        assert_eq!(
            parent
                .events()
                .iter()
                .filter(|event| event.type_ == "subagent/catalog")
                .count(),
            1
        );
        assert_eq!(parent.events().last().unwrap().data["mode"], "one-shot");
    }
}
