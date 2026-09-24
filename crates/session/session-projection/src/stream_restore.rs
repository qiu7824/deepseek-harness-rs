use super::*;

struct RestoreUnit {
    definition: ProjectionDefinition,
    state: ArcValue,
    after_seq: i64,
}

/// Private reconstruction state. Events are folded and released immediately;
/// no historical event vector or live Session is retained by this object.
pub struct ProjectionStreamRestore {
    units: Vec<RestoreUnit>,
    next_seq: i64,
    end_seq: i64,
    failed: bool,
}

impl SessionProjectionRegistry {
    /// Create a restore for one revision-qualified, inclusive event range.
    /// The caller must verify the source revision again before publishing.
    pub fn prepare_stream_restore(
        &self,
        header: &SessionHeader,
        checkpoint: &ProjectionCheckpoint,
        base_seq: i64,
        end_seq: i64,
    ) -> Result<ProjectionStreamRestore, String> {
        if base_seq < 0
            || end_seq < -1
            || end_seq > 9_007_199_254_740_991
            || base_seq > end_seq.saturating_add(1)
        {
            return Err("invalid projection restore range".into());
        }
        let mut units = Vec::new();
        for registration in self.registrations.lock().values() {
            let definition = &registration.def;
            let row = checkpoint.get(&definition.key).filter(|row| {
                row.ver == definition.state_version && row.seq >= base_seq - 1 && row.seq <= end_seq
            });
            if row.is_none() && base_seq > 0 {
                return Err(format!(
                    "projection {} needs a complete replay",
                    definition.key
                ));
            }
            units.push(RestoreUnit {
                definition: definition.clone(),
                state: row
                    .map(|row| arc(row.val.clone()))
                    .unwrap_or_else(|| (definition.init)(header)),
                after_seq: row.map_or(base_seq - 1, |row| row.seq),
            });
        }
        Ok(ProjectionStreamRestore {
            units,
            next_seq: base_seq,
            end_seq,
            failed: false,
        })
    }

    /// A plugin reload must not publish a checkpoint calculated by superseded
    /// definitions, even if a faulty plugin reused its old numeric version.
    pub fn stream_restore_is_current(&self, restore: &ProjectionStreamRestore) -> bool {
        let registrations = self.registrations.lock();
        registrations.len() == restore.units.len()
            && restore.units.iter().all(|unit| {
                registrations
                    .get(&unit.definition.key)
                    .is_some_and(|registration| {
                        let a = &registration.def;
                        let b = &unit.definition;
                        a.state_version == b.state_version
                            && Arc::ptr_eq(&a.init, &b.init)
                            && Arc::ptr_eq(&a.apply, &b.apply)
                            && Arc::ptr_eq(&a.view, &b.view)
                            && Arc::ptr_eq(&a.schema, &b.schema)
                    })
            })
    }
}

impl ProjectionStreamRestore {
    pub fn push(&mut self, event: &SessionEvent) -> Result<(), String> {
        if self.failed {
            return Err("projection restore already failed".into());
        }
        if event.seq.get() != self.next_seq as u64 || self.next_seq > self.end_seq {
            self.failed = true;
            return Err("projection source is not the expected contiguous revision".into());
        }
        for unit in &mut self.units {
            if self.next_seq > unit.after_seq {
                unit.state = (unit.definition.apply)(&unit.state, event);
            }
        }
        self.next_seq += 1;
        Ok(())
    }

    pub fn finish(self) -> Result<(ProjectionSnapshot, ProjectionCheckpoint), String> {
        if self.failed || self.next_seq != self.end_seq + 1 {
            return Err("incomplete projection restore cannot be published".into());
        }
        let mut values = serde_json::Map::new();
        let mut checkpoint = ProjectionCheckpoint::new();
        for unit in self.units {
            let definition = unit.definition;
            let value = (definition.schema)(&(definition.view)(&unit.state))?;
            let state: Arc<ProjectionValue> =
                cordis::downcast_arc(&unit.state).ok_or("projection state must be plain JSON")?;
            values.insert(definition.key.clone(), value);
            checkpoint.insert(
                definition.key,
                ProjectionCheckpointRow {
                    ver: definition.state_version,
                    seq: self.end_seq,
                    val: state.as_ref().clone(),
                },
            );
        }
        Ok((
            ProjectionSnapshot {
                as_of_seq: self.end_seq,
                values,
            },
            checkpoint,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn definition() -> ProjectionDefinition {
        ProjectionDefinition {
            key: "sum".into(),
            state_version: 2,
            init: Arc::new(|_| arc(json!(0))),
            apply: Arc::new(|state, event| {
                arc(json!(
                    downcast::<Value>(state).unwrap().as_u64().unwrap()
                        + event.data.as_u64().unwrap()
                ))
            }),
            view: Arc::new(|state| state.clone()),
            schema: Arc::new(|value| Ok(downcast::<Value>(value).unwrap().clone())),
        }
    }
    fn event(seq: u64) -> SessionEvent {
        serde_json::from_value(json!({"type":"feedback/record","seq":seq,"time":0,"data":seq+1}))
            .unwrap()
    }
    #[tokio::test]
    async fn streamed_and_materialized_restores_match_for_cold_warm_and_stale_checkpoints() {
        let ctx = Context::root();
        let registry = SessionProjectionRegistry::install(&ctx);
        registry.register(&ctx, definition()).unwrap();
        let session = Session::create(dsh_session::session_id("stream"), None, None, None).unwrap();
        let events: Vec<_> = (0..1000).map(event).collect();
        let (_, initial) = registry
            .restore(
                session.header(),
                &ProjectionCheckpoint::new(),
                &events[..31],
                0,
            )
            .unwrap();
        let mut stale = initial.clone();
        stale["sum"].ver = 1;
        for (rows, base) in [(ProjectionCheckpoint::new(), 0), (initial, 30), (stale, 0)] {
            let expected = registry
                .restore(session.header(), &rows, &events[base as usize..], base)
                .unwrap();
            let mut stream = registry
                .prepare_stream_restore(session.header(), &rows, base, 999)
                .unwrap();
            for row in &events[base as usize..] {
                stream.push(row).unwrap();
            }
            assert!(registry.stream_restore_is_current(&stream));
            assert_eq!(stream.finish().unwrap(), expected);
        }
    }
    #[tokio::test]
    async fn missing_duplicate_out_of_range_and_unfinished_streams_are_refused() {
        let ctx = Context::root();
        let registry = SessionProjectionRegistry::install(&ctx);
        registry.register(&ctx, definition()).unwrap();
        let session = Session::create(dsh_session::session_id("stream"), None, None, None).unwrap();
        for rows in [vec![0, 2], vec![0, 0], vec![0, 1, 2]] {
            let mut stream = registry
                .prepare_stream_restore(session.header(), &ProjectionCheckpoint::new(), 0, 1)
                .unwrap();
            assert!(
                rows.into_iter()
                    .try_for_each(|seq| stream.push(&event(seq)))
                    .is_err()
            );
            assert!(stream.finish().is_err());
        }
        let mut stream = registry
            .prepare_stream_restore(session.header(), &ProjectionCheckpoint::new(), 0, 1)
            .unwrap();
        stream.push(&event(0)).unwrap();
        assert!(stream.finish().is_err());
    }
    #[tokio::test]
    async fn definition_replacement_invalidates_an_inflight_restore() {
        let ctx = Context::root();
        let registry = SessionProjectionRegistry::install(&ctx);
        let release = registry.register(&ctx, definition()).unwrap();
        let session = Session::create(dsh_session::session_id("reload"), None, None, None).unwrap();
        let stream = registry
            .prepare_stream_restore(session.header(), &ProjectionCheckpoint::new(), 0, -1)
            .unwrap();
        assert!(registry.stream_restore_is_current(&stream));
        release().await;
        registry.register(&ctx, definition()).unwrap();
        assert!(!registry.stream_restore_is_current(&stream));
    }
}
