//! Pure sharing and destroy-cascade planning.
//!
//! Ported from `right_agent::agent::destroy::plan_destroy_provider_cascade`
//! and the `plan_share`/`plan_unshare` pair in `right::internal_api_providers`.
//! Everything here is a function of its arguments: no database, no filesystem,
//! no gateway. The store calls these with rows it has already read inside a
//! transaction; the tests call them with literals.

use std::collections::BTreeMap;

use crate::error::StoreError;

/// One provider reference held by one agent, reduced to what planning needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldProvider {
    /// Record name.
    pub name: String,
    /// The agent that owns the credential behind this reference.
    pub owner_agent: String,
}

impl HeldProvider {
    pub fn new(name: impl Into<String>, owner_agent: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            owner_agent: owner_agent.into(),
        }
    }

    /// True when `holder` is the owner of this reference.
    fn owned_by(&self, holder: &str) -> bool {
        self.owner_agent == holder
    }
}

/// What the destroy cascade must do to provider records when an agent is
/// removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DestroyProviderPlan {
    /// Records to detach from the deleting agent (all of its own references).
    pub detach: Vec<String>,
    /// Records no surviving agent references — safe to delete outright.
    pub delete: Vec<String>,
    /// Record name → surviving agent that becomes the new owner. Populated
    /// only when the deleting agent owned a record others still reference.
    pub rehome_owner_to: BTreeMap<String, String>,
}

/// Decide the provider cascade for destroying `deleting`.
///
/// `agents` is the full set of `(agent_name, its provider references)`,
/// including the agent being deleted. `all_complete` is false when sibling
/// enumeration was partial; the plan then fails closed — detach only, never
/// delete or re-home, so a record an unread agent still references survives.
pub fn plan_destroy_provider_cascade(
    deleting: &str,
    agents: &[(String, Vec<HeldProvider>)],
    all_complete: bool,
) -> DestroyProviderPlan {
    let mut plan = DestroyProviderPlan::default();
    let Some((_, deleting_providers)) = agents.iter().find(|(a, _)| a == deleting) else {
        return plan;
    };
    for entry in deleting_providers {
        plan.detach.push(entry.name.clone());
        if !all_complete {
            continue;
        }
        let others: Vec<&str> = agents
            .iter()
            .filter(|(a, _)| a != deleting)
            .filter(|(_, ps)| ps.iter().any(|p| p.name == entry.name))
            .map(|(a, _)| a.as_str())
            .collect();
        if others.is_empty() {
            plan.delete.push(entry.name.clone());
        } else if entry.owned_by(deleting) {
            // The deleting agent owned a still-referenced record: re-home it to
            // a survivor rather than orphaning the borrowers.
            plan.rehome_owner_to
                .insert(entry.name.clone(), others[0].to_string());
        }
        // Borrowed and still referenced elsewhere: the true owner is another
        // agent, so there is nothing to do beyond the detach.
    }
    plan
}

/// Validate a share request. Rejects sharing into self and sharing a record
/// the destination already declares.
pub fn plan_share(
    owner_agent: &str,
    dest_agent: &str,
    provider: &str,
    dest_providers: &[HeldProvider],
) -> Result<(), StoreError> {
    if owner_agent == dest_agent {
        return Err(StoreError::ShareConflict {
            reason: "cannot share a provider with the owning agent itself".into(),
        });
    }
    if dest_providers.iter().any(|p| p.name == provider) {
        return Err(StoreError::ShareConflict {
            reason: format!("destination agent already has provider \"{provider}\""),
        });
    }
    Ok(())
}

/// A borrowed reference can be unshared; an owned record cannot — that is a
/// remove.
pub fn plan_unshare(holder_agent: &str, entry: &HeldProvider) -> Result<(), StoreError> {
    if entry.owned_by(holder_agent) {
        return Err(StoreError::ShareConflict {
            reason: format!(
                "provider \"{}\" is owned by this agent, not borrowed; use remove, not unshare",
                entry.name
            ),
        });
    }
    Ok(())
}

/// Resolve the *true* owner a re-share must point at.
///
/// Re-sharing a borrowed record points the new borrower at the owning agent,
/// never at the intermediary, so rotation rights and the destroy cascade
/// always resolve to exactly one authority.
pub fn true_owner(entry: &HeldProvider) -> &str {
    &entry.owner_agent
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
