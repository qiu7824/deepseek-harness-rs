//! User-only requirement editing while all execution admission is quiescent.
use super::*;

impl TaskExecution {
    pub(super) fn revise_with_control(
        &self,
        agent: &Arc<dyn dsh_agent::Agent>,
        cwd: &str,
        id: &str,
        mut revision: UserRevision,
    ) -> std::result::Result<Value, TaskActionError> {
        let owner = agent.id().as_str();
        let _root_control = agent.try_idle_control().map_err(|_| RevisionError::Busy)?;
        let registry = self
            .context
            .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .ok_or_else(|| TaskActionError::Failed("Agent registry unavailable".into()))?;
        if registry
            .get(agent.id())
            .is_none_or(|current| !Arc::ptr_eq(&current, agent))
        {
            return Err(RevisionError::Busy.into());
        }
        // Parent admission is already held by the HTTP lease. Descendants need
        // their own execution gates because scheduled/delegated work bypasses HTTP.
        let all = registry.list();
        let mut descendants = Vec::new();
        let mut pending = vec![agent.clone()];
        let mut seen = BTreeSet::from([agent.id().clone()]);
        while let Some(parent) = pending.pop() {
            for child in &all {
                if !seen.contains(child.id()) && registry.is_owned_by(child.id(), &parent) {
                    seen.insert(child.id().clone());
                    descendants.push(child.clone());
                    pending.push(child.clone());
                }
            }
        }
        let mut _descendant_controls = Vec::new();
        for child in &descendants {
            _descendant_controls.push(child.try_idle_control().map_err(|_| RevisionError::Busy)?);
        }
        let subagents = self
            .context
            .get_typed::<Arc<dsh_subagent::SubagentRuntime>>("subagents", false);
        let terminals = self
            .context
            .get_typed::<Arc<dsh_terminal::TerminalSessionService>>("terminals", false);
        let jobs = self
            .context
            .get_typed::<Arc<dyn dsh_jobs::JobRegistry>>("jobs", false);
        let computer = self
            .context
            .get_typed::<Arc<dsh_tool_computer_use_command::ComputerUseRuntime>>(
                "computerUse",
                false,
            );
        for controlled in std::iter::once(agent).chain(descendants.iter()) {
            if subagents.as_ref().is_some_and(|runtime| {
                runtime.try_has_pending_descendants(controlled) != Some(false)
            }) || terminals
                .as_ref()
                .is_some_and(|runtime| runtime.has_owner_activity(controlled))
                || jobs
                    .as_ref()
                    .is_some_and(|runtime| runtime.has_owner_activity(controlled))
                || computer
                    .as_ref()
                    .is_some_and(|runtime| runtime.has_owner_activity(controlled))
            {
                return Err(RevisionError::Busy.into());
            }
        }
        revision.spec.environment_fingerprint = self.environment(owner, cwd)?;
        let task = self
            .validation_work
            .with_idle(owner, || {
                self.runtime.revise_by_user(
                    owner,
                    id,
                    &revision.key,
                    revision.expected,
                    revision.spec,
                    revision.mode,
                )
            })
            .ok_or(RevisionError::Busy)??;
        Ok(task_response(task))
    }
}
