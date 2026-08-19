use super::*;

fn agents(entries: &[(&str, &[(&str, &str)])]) -> Vec<(String, Vec<HeldProvider>)> {
    entries
        .iter()
        .map(|(agent, held)| {
            (
                (*agent).to_string(),
                held.iter()
                    .map(|(name, owner)| HeldProvider::new(*name, *owner))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn refcount_keeps_record_when_borrower_remains() {
    let agents = agents(&[
        ("agent-a", &[("fal-a1b2c3", "agent-a")]),
        ("right", &[("fal-a1b2c3", "agent-a")]),
    ]);
    let plan = plan_destroy_provider_cascade("agent-a", &agents, true);
    assert_eq!(plan.detach, vec!["fal-a1b2c3"]);
    assert!(plan.delete.is_empty(), "still referenced by right");
    assert_eq!(
        plan.rehome_owner_to.get("fal-a1b2c3").map(String::as_str),
        Some("right")
    );
}

#[test]
fn refcount_deletes_record_when_last_reference() {
    let agents = agents(&[("agent-a", &[("fal-a1b2c3", "agent-a")])]);
    let plan = plan_destroy_provider_cascade("agent-a", &agents, true);
    assert_eq!(plan.delete, vec!["fal-a1b2c3"]);
    assert!(plan.rehome_owner_to.is_empty());
}

#[test]
fn refcount_borrower_delete_keeps_record_no_rehome() {
    // Deleting a borrower while the owner survives: the record stays and
    // nothing is re-homed — the borrower never owned it.
    let agents = agents(&[
        ("agent-a", &[("fal-a1b2c3", "agent-a")]),
        ("right", &[("fal-a1b2c3", "agent-a")]),
    ]);
    let plan = plan_destroy_provider_cascade("right", &agents, true);
    assert_eq!(plan.detach, vec!["fal-a1b2c3"]);
    assert!(plan.delete.is_empty());
    assert!(plan.rehome_owner_to.is_empty());
}

#[test]
fn refcount_fails_closed_when_siblings_incomplete() {
    // Partial sibling enumeration must never delete or re-home: an unread
    // agent may still reference the record.
    let agents = agents(&[("agent-a", &[("fal-a1b2c3", "agent-a")])]);
    let plan = plan_destroy_provider_cascade("agent-a", &agents, false);
    assert_eq!(plan.detach, vec!["fal-a1b2c3"]);
    assert!(plan.delete.is_empty());
    assert!(plan.rehome_owner_to.is_empty());
}

#[test]
fn unknown_agent_yields_an_empty_plan() {
    let agents = agents(&[("agent-a", &[("fal-a1b2c3", "agent-a")])]);
    let plan = plan_destroy_provider_cascade("ghost", &agents, true);
    assert_eq!(plan, DestroyProviderPlan::default());
}

#[test]
fn rehome_picks_the_first_listed_survivor_deterministically() {
    let agents = agents(&[
        ("agent-a", &[("fal-a1b2c3", "agent-a")]),
        ("alpha", &[("fal-a1b2c3", "agent-a")]),
        ("beta", &[("fal-a1b2c3", "agent-a")]),
    ]);
    let plan = plan_destroy_provider_cascade("agent-a", &agents, true);
    assert_eq!(
        plan.rehome_owner_to.get("fal-a1b2c3").map(String::as_str),
        Some("alpha")
    );
}

#[test]
fn share_rejects_sharing_with_self() {
    let err = plan_share("agent-a", "agent-a", "fal-a1b2c3", &[]).unwrap_err();
    assert!(
        matches!(&err, StoreError::ShareConflict { reason }
            if reason == "cannot share a provider with the owning agent itself"),
        "got {err:?}"
    );
}

#[test]
fn share_rejects_a_record_the_destination_already_declares() {
    let dest = vec![HeldProvider::new("fal-a1b2c3", "someone")];
    let err = plan_share("agent-a", "right", "fal-a1b2c3", &dest).unwrap_err();
    assert!(
        matches!(&err, StoreError::ShareConflict { reason }
            if reason == "destination agent already has provider \"fal-a1b2c3\""),
        "got {err:?}"
    );
}

#[test]
fn share_accepts_a_fresh_destination() {
    plan_share("agent-a", "right", "fal-a1b2c3", &[]).expect("fresh destination");
}

#[test]
fn unshare_rejects_an_owned_record() {
    let owned = HeldProvider::new("fal-a1b2c3", "right");
    let err = plan_unshare("right", &owned).unwrap_err();
    assert!(
        matches!(&err, StoreError::ShareConflict { reason }
            if reason.contains("use remove, not unshare")),
        "got {err:?}"
    );
}

#[test]
fn unshare_accepts_a_borrowed_record() {
    let borrowed = HeldProvider::new("fal-a1b2c3", "agent-a");
    plan_unshare("right", &borrowed).expect("borrowed records can be unshared");
}

#[test]
fn true_owner_of_a_borrowed_reference_is_the_owning_agent() {
    let borrowed = HeldProvider::new("fal-a1b2c3", "agent-a");
    assert_eq!(true_owner(&borrowed), "agent-a");
}
