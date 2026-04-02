---
id: SEED-020
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next milestone — any work on cron, rightcron skill, or agent responsiveness
scope: tiny
---

# SEED-020: Cron reconciler — 10s interval with jitter instead of 60s

## Why This Matters

`run_cron_task` reconciles `crons/*.yaml` every 60 seconds (`cron.rs:281`). When an agent
adds or modifies a cron job (via `/rightcron` skill), it takes up to 60 seconds before the
change is picked up. That's a terrible feedback loop — the agent writes the file and nothing
appears to happen.

10 seconds is a much better interactive interval. Jitter (±2–3s) prevents multiple agents
from stampeding the filesystem simultaneously if they're reconciling at the same time.

## When to Surface

**Trigger:** Next milestone — surface whenever touching `crates/bot/src/cron.rs` or the
rightcron skill.

## Scope Estimate

**Tiny** — Two-line change in `run_cron_task`:
```rust
// before
let mut interval = tokio::time::interval(Duration::from_secs(60));

// after
let jitter = rand::thread_rng().gen_range(0..3);
let mut interval = tokio::time::interval(Duration::from_secs(10 + jitter));
```

Requires adding `rand` dependency (already may be in workspace — check first).

## Breadcrumbs

- `crates/bot/src/cron.rs:281` — `Duration::from_secs(60)` — the line to change
- `crates/bot/src/cron.rs:275–291` — full `run_cron_task` function context
- `Cargo.toml` workspace — check if `rand` is already a dependency before adding

## Notes

- Alternative: use `tokio::time::interval` with `MissedTickBehavior::Skip` (already default)
  so bursts don't cause catch-up storms
- Consider making the interval configurable via `agent.yaml` (`cron_reconcile_secs: 10`)
  as a future follow-up — but 10s hardcoded is good enough for now
