//! Schedule's Host bridge shares the same admission and cold-resume ownership
//! boundary as ordinary session controls.

use super::*;
use dsh_schedule::host_service::{
    ScheduleService, ScheduleSessionController, ScheduleSessionLease,
};
use dsh_schedule::host_types::ScheduleError;

fn failure(code: &str, message: impl Into<String>) -> ScheduleError {
    ScheduleError {
        code: code.into(),
        message: message.into(),
    }
}

pub(super) fn rpc_error(error: ScheduleError) -> RpcError {
    RpcError::ScheduleRejected(RpcErrorBody {
        message: error.message,
        details: crate::api::rpc::ReasonDetails { reason: error.code },
    })
}

struct ScheduleController(std::sync::Weak<ApiProxyService>);

struct SessionLease {
    owner: std::sync::Weak<ApiProxyService>,
    control: Option<ControlAgentLease>,
    _admission: Option<tokio::sync::OwnedMutexGuard<()>>,
}

#[async_trait::async_trait]
impl ScheduleSessionController for ScheduleController {
    async fn acquire(
        &self,
        session_id: &str,
        wake: bool,
    ) -> Result<Box<dyn ScheduleSessionLease>, ScheduleError> {
        let owner = self
            .0
            .upgrade()
            .ok_or_else(|| failure("session_unavailable", "The Host is shutting down."))?;
        let id = dsh_session::session_id(session_id);
        let admission = owner.resolver.admission(&id).lock_owned().await;
        owner.validate_schedule_session(&id, !wake).await?;
        if wake {
            let control = owner
                .resolve_admitted_control_agent(&id, admission)
                .await
                .map_err(|message| failure("session_unavailable", message))?;
            Ok(Box::new(SessionLease {
                owner: Arc::downgrade(&owner),
                control: Some(control),
                _admission: None,
            }))
        } else {
            Ok(Box::new(SessionLease {
                owner: Arc::downgrade(&owner),
                control: None,
                _admission: Some(admission),
            }))
        }
    }
}

#[async_trait::async_trait]
impl ScheduleSessionLease for SessionLease {
    async fn deliver(&self, message: dsh_llm::UserMessage) -> Result<(), ScheduleError> {
        let owner = self
            .owner
            .upgrade()
            .ok_or_else(|| failure("session_unavailable", "The Host is shutting down."))?;
        let control = self
            .control
            .as_ref()
            .ok_or_else(|| failure("session_unavailable", "A delivery lease is required."))?;
        // The current Agent trait is infallible while its inbox implementation
        // can reject an append. A rejected append must never produce a receipt.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            control.agent.followup(message)
        }))
        .map_err(|_| {
            failure(
                "persistence_failed",
                "The reminder could not be queued durably.",
            )
        })?;
        dsh_schedule::flush_schedule_persistence(&owner.ctx, control.agent.session())
            .await
            .map_err(|error| failure("persistence_failed", error.to_string()))
    }
}

impl ApiProxyService {
    pub fn schedule_session_controller(self: &Arc<Self>) -> Arc<dyn ScheduleSessionController> {
        Arc::new(ScheduleController(Arc::downgrade(self)))
    }

    pub(super) fn schedule_service(&self) -> Option<Arc<ScheduleService>> {
        self.ctx
            .get_typed::<Arc<ScheduleService>>("schedule", false)
            .map(|slot| slot.as_ref().clone())
    }

    pub(super) fn scheduled_task_service(&self) -> Option<Arc<dsh_schedule_host::ScheduleService>> {
        self.ctx
            .get_typed::<Arc<dsh_schedule_host::ScheduleService>>("scheduledTasks", false)
            .map(|slot| slot.as_ref().clone())
    }

    async fn validate_schedule_session(
        &self,
        id: &dsh_session::SessionId,
        require_cold_restore: bool,
    ) -> Result<(), ScheduleError> {
        if self
            .workspace_registry()
            .is_some_and(|registry| registry.archived_session_ids().contains(id))
        {
            return Err(failure(
                "session_archived",
                "An archived session cannot receive reminders.",
            ));
        }
        let live = self.agents().and_then(|agents| agents.get(id));
        let header = if let Some(agent) = &live {
            agent.session().header().clone()
        } else if let Some(session) = self.sessions().and_then(|sessions| sessions.get(id)) {
            session.header().clone()
        } else {
            let persistence = self
                .ctx
                .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                    "sessionPersistence",
                    false,
                )
                .map(|slot| slot.as_ref().clone())
                .ok_or_else(|| {
                    failure(
                        "session_unavailable",
                        "Session persistence is not available.",
                    )
                })?;
            persistence
                .read_snapshot(id)
                .await
                .map_err(|message| failure("session_unavailable", message))?
                .ok_or_else(|| {
                    failure(
                        "session_not_found",
                        format!("Session \"{id}\" does not exist."),
                    )
                })?
                .header
        };
        if header.id != *id {
            return Err(failure(
                "session_unavailable",
                "Session authority does not match the requested identity.",
            ));
        }
        if crate::agent_lookup::has_api_remote_subagent_owner(&self.ctx, &header, live.as_ref()) {
            return Err(failure(
                "session_unauthorized",
                "Reminders must be bound to a root session.",
            ));
        }
        if require_cold_restore && header.cwd.is_none() {
            return Err(failure(
                "session_unavailable",
                "This session has no saved working directory and cannot be restored for a reminder.",
            ));
        }
        Ok(())
    }

    pub(super) async fn schedule_rpc(
        &self,
        rpc_id: RpcId,
        method: &str,
        payload: serde_json::Value,
        signal: AbortSignal,
    ) -> RpcResponse<serde_json::Value> {
        let Some(service) = self.schedule_service() else {
            return err(
                rpc_id,
                rpc_error(failure(
                    "schedule_unavailable",
                    "Reminder management is not available.",
                )),
            );
        };
        if signal.aborted() {
            return err(
                rpc_id,
                RpcError::Cancelled(RpcErrorBody {
                    message: "Reminder request was cancelled.".into(),
                    details: EmptyDetails {},
                }),
            );
        }
        let cancellation_signal = signal.clone();
        let cancelled: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || cancellation_signal.aborted());
        let operation = async {
            macro_rules! payload {
                ($ty:ty) => {
                    serde_json::from_value::<$ty>(payload)
                        .map_err(|error| failure("invalid_rule", error.to_string()))?
                };
            }
            let value = match method {
                "schedule.catalog" => serde_json::to_value(service.catalog().await?),
                "schedule.list" => {
                    let request = payload!(crate::api::schedule::ScheduleSessionRequest);
                    serde_json::to_value(service.list(&request.session_id).await?)
                }
                "schedule.create" => {
                    let request = crate::api::schedule::decode_create_request(&payload)?;
                    serde_json::to_value(
                        service
                            .create_with_signal(
                                &request.session_id,
                                request.reminder,
                                cancelled.clone(),
                            )
                            .await?,
                    )
                }
                "schedule.update" => serde_json::to_value(
                    service
                        .update_with_signal(
                            dsh_schedule::calendar::decode_update_request(&payload)?,
                            cancelled.clone(),
                        )
                        .await?,
                ),
                "schedule.delete" => {
                    let request = payload!(crate::api::schedule::ScheduleDeleteRequest);
                    serde_json::to_value(
                        service
                            .delete_with_signal(&request.session_id, &request.id, cancelled.clone())
                            .await?,
                    )
                }
                "schedule.history" => serde_json::to_value(
                    service
                        .history(crate::api::schedule::decode_history_request(&payload)?)
                        .await?,
                ),
                "schedule.retry" => {
                    service.request_drive();
                    Ok(serde_json::json!({"requested":true,"enabled":service.enabled()}))
                }
                _ => unreachable!("closed schedule method registry"),
            };
            value.map_err(|error| failure("internal_error", error.to_string()))
        };
        // Service mutations shield the accepted store commit. Dropping an RPC
        // only cancels acquisition/FIFO waits, never a write already accepted.
        tokio::select! {
            biased;
            _ = signal.cancelled() => err(rpc_id, RpcError::Cancelled(RpcErrorBody { message: "Reminder request was cancelled.".into(), details: EmptyDetails {} })),
            result = operation => match result { Ok(value) => ok(rpc_id, value), Err(error) => err(rpc_id, rpc_error(error)) },
        }
    }
}
