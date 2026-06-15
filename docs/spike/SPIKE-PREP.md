Confirmed: the file twins are written from the same constants via `write_regenerated`, so they are byte-identical. The schema inventory is fully verified. The evidence is accurate; my only refinement is the file-twin-vs-constant nuance (which the evidence's `definition_kind` already captures correctly as json-file vs json-literal).

I have everything needed. The MiMo claims are settled by primary-source verdicts in the evidence (C1 partial, C2/C3/C4 confirmed; the structuredDeep finding independently corroborates the linchpin). The verdicts override findings where they conflict — most importantly C1's correction about the SDK `format` field. Let me reconcile that contradiction carefully before writing, because it's load-bearing for the whole brief.

The contradiction: the `runCap` finding claims `SessionPromptData.body.format` exists on the SDK (C8, citing types.gen.ts:4426-4427), while the C1 verdict says "there is no structured-output/json-schema `format` field on the SDK's `session.prompt` body at all." The verdict wins per the rules — but I need to state this precisely because it changes the fork surface. Let me note both and lead with the verdict's conclusion while flagging the contradiction as an explicit UNKNOWN for the spike, since C8 cites a specific line the verdict author may not have checked (the verdict checked the `dev`-branch sst SDK shape, C8 cites the MiMo v2 gen types). This is exactly the kind of thing the spike must resolve live.

I now have enough to write the brief.

---

# SPIKE-PREP BRIEF — `mimo run` Capability Map + RightClaw Schema Inventory

**Purpose:** Hands-on inputs for the adopt-MiMo vs build-rig spike. Sandbox/egress are off the table (OpenShell owns isolation). Claude stays on spawn-real-`claude -p` (subscription-OAuth self-attach is ToS-prohibited); this spike targets **non-Claude/self-host models** (DeepSeek/Kimi/GLM/vLLM) where MiMo would carry the harness.

> **Verdict precedence:** Where a verified verdict contradicts a raw finding, the verdict wins. The C1 verdict materially corrects the finding's claim about an SDK `format` field — see §1 and the flagged UNKNOWN in §4.

---

## 1. ARTIFACT 1 — `mimo run` capability map

### THE LINCHPIN: per-turn json-schema structured output

**Answer: `mimo run` as shipped CANNOT request per-turn json-schema structured output — but this does NOT force a standalone HTTP daemon.** The capability is gated behind a per-call `format` field that `run` never passes; the same engine runs in-process on the run path.

The mechanism exists and is fully wired in the session engine:
- Structured output triggers only when `lastUser.format?.type === "json_schema"`. That check installs a synthetic `StructuredOutput` tool, pushes `STRUCTURED_OUTPUT_SYSTEM_PROMPT`, and sets `toolChoice: "required"` — [prompt.ts#L2556-L2558](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/prompt.ts#L2556-L2558), [#L2793](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/prompt.ts#L2793) (**C4, C5 — both CONFIRMED verbatim against source**).
- The schema carries `retryCount` (default 2) for a bounded repair loop — [message-v2.ts#L75-L83](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/message-v2.ts#L75-L83) (**C6**).
- When `format` is absent it defaults to `{ type: "text" }`, so none of that machinery runs (sst [prompt.ts#L1334](https://github.com/sst/opencode/blob/dev/packages/opencode/src/session/prompt.ts#L1334), MiMo [#L2588](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/prompt.ts#L2588)).

**The gap — `run` never sets `format`.** The non-interactive handler issues exactly one prompt call: `sdk.session.prompt({ sessionID, agent, model, variant, parts })` — no `format`, no `system`. (**C1 verdict: PARTIAL.** The substance is confirmed — one prompt call, no `format`, no `system` — but the verdict corrects two precision defects: the finding's cited line range `#L661-668` does not exist on `main` [the file is ~521 lines, call sits ~L500-507], and there is **no structured-output/json-schema `format` field on the sst SDK `session.prompt` body at all** [SessionPromptData fields: `messageID, model, agent, noReply, system, tools, parts`]. So "never sets a format field" is, on that SDK shape, *vacuously* true.) The `--command` (slash) path also has no `format` (**C16**).

**The `--format default|json` flag is OUTPUT RENDERING ONLY** — it gates raw event JSON to stdout (`if (args.format === "json") process.stdout.write(...)`), never forwarded to the prompt. Event types: `tool_use / step_start / step_finish / text / reasoning / error`. ([run.ts#L417-L420](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/cli/cmd/run.ts#L417-L420), **C9 CONFIRMED**). **No run flag, config setting, or agent-config pins an output schema** — nothing in `prompt.ts` reads `format` from `opencode.json`/`mimocode` config or an agent definition; `format` originates only from the per-call `PromptInput.format` carried into the user message via `format: input.format` ([prompt.ts#L1295](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/prompt.ts#L1295)).

**The HTTP daemon is NOT forced.** Local (non-`--attach`) `mimo run` builds an **in-process** server: `createOpencodeClient({ baseUrl: "http://opencode.internal", fetch: fetchFn })` where `fetchFn` delegates to `Server.Default().app.fetch(request)`. `Server.Default = lazy(() => create({}))` registers all Hono routes but **never calls `.listen()`** — port-binding `listen()` is a separate, unused function. (**C2, C3 — both CONFIRMED**; the `--attach` branch is the only path hitting a real network server.) [run.ts#L685-L691](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/cli/cmd/run.ts#L685-L691), [server.ts#L34](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/server/server.ts#L34).

So: **the per-agent HTTP-server operability burden the user thought `mimo run` removed is genuinely NOT reintroduced** — neither for structured output nor MCP nor system prompt. The engine is in-process either way. The blocker is purely the run *command's argument surface*.

### MCP on the run path — **YES, via config, no flag needed**

MCP servers auto-load from the resolved per-directory config: `const config = cfg.mcp ?? {}` from `Config.get()`, each server connected, and every connected MCP tool pulled into the LLM tool set via `for (const [key, item] of Object.entries(yield* mcp.tools()))` in the same in-process prompt loop `run` uses ([prompt.ts#L720](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/prompt.ts#L720), **C10 CONFIRMED**). Config is resolved per-directory (merging `dir/.claude.json` and `dir/.mcp.json`), and `mimo run --dir <path>` does `process.chdir(args.dir)` **before** `bootstrap(process.cwd(), ...)`, so resolved config (incl. `mcp`) follows the run directory (**C11**). `run.ts` has no `--mcp-config`/`--strict-mcp-config` equivalent — the WebFetch "MCP not supported as flags" claim is technically true but misleading; **MCP works, just via config not flags.** RightClaw's per-agent aggregator MCP config therefore loads on the run path with zero changes.

> Verifier note: the `structuredDeep` pass marked MCP-on-run "unknown" only because it was scoped to the linchpin; it incidentally confirmed `MCP.Service` is provided into the in-process prompt loop, consistent with C10. No contradiction — `runCap` traced it fully.

### System prompt on the run path — **only static `--agent`; per-turn dynamic needs a fork**

`LLM.buildSystemArray` is the single composition point. Order: `agent.prompt` (or `SystemPrompt.provider(model)` if no agent prompt) → `input.system` → `user.system` → an always-appended memory-instructions section (skipped only for system-spawned actors) ([llm.ts#L283-L296](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/llm.ts#L283-L296), **C12 CONFIRMED**).

Implications for RightClaw:
- **`--agent <name>` gives a static-per-agent base prompt** that replaces only the built-in *provider* prompt (either/or with `SystemPrompt.provider`).
- **Per-turn dynamic content is supported via the per-call `system` field** — it flows onto the user message as `system: input.system` ([prompt.ts#L1294-1295](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/prompt.ts#L1294)) and is **appended** after the agent prompt (**C13**). But `mimo run` never passes `system`, so as-shipped `run` can set only the static `--agent` prompt, NOT RightClaw's per-turn composite (which changes every turn with chat context).
- **It is ADDITIVE, not a replacement.** Unlike `claude -p --system-prompt-file` (full byte-control replacement), MiMo always layers your `system` on top of the agent/provider prompt **and** the always-on memory-instructions section. No run/SDK field is known to fully suppress the built-in agent prompt + memory section. This is a semantic mismatch RightClaw must resolve (see UNKNOWN in §4).

### Drop-in verdict

**`mimo run --format json` is NOT a complete `claude -p` drop-in AS SHIPPED — but the HTTP server is NOT forced, and the gap is two missing fields on an already-in-process SDK call.**

| RightClaw per-turn need | `mimo run` as shipped | In-process engine supports it? |
|---|---|---|
| MCP from per-agent config | ✅ Works today, no flag | ✅ Yes (config-resolved) |
| json-schema structured output | ❌ `run` never sets `format` | ⚠️ Mechanism exists; field-on-SDK status DISPUTED (see §4) |
| Per-turn dynamic system prompt | ❌ `run` never sets `system`; only static `--agent` | ⚠️ `system` accepted but ADDITIVE, not replacement |

**Recommended path for the spike:** prototype a **~30-line custom in-process driver** that reuses `bootstrap()` + `Server.Default().app.fetch` + `sdk.session.prompt(...)` and adds `format` + `system` to the call — rather than patching `run.ts` (decouples RightClaw from run.ts's UI/event-rendering code, which is an *older* opencode fork — pin the MiMo commit, current upstream opencode 1.17.4 adds `--interactive` MiMo lacks, **C15**). **No HTTP daemon either way.**

**Hard caveat carried into the spike:** the structured-output path is a **tool emulation** (`StructuredOutput` tool + `toolChoice: "required"` + retry loop), NOT native provider `response_format`/`json_schema`. Conformance is a **model+adapter property** — some OpenAI-compatible/vLLM backends ignore or reject `toolChoice: "required"`. The A/B must measure, per target model, (a) whether the adapter honors `toolChoice: required` and (b) schema-conformance reliability of the emitted tool call. This is NOT settled by source.

---

## 2. ARTIFACT 2 — RightClaw schema inventory (the conformance test set)

All schemas verified against source at the current working directory. **Total: 6 distinct per-turn schemas** across 9 session-bearing callsites; 2 callsites pass **no schema** (`json_schema: None`). Passed to `claude -p` as the **full JSON string** (not a path) via `--json-schema` at [invocation.rs#L586-L589](file:///Users/developer/dev/rightclaw/crates/bot/src/cc/invocation.rs).

| Callsite | Purpose | defined_at (verified) | Kind | Top-level fields (types) |
|---|---|---|---|---|
| **Foreground worker reply** | Interactive Telegram turn reply | reads file twin `reply-schema.json` at `worker.rs:3038-3071`; constant `REPLY_SCHEMA_JSON` `agent_def.rs:31-65` | json-file (twin of literal) | `content` (string\|null), `reply_to_message_id` (int\|null), `attachments` (array\|null of {type-enum, path, filename, caption, media_group_id}), `used_skill_receipts` (array of {package_name `^rightx-`, message}); **required: [content, used_skill_receipts]** |
| **Bootstrap mode** (first invite) | Onboarding reply + completion flag | reads file twin `bootstrap-schema.json` (selected at `worker.rs:3038-3039`); constant `agent_def.rs:71` | json-file (twin of literal) | REPLY fields **+ `bootstrap_complete` (boolean)**; required: [content, bootstrap_complete] |
| **Reflection** (CC error recovery) | Human summary after failure on `--resume` | reads `reply-schema.json` at `reflection.rs:195`; passed `reflection.rs:205` | json-file | Same as foreground reply |
| **Async delivery** (cron result relay) | Deliver async/cron result | reads `reply-schema.json` at `async_delivery.rs:966-971` | json-file | Same as foreground reply |
| **Cron job execution** | Scheduled task w/ delivery decision | constant `CRON_SCHEMA_JSON` `agent_def.rs:78`; passed inline `cron.rs:567` | json-literal | `delivery` (**oneOf**: notify {kind=`notify`, content minLen1, attachments} \| silent {kind=`silent`, reason minLen1}), `run_note` (string); required: [delivery, run_note] |
| **Background continuation** (fg handoff) | Forked-session bg run; notify-only | constant `BG_CONTINUATION_SCHEMA_JSON` `agent_def.rs:85`; passed inline `background.rs:70` | json-literal | `delivery` (object {kind=`notify`, content minLen1, attachments} — **no silent branch**), `run_note` (string); required: [delivery, run_note] |
| **Learning prefilter** (Haiku) | Classify turn → skip/patch/create | constant `PREFILTER_SCHEMA_JSON` `learning_prefilter.rs:26-47`; passed `learning_prefilter.rs:616` | json-literal | `decision` (enum skip\|patch_existing\|create_new), `target_skill` (string `^rightx-[a-z0-9-]+$`), `topic_hint` (string maxLen120), `reason` (string maxLen400); required: [decision, reason] |
| **Learning probe-writer** | Skill create/patch (writes via MCP) | `learning_probe_writer.rs:123` | **none** (`json_schema: None`) | N/A — natural prose + MCP write tools |
| **Learning curator** | Skill consolidation (writes via MCP) | `learning_curator.rs:582` | **none** (`json_schema: None`) | N/A — natural prose + MCP write tools |

**Definition mechanics (verified):**
- 4 core schemas are JSON-literal string constants in `crates/right-codegen/src/agent_def.rs` (`REPLY`, `BOOTSTRAP`, `CRON`, `BG_CONTINUATION`).
- 1 is an inline const in `crates/bot/src/learning_prefilter.rs` (`PREFILTER`).
- File twins `reply-schema.json` / `bootstrap-schema.json` / `cron-schema.json` are written at init by `pipeline.rs:106-134` via `write_regenerated()` **from the same constants** — confirmed **byte-identical**. `cron-schema.json` file is written but unused on foreground paths (cron reads the constant directly).
- **2 callsites pass no schema** — probe-writer and curator emit prose + MCP file writes, by design.
- **No `schemars` derives.** Every schema is hand-authored JSON text. (Evidence's "schemars derive" mention in the prompt framing does not apply — none exist for these.)

**Most complex schema = the hardest conformance target: `CRON_SCHEMA_JSON`.** It is the only schema with a **`oneOf` discriminated union** (notify vs silent, each with `const` discriminant `kind` and divergent required fields) nesting the attachments array. This is the stress case for the tool-emulation path: a model must (a) pick the right `oneOf` branch, (b) honor `const`-valued discriminants, and (c) satisfy branch-specific `required` + `minLength`. `REPLY_SCHEMA_JSON` is second-hardest (nested arrays of objects with `pattern`-constrained strings). `PREFILTER` is the simplest and the only Haiku-class target. **Run the A/B in that difficulty order: CRON → REPLY/BOOTSTRAP/BG → PREFILTER.**

---

## 3. SPIKE IMPACT — does the design change?

**Step 0 result (settled by this brief):** `mimo run` as shipped fails 2 of 3 RightClaw per-turn needs (json-schema, dynamic system prompt) but passes MCP. **Crucially, the HTTP-daemon operability story the user de-weighted is NOT reintroduced** — the engine runs in-process on the run path; the gap is the run command's argument surface, not the architecture. **You do NOT need to test the standalone HTTP `/session/:id/prompt` server path to get structured output.** The capability lives on the same in-process `session.prompt({format, system})` call.

**So the spike design changes as follows:**

1. **Do NOT spike `mimo run --format json` directly as the harness adapter** — it provably can't carry json-schema or per-turn prompt. Spiking it would only re-confirm the negative.
2. **Spike the in-process SDK driver instead** (~30 lines reusing `bootstrap` + `Server.Default().app.fetch` + `sdk.session.prompt`). This is the real adopt-MiMo integration surface and the honest comparison point against build-rig. It keeps the no-daemon property the user values.
3. **Re-weigh the operability story in MiMo's FAVOR, not against it:** the earlier worry ("HTTP server forced ⇒ per-agent daemon burden") is **refuted by source**. Adopt-MiMo's operability cost is a TS in-process driver + pinned-fork maintenance, NOT a fleet of HTTP daemons. That materially narrows build-rig's operability edge — consistent with the user already de-weighting rig's Rust-control advantage.

**Step 1 feed (the inventory → conformance test):** Artifact 2's 6 schemas are the literal `--json-schema` inputs RightClaw ships today. Feed them verbatim into the in-process `session.prompt({ format: { type: "json_schema", schema: <one of the 6>, retryCount } })` call against each target model (DeepSeek/Kimi/GLM/vLLM). Measure per (model × schema): (a) does the adapter honor `toolChoice: required`, (b) does the model emit the `StructuredOutput` tool call, (c) does the payload validate against the schema, (d) retry count consumed. The `oneOf` CRON schema is the gating pass/fail — if a model can't reliably hit it, that model is unfit for cron/bg under MiMo regardless of how it does on REPLY.

---

## 4. Remaining UNKNOWNs to resolve LIVE in the spike

1. **[CONTRADICTION — highest priority] Does the SDK `session.prompt` body actually expose a `format` field?** The `runCap` finding (C8) cites `SessionPromptData.body.format` at [MiMo types.gen.ts#L4426-L4427](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/sdk/js/src/v2/gen/types.gen.ts#L4426) as present. The **C1 verdict** (checking the **sst `dev`** SDK shape) says there is **no `format` field on the SDK body at all** (only `messageID, model, agent, noReply, system, tools, parts`). These cite **different SDK surfaces** (MiMo v2 gen vs sst dev) and were not reconciled. **This determines the entire fork surface.** If MiMo's v2 SDK already exposes `format`, the driver is a thin call with zero engine patching. If not, the driver must inject `format` at the route/in-process boundary or patch the SDK gen. **Resolve first: read MiMo `packages/sdk/js/src/v2/gen/types.gen.ts` around L4426 directly and confirm whether `format?: OutputFormat` is present on the prompt body.** (Per verdict-precedence, treat the field as NOT guaranteed present until confirmed live.)

2. **`format`/`OutputFormat` exact shape for authoring inputs.** `schema` and `retryCount` are confirmed; whether the union requires/allows a `name`, `strict`, or `description` is in `message-v2.ts` `Format` / `OutputFormatJsonSchema` and was not fully extracted. RightClaw's 6 schemas may need wrapping (e.g. `{ name, schema }`) before they're valid `format` inputs. Read the full `Format` discriminated union before authoring test inputs.

3. **Can the built-in agent prompt + always-on memory-instructions section be fully SUPPRESSED?** RightClaw's composite is a full replacement (`--system-prompt-file`); MiMo only appends `system`. Confirm whether an empty/custom agent prompt + config can null out `SystemPrompt.provider` AND the memory-instructions append, or whether `buildSystemArray` must be patched. If MiMo's `MEMORY.md`/checkpoint instructions leak into RightClaw agents, that's a correctness problem — RightClaw owns the memory contract (Hindsight/file modes).

4. **Tool-emulation conformance on non-Claude models (the actual A/B).** Per model+adapter: does it honor `toolChoice: "required"` at all (some vLLM/OpenAI-compat backends ignore/reject it)? Does it reliably emit the `StructuredOutput` tool call? Does the payload conform — especially to the CRON `oneOf` union? This is unsettleable from source; it IS the spike.

5. **Per-turn cold-start + teardown cost.** RightClaw's per-turn model spawns fresh each turn. Does the in-process server stay alive only for the single prompt, and does `bootstrap`'s 120s checkpoint-writer drain (`cli/bootstrap.ts`) add per-invocation latency/teardown? Measure cold-start + drain vs `claude -p`, and whether `--continue`/`--session` resume is fast enough for per-turn use.

6. **Final structured result extraction from the event stream.** RightClaw parses `claude -p` stream-json today. With MiMo's events (`tool_use/step_start/step_finish/text/reasoning/error`), confirm the validated structured payload is retrievable from the `StructuredOutput` `tool_use` part in-stream vs requiring a separate `session.messages` fetch. The driver's result-extraction shape depends on this.

7. **`/prompt` vs `/prompt_async` route schema parity.** Both validate against `PromptInput.omit({sessionID})` per [session.ts#L954](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/server/routes/instance/session.ts#L954) (**C14**), so both expose `format`+`system` — but the structuredDeep pass flagged it didn't read the actual HTTP route module (only re-exports). Low risk given the in-process driver bypasses HTTP routing, but confirm if the driver ends up calling a route handler rather than the SDK method.

---

**Files verified this session (RightClaw, absolute paths):**
- `/Users/developer/dev/rightclaw/crates/right-codegen/src/agent_def.rs` (L31-85: REPLY/BOOTSTRAP/CRON/BG_CONTINUATION constants)
- `/Users/developer/dev/rightclaw/crates/right-codegen/src/pipeline.rs` (L106-134: file-twin writes from constants — byte-identical confirmed)
- `/Users/developer/dev/rightclaw/crates/right-codegen/src/contract.rs` (L211-219: codegen file registry entries)
- `/Users/developer/dev/rightclaw/crates/bot/src/learning_prefilter.rs` (L26-47 schema, L616 callsite)
- `/Users/developer/dev/rightclaw/crates/bot/src/cc/invocation.rs` (L586-589: `--json-schema` wiring)
- `/Users/developer/dev/rightclaw/crates/bot/src/cron.rs` (L567), `/Users/developer/dev/rightclaw/crates/bot/src/background.rs` (L70), `/Users/developer/dev/rightclaw/crates/bot/src/telegram/worker.rs` (L3038-3071), `/Users/developer/dev/rightclaw/crates/bot/src/async_delivery.rs` (L966-971), `/Users/developer/dev/rightclaw/crates/bot/src/reflection.rs` (L195-205), `/Users/developer/dev/rightclaw/crates/bot/src/learning_probe_writer.rs` (L123), `/Users/developer/dev/rightclaw/crates/bot/src/learning_curator.rs` (L582)