//! Adjacent conversion for explicitly owned in-memory histories and imports.
use super::{V3Dialect, V3ToV4Transform, V4Validator};
use crate::{SessionEvent, SessionHeader, SessionLogOffset};
use serde_json::Value;

pub fn upgrade_v3_events(
    mut header: SessionHeader,
    inherited: SessionLogOffset,
    events: Vec<SessionEvent>,
    children: Vec<Value>,
) -> Result<(SessionHeader, SessionLogOffset, Vec<SessionEvent>), String> {
    header.delegation_depth.get_or_insert(0);
    let mut transform = V3ToV4Transform::new(
        serde_json::to_value(header).map_err(|error| error.to_string())?,
        Some(children),
        Some(inherited.get()),
        V3Dialect::Rust,
    )?;
    let mut rows = Vec::with_capacity(events.len());
    for event in events {
        rows.extend(
            transform.push(serde_json::to_value(event).map_err(|error| error.to_string())?)?,
        );
    }
    let (summary, trailing) = transform.finish()?;
    rows.extend(trailing);
    let mut validation = V4Validator::new(summary.header.clone(), summary.inherited_event_count)?;
    let mut output = Vec::with_capacity(rows.len());
    for row in rows {
        validation.push(&row)?;
        output.push(serde_json::from_value(row).map_err(|error| error.to_string())?);
    }
    validation.finish()?;
    Ok((
        serde_json::from_value(summary.header).map_err(|error| error.to_string())?,
        SessionLogOffset::new(summary.inherited_event_count)?,
        output,
    ))
}
