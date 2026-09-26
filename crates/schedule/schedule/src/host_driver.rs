//! One Host driver; idle tasks retain neither Agents nor their history.
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cordis::arc;
use dsh_llm::{ContentBlock, MessageSource, create_user_message};
use tokio::sync::watch;

use crate::calendar::{
    format_instant, parse_instant, render_recurring_batch, render_reminder, resolve_occurrence,
};
use crate::host_history::append_delivery;
use crate::host_service::{ScheduleService, accepted};
use crate::host_store::{HostStore, TaskIndex};
use crate::host_types::{DeliveryReceipt, ScheduleError, TaskStatus};

pub(crate) async fn run(service: Arc<ScheduleService>, mut generation: watch::Receiver<u64>) {
    let owned_generation = *generation.borrow_and_update();
    let store = match service.store().await {
        Ok(store) => store,
        Err(error) => {
            warn(&service, &[], &error);
            return;
        }
    };
    let mut failed = HashSet::new();
    let mut handled_request = 0;
    loop {
        let wake = service.wake.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let requested = service.request_revision.load(Ordering::SeqCst);
        if requested != handled_request {
            failed.clear();
            handled_request = requested;
        }
        if !service.enabled() || *generation.borrow() != owned_generation {
            return;
        }
        let due = store.due((service.clock)(), &failed);
        let mut handled = HashSet::new();
        for candidate in &due {
            if handled.contains(&candidate.id) {
                continue;
            }
            let group: Vec<_> = if candidate.recurring {
                due.iter()
                    .filter(|entry| entry.recurring && entry.session_id == candidate.session_id)
                    .cloned()
                    .collect()
            } else {
                vec![candidate.clone()]
            };
            handled.extend(group.iter().map(|entry| entry.id.clone()));
            if !service.enabled() || *generation.borrow() != owned_generation {
                return;
            }
            // Restore before taking the service gate: archive/delete take the
            // same session admission first and must never wait behind a resolver.
            let admission = tokio::select! {
                biased;
                _ = generation.changed() => return,
                result = service.controller.acquire(&candidate.session_id, true) => result,
            };
            let admission = match admission {
                Ok(admission) => admission,
                Err(error) => {
                    fail_group(&service, &store, &group, &mut failed, &error);
                    continue;
                }
            };
            let guard = tokio::select! {
                biased;
                _ = generation.changed() => return,
                guard = service.gate.clone().lock_owned() => guard,
            };
            if !service.enabled() || *generation.borrow() != owned_generation {
                return;
            }
            let current_service = service.clone();
            let current_store = store.clone();
            let claimed = group.clone();
            // Once admitted, follow-up + flush + receipt commits are one owned
            // operation. Disabling drains it instead of cancelling mid-publication.
            let result = accepted(async move {
                let _guard = guard;
                let now = (current_service.clock)();
                let mut tasks = Vec::new();
                for index in &claimed {
                    if !current_store.matches(index) {
                        continue;
                    }
                    if let Some(task) = current_store.read(&index.id)? {
                        if task.status == TaskStatus::Active
                            && parse_instant(task.record.scheduled_at())? <= now
                        {
                            let occurrence = resolve_occurrence(&task.record, now)?;
                            tasks.push((task, occurrence));
                        }
                    }
                }
                if tasks.is_empty() {
                    return Ok(());
                }
                let text = if tasks[0].0.record.is_recurring() {
                    render_recurring_batch(
                        &tasks
                            .iter()
                            .map(|(task, occurrence)| {
                                (&task.record, occurrence.scheduled_at.as_str())
                            })
                            .collect::<Vec<_>>(),
                    )
                } else {
                    render_reminder(&tasks[0].0.record)
                };
                let message = create_user_message(
                    vec![ContentBlock::Text { text }],
                    MessageSource::Plugin {
                        plugin: "schedule".into(),
                        form: None,
                        sections: None,
                        summary: None,
                        compaction_id: None,
                        source_command_id: None,
                    },
                );
                let message_id = message.id.to_string();
                admission.deliver(message).await?;
                let delivered_at = format_instant((current_service.clock)())?;
                let retention = current_service.config().retention();
                for (task, occurrence) in tasks {
                    let receipt = DeliveryReceipt {
                        scheduled_at: occurrence.scheduled_at.clone(),
                        delivered_at: delivered_at.clone(),
                        message_id: message_id.clone(),
                    };
                    let mut next = append_delivery(&task, &receipt, &retention)?;
                    next.status = if occurrence.next_scheduled_at.is_some() {
                        TaskStatus::Active
                    } else {
                        TaskStatus::Inactive
                    };
                    if task.record.is_recurring() {
                        next.record = task.record.with_scheduled_at(
                            occurrence
                                .next_scheduled_at
                                .unwrap_or(occurrence.scheduled_at),
                        );
                    }
                    current_store.put(&next).await?;
                    current_service.changed();
                }
                drop(admission);
                Ok(())
            })
            .await;
            if let Err(error) = result {
                fail_group(&service, &store, &group, &mut failed, &error);
            }
        }
        if !service.enabled() || *generation.borrow() != owned_generation {
            return;
        }
        // Recheck wall time at least every 30 seconds, including after sleep or
        // a system-clock change. Failed tasks do not become a zero-delay loop.
        let delay = store
            .next_target(&failed)
            .map(|target| target.saturating_sub((service.clock)()).clamp(1, 30_000) as u64)
            .unwrap_or(30_000);
        tokio::select! {
            biased;
            _ = generation.changed() => return,
            _ = &mut wake => {},
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {},
        }
    }
}

fn fail_group(
    service: &ScheduleService,
    store: &HostStore,
    group: &[TaskIndex],
    failed: &mut HashSet<String>,
    error: &ScheduleError,
) {
    let now = (service.clock)();
    // A partial batch commit advances its index revision; only uncommitted
    // members remain failed, while committed members retain their next timer.
    let pending: Vec<_> = group
        .iter()
        .filter(|entry| store.matches(entry) && entry.scheduled_at <= now)
        .map(|entry| entry.id.clone())
        .collect();
    failed.extend(pending.iter().cloned());
    warn(service, &pending, error);
}

fn warn(service: &ScheduleService, ids: &[String], error: &ScheduleError) {
    service.ctx.logger.warn(
        &service.ctx,
        vec![arc(format!(
            "schedule: {} task(s) were not acknowledged; ids={:?}: {}",
            ids.len(),
            &ids[..ids.len().min(32)],
            error
        ))],
    );
}
