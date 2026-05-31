# Base prompt vs OPERATING_INSTRUCTIONS: boundary invariant

**Date:** 2026-06-01
**Status:** Design approved, pending implementation plan

## Problem

The composite system prompt for session-bearing `claude -p` invocations is
assembled from two pieces that look like they overlap:

- **Base prompt** — `generate_system_prompt()` in
  `crates/right-codegen/src/agent_def.rs`. A Rust `format!` parameterized by
  `(agent_name, sandbox_mode, home_dir)`.
- **OPERATING_INSTRUCTIONS** — `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`,
  compiled in via `include_str!`. Byte-identical for every agent and mode.

The question that motivated this design: *does it make sense to have both?*

## Answer: yes — the split is justified on two independent axes

The separation is **not** topical; it rests on two technical axes, and both are
real:

1. **Parameterized vs static.** The base prompt *must* be a function: it
   interpolates runtime values and branches on `sandbox_mode` (the
   "User-Installed CLI Tools" and "User SSH Access" blocks are openshell-only).
   That cannot be a static `.md` without adding a templating layer.
   OPERATING_INSTRUCTIONS is byte-identical for all agents → a static markdown
   file is correct and is easier to edit, diff, and review as prose.

2. **Universal prefix vs operating-only.** The base prompt is included in
   **every** mode — Normal, **Bootstrap**, and Cron. OPERATING_INSTRUCTIONS is
   included only in Normal and Cron; **Bootstrap mode omits it**
   (`PROMPT_SYSTEM.md` → "Bootstrap mode" = base + BOOTSTRAP.md only). So the
   base prompt carries the minimum every mode needs (including the
   bootstrap turn), while OPERATING carries operating procedure for turns that
   actually serve a user or cron job.

Merging the two mechanisms is therefore the wrong move: it would force either a
templating layer into the markdown or a ~200-line string literal into Rust, and
it would break Bootstrap (which must not receive the operating-only rules).

## The real defect: a leaky boundary (duplication)

Although the split is sound, the current content allocation leaks. In a Normal
composite, the same thing is stated twice:

1. **The "remember / save this / don't forget → `/right-memory` skill" rule** is
   present **verbatim** in both:
   - base §"Identity Files" (`agent_def.rs`, the paragraph beginning
     *"When the user says \"remember\"…"*), and
   - OPERATING §"Your Files" ("When the user says \"remember\", \"save this\",
     or \"don't forget\"…").

   This is the single genuine verbatim duplicate.

2. **Identity files as a topic are split across two sections** of the same
   composite: base §"Identity Files" states *what each file stores*; OPERATING
   §"Your Files" states *how/when to edit them*. This is topic-overlap, not a
   verbatim dup.

There are **no other** base↔OPERATING overlaps. (An earlier reading wrongly
claimed the Memory section was duplicated; the base prompt has no Memory
section. Corrected here.)

This duplication is exactly what `AGENTS.md` → "Prompt-tier brevity" forbids
("avoid duplicating anything already in another section of the same composite
prompt"), and the duplicated tokens are paid on every turn.

## Design

### The invariant (the primary deliverable)

Documented in `PROMPT_SYSTEM.md`, cross-referenced from `AGENTS.md` →
"Prompt-tier brevity":

> The base prompt (`generate_system_prompt`) carries exactly two kinds of
> content: (1) values it interpolates or branches on — `agent_name`,
> `sandbox_mode`, `home_dir`; and (2) the universal minimum every mode needs,
> **including Bootstrap** — platform description, MCP reference, Response Rules,
> and the *purpose* list of the identity files. All other content — static
> operating procedure for Normal/Cron turns — lives only in
> OPERATING_INSTRUCTIONS. No rule appears in both.
>
> Tie-breaker: *does Bootstrap mode need it?* (Bootstrap does not include
> OPERATING.) Yes → base. No → OPERATING.

The documented invariant is the main value of this change; the code edit below
is small and merely brings the current state into line with it.

### Edits

1. **Remove** from base §"Identity Files" the paragraph delegating
   remember/save/don't-forget to the `/right-memory` skill (`agent_def.rs`).
   Its canonical home is OPERATING §"Your Files", which already states it.
   - Effect: the rule disappears from Bootstrap (where it is not needed — a
     bootstrap turn is not a persistence-on-request turn) and stops doubling in
     Normal/Cron.

2. **Keep** in base the identity-file *purpose* list (IDENTITY/SOUL/USER/TOOLS,
   one line each) and the "always-loaded durable context" framing. Bootstrap
   needs this to write IDENTITY.md/SOUL.md.

3. **Do not change** the principled purpose(base) / edit-discipline(OPERATING)
   split, and **do not** move Response Rules or the MCP reference out of base —
   Bootstrap needs them. (YAGNI: no consolidation for aesthetics.)

4. **Update the pinning test.** `agent_def_tests.rs::system_prompt_delegates_remember_routing_to_right_memory_skill`
   currently asserts the base prompt *contains* `/right-memory` — it encodes the
   opposite of this invariant. Refocus it: drop the `/right-memory` needle from
   the base-prompt assertion (keep the identity-framing needles
   "Identity files are always-loaded durable context", "`SOUL.md`",
   "agent-authored durable voice"), and add a needle to
   `operating_instructions_*` (or a new test) asserting OPERATING is the **only**
   carrier of the remember→`/right-memory` routing. Consider an assertion that
   the base prompt does **not** contain `/right-memory`, to lock the invariant.

5. **Update `PROMPT_SYSTEM.md`** to state the invariant and to reflect that the
   remember rule now lives only in OPERATING.

### Non-goals

- Merging the two mechanisms (Rust fn stays, `.md` stays).
- Moving Response Rules / MCP reference out of base.
- Cleaning up any other OPERATING_INSTRUCTIONS sections.
- Touching Bootstrap or Cron content beyond the single removed paragraph.

## Verification

- TDD: first update/extend `agent_def_tests.rs` to encode the new invariant
  (base must NOT carry the remember routing; OPERATING must), confirm the
  no-`/right-memory`-in-base assertion fails against current code, then make the
  edit.
- Targeted: `devenv shell -- cargo test -p right-codegen`.
- Final (mandatory): `devenv shell -- cargo test --workspace`.
- Re-read `crates/right-codegen/src/agent_def_tests.rs` for any other assertion
  that pins the removed paragraph before editing.

## Affected files

- `crates/right-codegen/src/agent_def.rs` (remove one paragraph)
- `crates/right-codegen/src/agent_def_tests.rs` (refocus the pinning test)
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
  (no content change expected — confirm it remains the sole carrier)
- `PROMPT_SYSTEM.md` (document the invariant)
- `AGENTS.md` (cross-reference under "Prompt-tier brevity")
