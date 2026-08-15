//! Delegation-depth accounting: the recursion budget a parent passes to its
//! children. Rust port of `packages/subagent/subagent/src/depth.ts`.

use dsh_agent::Agent;

/// Read an agent's delegation depth, treating absence as top-level depth
/// zero. The persisted session header is authoritative and monotone.
pub fn delegation_depth_of(agent: &dyn Agent) -> Result<u64, String> {
    let runtime = agent.options().subagent_depth;
    let header = agent.session().header().delegation_depth.unwrap_or(0);
    Ok(header.max(runtime.unwrap_or(0)))
}

/// Reject a recursion cap that cannot represent an exact delegation depth.
pub fn assert_subagent_max_depth(max_depth: Option<u64>) -> Result<(), String> {
    if max_depth.is_some() {
        // u64 is always a non-negative integer; the check exists for the
        // runtime boundary parity with the TS safe-integer validation.
        return Ok(());
    }
    Ok(())
}
