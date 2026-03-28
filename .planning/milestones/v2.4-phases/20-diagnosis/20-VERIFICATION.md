---
phase: 20-diagnosis
verified: 2026-03-28T22:30:00Z
status: gaps_found
score: 5/5 must-haves verified (truths from PLAN); 1/3 REQUIREMENTS.md criteria fully satisfied
re_verification: false
gaps:
  - truth: "DIAG-02: Root cause confirmed as sandbox-specific via log comparison (sandbox-on vs --no-sandbox)"
    status: failed
    reason: "No log comparison was produced. The diagnosis explicitly argues the 'works without sandbox' observation is confounded — the --no-sandbox session likely had no rightcron startup_prompt, so SubagentStop never fired. The root cause (CC event loop gap) is not sandbox-specific: it would affect any session running a background agent that completes. REQUIREMENTS.md marks DIAG-02 as completed but the requirement text asks for a sandbox-specific confirmation that does not exist in DIAGNOSIS.md."
    artifacts:
      - path: ".planning/phases/20-diagnosis/DIAGNOSIS.md"
        issue: "States sandbox comparison is NOT a useful test without matching test conditions; no log comparison produced"
    missing:
      - "Either: produce a controlled log comparison (sandbox-on with rightcron vs --no-sandbox with rightcron) and document whether the event loop gap is sandbox-specific or universal"
      - "Or: formally revise DIAG-02 in REQUIREMENTS.md to reflect the new finding — the root cause is a CC event loop bug that manifests regardless of sandbox state, and the diagnosis should say so explicitly"
  - truth: "DIAG-03: Specific config element responsible named from expected candidates (bwrap network rule, socat relay, settings.json section)"
    status: partial
    reason: "The specific element identified is the CC event loop's iv6/M6() gap — not any of the three config elements listed in the requirement. The diagnosis correctly eliminates all three config candidates (bwrap, socat, settings.json). REQUIREMENTS.md marks DIAG-03 complete, but the requirement's enumerated candidates were all eliminated, not one confirmed. The element named is outside the requirement's scope as written."
    artifacts:
      - path: ".planning/REQUIREMENTS.md"
        issue: "DIAG-03 says 'identified (bwrap network rule, socat relay, or settings.json network/filesystem section)' — none of these is the actual root cause"
    missing:
      - "Update REQUIREMENTS.md DIAG-03 to reflect the actual finding: the specific element responsible is the CC event loop idle state (no M6() call path from iv6() when Z===null after SubagentStop)"
      - "This is a requirements revision, not a code fix — the diagnosis is correct, the requirement text is now outdated"
human_verification:
  - test: "Run Confirmation Test A from DIAGNOSIS.md"
    expected: "No Telegram response after sending a message 30 seconds post-SubagentStop with sandbox enabled"
    why_human: "Requires live rightclaw session, TUI observation, and Telegram message send — cannot verify programmatically"
  - test: "Run Confirmation Test B from DIAGNOSIS.md"
    expected: "Telegram response received when message sent while rightcron is still running (within 60s of session start)"
    why_human: "Requires timing a message send against rightcron execution window — live session required"
---

# Phase 20: Diagnosis Verification Report

**Phase Goal:** Root cause of CC sandbox blocking Telegram event processing is identified and confirmed
**Verified:** 2026-03-28T22:30:00Z
**Status:** gaps_found
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths (from PLAN must_haves)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | DIAGNOSIS.md exists at `.planning/phases/20-diagnosis/DIAGNOSIS.md` | VERIFIED | File exists, 216 lines, committed as `1c616e2` |
| 2 | DIAGNOSIS.md names the specific root cause (CC event loop post-SubagentStop, not socat) | VERIFIED | Lines 75-121: Hypothesis A confirmed via cli.js analysis; iv6/M6() gap named explicitly |
| 3 | DIAGNOSIS.md explains why Hypothesis B (socat TCP timeout) is structurally impossible | VERIFIED | Lines 57-73: 5 distinct structural reasons, process topology with network namespace inode confirmed |
| 4 | DIAGNOSIS.md provides at least one concrete confirmation test with exact steps | VERIFIED | Lines 124-154: Two tests (Test A and Test B) with numbered steps and expected results |
| 5 | DIAGNOSIS.md proposes a specific fix approach for Phase 21 | VERIFIED | Lines 157-198: Three options with tradeoffs; Option A recommended with rationale |

**PLAN must-haves score: 5/5 verified**

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `.planning/phases/20-diagnosis/DIAGNOSIS.md` | Root cause analysis and fix proposal | VERIFIED | 216 lines; contains "SubagentStop" (14x), "M6" (13x), "Hypothesis B" (2x), "ELIMINATED" (1x), "Confirmation Test" (5x), "Phase 21" (10x). All PLAN acceptance criteria pass. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| Evidence (right-debug.log + process topology) | Root cause conclusion | CC cli.js event loop analysis | VERIFIED | Lines 79-121: cli.js bundle analysis cited (cc_version 2.1.86, line numbers referenced); `M6()` call paths enumerated |
| Root cause | Fix proposal | Identified workaround mechanism | VERIFIED | Lines 157-198: Option A (persistent background agent) directly follows from the finding that M6() is only called during active step cycles |

### Data-Flow Trace (Level 4)

Not applicable — this phase produces a documentation artifact (DIAGNOSIS.md), not runnable code with data flows.

### Behavioral Spot-Checks

Step 7b: SKIPPED — no runnable entry points produced by this phase. Phase 20 output is a diagnosis document.

---

## Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| DIAG-01 | 20-01-PLAN.md | Developer can identify why CC stops processing Telegram events when sandbox is enabled by analyzing right-debug.log | SATISFIED | DIAGNOSIS.md Section 1 provides exact log timeline (21:21:58, 21:22:21, 21:23:25, 22:22 entries) with event-by-event annotation |
| DIAG-02 | 20-01-PLAN.md | Root cause confirmed as sandbox-specific (log comparison: sandbox on vs --no-sandbox) | NOT SATISFIED | No sandbox-on vs --no-sandbox log comparison produced. DIAGNOSIS.md explicitly argues this comparison is confounded. The root cause (CC event loop gap) is NOT sandbox-specific — it occurs whenever a background agent completes via SubagentStop. REQUIREMENTS.md marks this checked, but the requirement text is not met. |
| DIAG-03 | 20-01-PLAN.md | Specific config element responsible identified (bwrap network rule, socat relay, or settings.json section) | PARTIAL | All three listed candidates are eliminated. The actual root cause is the CC event loop iv6/M6() gap — an internal CC behavior, not a rightclaw config element. The diagnosis is accurate but the requirement's enumerated candidates were written before the investigation; none proved correct. |

### Orphaned Requirements Check

REQUIREMENTS.md maps DIAG-01, DIAG-02, DIAG-03 to Phase 20. All three are claimed by 20-01-PLAN.md. No orphaned requirements.

---

## Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `.planning/REQUIREMENTS.md` | 12-13 | Both DIAG-02 and DIAG-03 marked `[x]` (complete) but the evidence does not satisfy them as written | Warning | Requirements appear complete in tracking but the sandbox-specificity claim (DIAG-02) is unverified and the element identification (DIAG-03) names a different class of problem than specified |

No code anti-patterns — this phase produced no source code.

---

## Human Verification Required

### 1. Confirmation Test A — Immediate Failure After SubagentStop

**Test:** Run `rightclaw up` with sandbox enabled and rightcron startup_prompt active. Wait for "Agent 'Bootstrap rightcron reconciler' completed" in TUI. Wait 30 seconds. Send a Telegram message.

**Expected:** No response. Message queued in `hz`, M6() never called.

**Why human:** Requires live rightclaw session, TUI observation, and Telegram message send. Cannot verify programmatically.

### 2. Confirmation Test B — Working Path Regression Guard

**Test:** Run `rightclaw up` with sandbox enabled and rightcron startup_prompt active. While rightcron is still running (within first 60 seconds), send a Telegram message.

**Expected:** Response received within seconds — message drained during active agent step cycle.

**Why human:** Requires timing a send against rightcron execution window.

### 3. DIAG-02 Sandbox Specificity

**Test:** Run Confirmation Test A with sandbox enabled AND with --no-sandbox (with rightcron startup_prompt active in both sessions). Compare whether the event loop gap occurs in both configurations.

**Expected:** If the gap is universal (not sandbox-specific), both sessions should fail to respond post-SubagentStop. If only sandbox-on fails, then DIAG-02 is satisfied and the root cause has a sandbox component not yet identified.

**Why human:** Requires two controlled live sessions with identical rightcron configuration.

---

## Gaps Summary

The PLAN-level must-haves are fully satisfied: DIAGNOSIS.md exists, is substantive (216 lines), names the correct root cause, eliminates the wrong hypothesis, provides confirmation tests, and proposes a fix for Phase 21. The commit `1c616e2` is verified.

The gaps are at the **requirements layer**, not the artifact layer:

1. **DIAG-02 (sandbox specificity):** The diagnosis found that the root cause is NOT any sandbox config element — it is a CC event loop bug. The `--no-sandbox` "works" observation is called confounded and no controlled comparison was run. REQUIREMENTS.md marks DIAG-02 complete, but sandbox specificity was not proven — it was argued away. Before Phase 21 implements Option A (persistent background agent), it is worth confirming whether the bug also manifests without `--no-sandbox` when rightcron runs identically. If it does, the phase title "sandbox blocking Telegram event processing" is a misnomer and Phase 21 scope needs re-evaluation.

2. **DIAG-03 (config element):** The requirement listed candidate config elements as a hypothesis list. All were eliminated. The actual cause is CC-internal. REQUIREMENTS.md text should be updated to reflect the actual finding.

These gaps do not block Phase 21 from starting with Option A — the diagnosis is sound and the fix direction is clear. But DIAG-02 sandbox specificity should be confirmed early in Phase 21 testing (per the note in DIAGNOSIS.md Section 3).

---

_Verified: 2026-03-28T22:30:00Z_
_Verifier: Claude (gsd-verifier)_
