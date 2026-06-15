# Pi Re-Evaluation for RightClaw — Round 11 (Final)

**Headline:** Round-3's "Pi is disqualified" is **wrong and stays corrected**. Pi is a serious, co-equal contender. But "Pi" is two different products with opposite trade-offs, and the choice between them is the whole decision. Vanilla `earendil-works/pi` is a clean, multi-maintainer, npm-published **embeddable toolkit** that is mechanism-C native — but ships **no MCP and no subagents by design**. `can1357/oh-my-pi` is a feature-complete **harness** that has MCP + subagents + forks + `.claude` drop-in skills, and **is npm-published** (verified this round) — but it ships **TypeScript source as its runtime entry point** (`main: ./src/index.ts`), so it is Bun-oriented and a Node/Rust shop still embeds it as a Bun process or self-compiles. Neither pi has a skill-learning loop or a forced-envelope mechanism, and neither matches RightClaw's *current* every-turn `--json-schema` contract. Both match RightClaw's *emerging* direction (text-reply + tool-call-as-control + MCP side-channels) precisely.

**Verdicts win on contradiction.** Two prior-round verdicts are explicitly overridden by fresh primary-source checks this round: (1) **C20 ("oh-my-pi NOT npm-published") is stale** — superseded by direct npm registry reads. (2) The robustness claim that the nested-stringification failure is "fork-only repaired" (round-4 C4) is **mis-attributed** — superseded by reading the shared validator; the behavior is schema-kind-conditional, not vanilla-vs-fork. Both corrections are detailed below.

---

## 1. Pi ecosystem — what it is

**Repo identity (settled, high confidence).** `badlogic/pi-mono` and `earendil-works/pi` are the **same repository** — same immutable GitHub id `1035029907`, node_id `R_kgDOPbFNkw`; `badlogic/pi-mono` returns HTTP 301 → `earendil-works/pi`; 62.3k stars; `fork:false` (a transfer into the org, not a copy) [verdict C1, https://github.com/badlogic/pi-mono]. So the `pi-mono#2751` Anthropic-OAuth reference and "Mario Zechner's pi" all point at the same vanilla repo. It is a monorepo (root package `pi-monorepo`, README "Pi Agent Harness Mono Repo") publishing four npm packages: `@earendil-works/pi-ai` (unified LLM wire), `@earendil-works/pi-agent-core` (agent runtime/loop/tool-validation), `@earendil-works/pi-coding-agent` (the `pi` CLI + SDK), `@earendil-works/pi-tui` [verdict C2, https://github.com/earendil-works/pi/blob/main/README.md].

**`can1357/oh-my-pi` (`omp`)** is a **hard fork** — its own `@oh-my-pi/*` packages, no `@earendil-works` dependency, ~12.2k stars, very active [C16, https://github.com/can1357/oh-my-pi].

**Round-4's "oh-my-pi is NOT npm-published" is overridden — but with a critical caveat.** I verified this round on the npm registry directly:
- `@oh-my-pi/pi-coding-agent` and `@oh-my-pi/pi-agent-core` both resolve at `dist-tags.latest = 15.12.4`, with **468** and **460** published versions respectively [https://registry.npmjs.org/@oh-my-pi/pi-coding-agent, https://registry.npmjs.org/@oh-my-pi/pi-agent-core].
- **BUT the runtime entry point is uncompiled TypeScript.** Both packages declare `"main": "./src/index.ts"` and `exports["."].import = "./src/index.ts"`; only `dist/types/*.d.ts` (declarations) and the compiled `omp` CLI (`bin.omp = "dist/cli.js"`) ship as JS. A normal Node consumer doing `import {...} from '@oh-my-pi/pi-coding-agent'` resolves to a `.ts` file and fails without a TS-aware runtime (Bun) or a build step.

**Net effect on operability:** round-4's real objection — "you must build from source / vendor it" — is **reduced, not eliminated**. You `npm install` it (dependency-pin, not a git submodule), but you still run it under Bun or `bun build --compile` it yourself, exactly like the `omp` CLI binary. This is meaningfully better than git-vendoring, but it is **not** "drop-in compiled library for a Node/Rust consumer." Evidence: `@oh-my-pi/pi-coding-agent@15.12.4` package.json `"main":"./src/index.ts"`, `exports["."]={types:./dist/types/index.d.ts, import:./src/index.ts}`, `dist` JS = only `cli.js` + `types/` [https://registry.npmjs.org/@oh-my-pi/pi-coding-agent/15.12.4]. `svkozak/pi-acp` is a thin ACP adapter spawning `pi --mode rpc`; MVP, ~444 stars; not the toolkit [C20-round1, https://github.com/svkozak/pi-acp].

**Toolkit vs harness — Pi is BOTH, layered** [verdict C8, https://github.com/earendil-works/pi/blob/main/packages/agent/src/index.ts]:
- **Low / toolkit:** `agentLoop()`/`runAgentLoop()` driven by `AgentLoopConfig` with a **required bring-your-own `convertToLlm`** plus callback extension points (`transformContext`, `getSteeringMessages`, `prepareNextTurn`, `shouldStopAfterTurn`, `getFollowUpMessages`). This is the rig-but-TS layer.
- **Mid:** `Agent` class — `prompt`/`continue`/`steer`/`followUp`/`subscribe` + queueing.
- **High / harness:** `AgentHarness` — `skill()`, `promptFromTemplate()`, `compact()`, `navigateTree()`, `setModel()`, `setTools()`.

All three are independently exported, so RightClaw can adopt at any layer. (Verdict caveat: the loop's callbacks aren't literally named "hooks"; the term "hook" lives in the harness layer. Functionally they are hooks.)

**Maturity / bus-factor.** Round-3's "single-maintainer / stale" framing is **out of date for vanilla**: HEAD commit by Armin Ronacher (mitsuhiko), org-owned, npm-published at v0.79.3, daily commits, external contributors in the 5600s PR range [verdict C2/C12; https://github.com/earendil-works/pi/blob/main/packages/ai/CHANGELOG.md]. **But the version is `0.79.x` / `0.0.3` root — pre-1.0, fast-moving public API** (UNKNOWN: breaking-change cadence over an adoption horizon). oh-my-pi remains **single-lead (Can Bölük)** — bus-factor concern persists there even though it is now npm-published. **Note on version optics:** oh-my-pi's `15.x` is a **fork-internal version line** with no relation to vanilla's `0.79.x`; do not read `15.12.4` as "more mature than 0.79" — it says nothing about stability relative to vanilla.

---

## 2. THE signaling mechanism — Pi is mechanism C (tool-call-as-control)

This is the headline and it is **decisive and verified**.

- **(A) forced json-schema envelope — NOT used by default.** `toolChoice` exists *per-provider* in pi-ai (`"auto"|"any"|"none"|{type:"tool",name}`) but is **never set anywhere in the harness or coding-agent**; the shared `SimpleStreamOptions`/`StreamOptions` that `AgentLoopConfig` extends has **no `toolChoice` field**, so neither the config spread (`agent-loop.ts:304`) nor the production options object (`agent-harness.ts:385`) can force a tool. The model chooses freely [verdict C6, https://github.com/earendil-works/pi/blob/main/packages/agent/src/harness/agent-harness.ts#L385-L405].
- **(B) native `response_format` — ABSENT.** Zero `response_format`/`responseFormat`/`json_object`/`json_schema`/`structuredOutput` request fields across all 54 files of `packages/ai/src`, including the OpenAI Responses providers; repo-wide code search = 0 hits. The only schema concept is `Tool<TParameters extends TSchema>` (tool-call parameters) [verdict C5, https://github.com/earendil-works/pi/tree/main/packages/ai/src].
- **(C) tool-call-as-control — YES, native.** `AssistantMessage.content` is `(TextContent | ThinkingContent | ToolCall)[]`; `StopReason = "stop"|"length"|"toolUse"|"error"|"aborted"` — reply is plain text, decisions are native tool calls, no structured-output mode anywhere [verdict C4, https://github.com/earendil-works/pi/blob/main/packages/ai/src/types.ts].

The ecosystem's canonical "structured output" is mechanism C: a `defineTool` with `terminate:true` whose typed payload rides the tool-result `details` field [C2-piai, https://github.com/earendil-works/pi/blob/main/packages/coding-agent/examples/extensions/structured-output.ts].

**What this means for weak-model fragility:** Pi **sidesteps the forced-envelope fragility class entirely** (no ignored-forced-tool, no nested-object-stringification from `toolChoice:required`), because it never forces a single-shot envelope. It does **not** improve on a *server-guaranteed* envelope (B) for self-host — it has none. Net: Pi is in the same robustness class as Hermes' approach, *better* than MiMo's emulated-forced-tool path on the specific failure modes the spike measured, and it **validates the user's thesis in production code** — a 35-provider toolkit ships only mechanism C for every model including Anthropic.

**Coerce/repair — present, two-layer, but NOT Hermes-grade — and the attribution must be corrected.** Two layers exist: (1) wire-level `parseStreamingJson`/`repairJson` (control-char + backslash repair, partial-json fallback, never-undefined) [C6-piai, `.../utils/json-parse.ts`]; (2) schema-level `validateToolArguments` = TypeBox `Value.Convert` + recursive `coerceWithJsonSchema` (scalar coercion, anyOf/oneOf/allOf, nested recursion) [verdict C7, `.../utils/validation.ts`]. **Key difference from Hermes:** Pi **coerces-then-validates-then-THROWS** on unsalvageable args; Hermes is repair-never-reject. A throw degrades to a model-visible retry (consistent with RightClaw's FAIL-FAST rule), not silent corruption [verdict C7].

  - **Corrected attribution (overrides round-4 C4).** Round-4 framed oh-my-pi's `repair-args.ts` auto-correcting the whole-args-as-JSON-string case as a **fork-only** capability. **That is mis-attributed.** oh-my-pi's own `repair-args` doc-comment says the whole-args form "is **already auto-corrected by the validator's JSON-string coercion**" — and that validator is pi-ai's **shared** `validation.ts` (`Value.Convert`), present in **vanilla too**. So:
    1. **Whole-args-JSON-string handling is NOT fork-only** — it lives in the shared validator's `Value.Convert` (the TypeBox path).
    2. **`repair-args.ts` is narrower than implied** — it only un-double-escapes *per-field prose* fields and is *deliberately not applied* to code-bearing tools (write/edit/bash) where a backslash/quote is load-bearing. It is a task-tool cosmetic fix, not a general repair layer.
    3. This means C7's "the MiMo stringified-nested-object failure is NOT auto-repaired by vanilla pi" is **conditional on schema kind, not on vanilla-vs-fork**: it holds for the **plain-JSON-Schema** `coerceWithJsonSchema` path, but the **TypeBox** path (`Value.Convert`, what pi's own `defineTool`/`Type.Object` emits) may salvage the whole-args case. **Open question the spike must close:** which schema kind does RightClaw's tool-def path compile to, and does `Value.Convert` salvage RightClaw's nested cron `oneOf` as tool args? Do not bank either behavior until measured.

  If RightClaw wants true repair-never-reject on weak models, that lives in Hermes' philosophy or must be built; vanilla pi's terminal behavior is throw-and-retry.

---

## 3. Hard-requirement map (Pi PROVIDES vs RightClaw BUILDS)

| Req | Vanilla pi | oh-my-pi | RightClaw builds |
|---|---|---|---|
| **SKILLS — SKILL.md format** | ✅ native Agent-Skills loader: `SKILL.md`, frontmatter `name`/`description`/`disable-model-invocation`, lowercase-hyphen validation, recursive load, `<available_skills>` prompt block, programmatic `skill()` inject [verdict C9] | ✅ + native `.claude/skills/*/SKILL.md` discovery at user+project scope [c10/c11] | — |
| **SKILLS — `.claude` dir drop-in** | ⚠️ format-compatible but discovers only `.pi` dirs; reach `.claude` via `--skill <path>`, `piConfig.configDir`, or inline `Skill` objects [c4/c5/c6] | ✅ **directory-level** drop-in: reads `~/.claude` + project `.claude` natively, matches `/sandbox/.claude/skills/rightx-*` [c10] | thin glue (vanilla) or none (fork) |
| **SKILLS — `allowed-tools` enforcement** | ❌ not modeled | ❌ parsed-then-ignored (same as opencode/MiMo); BUT oh-my-pi *does* enforce per-**subagent** `tools:` allowlist [c14/c15] | tool-gating itself — **acceptable: OpenShell is RightClaw's security layer, not skill-level allowed-tools** |
| **SKILLS — learning loop (prefilter→probe-writer→curator)** | ❌ none | ❌ none (`mnemopi` is a *memory* engine, a Hindsight peer, NOT skill authoring) [c17/c18] | **full port** — but grafts cleanly: pipeline writes `rightx-*` SKILL.md files, pi picks them up on `reload()` [c17 reusable] |
| **SUBAGENTS** | ❌ not builtin (builtin tools = read/bash/edit/write/grep/find/ls only); exists only as an **opt-in example extension** spawning a `pi` subprocess + NDJSON structured return [verdict C12; C10-sub] | ✅ builtin `task` tool: child sessions, sync/async, batch `tasks[]`, schema'd return via agent `output` frontmatter; **worktree isolation is opt-in, default `none`** [verdict C17; C11/C12-sub] | implement (vanilla) or adopt (fork). Note: subprocess-spawn pattern maps cleanly onto OpenShell |
| **CONTEXT FORKS** | ✅ **strongest result** — `JsonlSessionRepo.fork({entryId, position})` clones to a new file, parent read-only; parentId-linked **tree** (`moveTo`/`getBranch`); `SessionManager.forkFrom`; `AgentHarness.navigateTree` [verdict C11; C1/C2/C3-sub] | ✅ inherits + extends | — (covers probe-writer fork + background continuation natively) |
| **MCP client** | ❌ **none, by deliberate design** — README "No MCP… build an extension that adds MCP support" [verdict C13 (partial)] | ✅ full hand-rolled Streamable-HTTP client: `Mcp-Session-Id` handshake, `Accept: application/json,text/event-stream`, injectable `headers` (Bearer drops in), `onAuthError` 401/403 refresh hook, OAuth resource-indicator persistence [C5/C6/C7/C8-sub] | **write a pi→MCP tool-adapter** (vanilla) or adopt fork's client (matches RightClaw's `:8100/mcp` + Bearer aggregator as-is) |
| **MULTI-MODEL + self-host** | ✅ any OpenAI-compatible baseUrl (Ollama/vLLM/LM Studio/LiteLLM/Venice), per-provider compat knobs (qwen/deepseek/zai reasoning); 966 builtin model ids / 35 providers [verdict implied; C14, C8/C9-piai] | ✅ inherits | — |
| **PROMPT CACHING** | ✅ Anthropic `cache_control` ephemeral + 1h/24h; OpenAI `prompt_cache_key`/`prompt_cache_retention`; normalized `cacheRetention` [C15; C11/C11b-piai] | ✅ inherits | — |
| **STRUCTURED SIGNALING envelope (req #4)** | ❌ no forced envelope (mechanism C only) | ❌ same (C) | **own the envelope policy either way** — see §5 |
| session resume/id, budget/turn caps, idle compaction, reflection, NDJSON | partial: sessionId settable, JSON/RPC modes give NDJSON; **NO loop-level turn cap or retry bound** (verified — see below) | partial + richer | turn caps / abort-after-3 / reflection / idle-compaction equivalents are RightClaw's to build on `shouldStopAfterTurn` |

**Loop-bound (verified) + recovery path (inferred — flagged).** Vanilla pi's `agentLoop` is `while(true)` with exits only on `stopReason "error"|"aborted"`, `shouldStopAfterTurn`, tool-batch `terminate:true`, or no follow-ups — there is **no max-iteration / max-turns / consecutive-validation-failure counter** (verified by reading `agent-loop.ts`) [https://raw.githubusercontent.com/earendil-works/pi/main/packages/agent/src/agent-loop.ts]. RightClaw's "abort after 3 consecutive structured-output rejections → reflect" has **no analog** and must live in a `shouldStopAfterTurn` callback — a real porting cost, but the hook exists for it. **Inferred, not yet source-confirmed:** that a validation throw is caught, returned as an error tool-result, and fed back for retry (the `executeToolCalls` catch path was not read this round). The loop-bound *absence* is verified; the *throw→retry recovery* is inferred — confirm by reading `executeToolCalls` before relying on it.

---

## 3b. Did round 3 hold up?

| Round-3 claim | Status now | Evidence |
|---|---|---|
| Vanilla pi lacks MCP | **CONFIRMED-still-true** (now *by documented design*, not oversight — won't appear upstream) | verdict C13 (partial: corrects "no dep anywhere" — `@modelcontextprotocol/sdk` is present in lockfiles only as an *optional peer* of `@google/genai`, never a pi dep) |
| Vanilla pi lacks subagents | **CONFIRMED-still-true** as builtin (exists only as opt-in example) | verdict C12 |
| Vanilla pi lacks native structured output | **CONFIRMED-still-true** (mechanism C; no B, no default A) — but round-3's "only forced tool calls" is **imprecise**: pi doesn't even force tools | verdicts C4/C5/C6 |
| SKILL.md drop-in unverified | **CHANGED → VERIFIED PRESENT** (Agent-Skills standard, agentskills.io, in source) | verdict C9; c1/c2/c3/c21 |
| oh-my-pi MCP client "unverified-to-absent" | **CHANGED → VERIFIED PRESENT** (full Streamable-HTTP+OAuth client) | C5-sub |
| Single-maintainer bus-factor (both) | **CHANGED for vanilla** (multi-contributor, org, npm-published); **CONFIRMED for oh-my-pi** (single-lead) | verdict C2/C12; C16 |
| oh-my-pi NOT npm-published (round-4 C20) | **OVERRIDDEN → npm-published, but TS-source runtime entry** (verified this round; supersedes verdict C20 as stale) | npm registry `@oh-my-pi/pi-agent-core@15.12.4` / `@oh-my-pi/pi-coding-agent@15.12.4`; both `main:./src/index.ts` |
| "Pi powers OpenClaw" | **STILL UNVERIFIED → at best one-directional, and possibly a NAMESAKE** — see note below | verdict C21 (partial); C22 |

**OpenClaw note (UNKNOWN, do not assume one OpenClaw).** Pi's README cites `openclaw/openclaw` as "a real-world SDK integration" — i.e. pi *powers* a thing called OpenClaw, one-directionally [verdict C21, README:24]. The fork-side repo references pi only in one refactor doc. **Critically unresolved this round:** whether pi's `openclaw/openclaw` is the **same** OpenClaw RightClaw claims drop-in compatibility with (the OpenClaw/ClawHub *skill ecosystem*), or a different project sharing the name. RightClaw's identity hinges on that ecosystem, so this is marked **UNKNOWN** — not corroboration, and not to be silently collapsed into "one OpenClaw."

---

## 4. Driving surface + deployment in OpenShell

Five options, ranked by fit:

1. **Embed `@earendil-works/pi-agent-core` in-process (TS).** Inject your own `convertToLlm`, supply MCP-proxied tools as `AgentTool[]`, use `beforeToolCall`/`afterToolCall` hooks for policy/observability, `JsonlSessionRepo.fork` for probe-writer/background, `shouldStopAfterTurn` for turn-caps/abort-after-3. **Best fit if RightClaw goes build-your-own-in-TS** — you get forks + caching + multi-model + skills free, and own MCP + subagents + envelope. Runs as a Node/Bun process inside OpenShell; OpenShell owns isolation.
2. **Adopt `@oh-my-pi/pi-coding-agent` (npm-installable, Bun-oriented).** Get MCP + subagents + `.claude` skills + forks out of the box. Cost: single-maintainer fork; **TS-source runtime entry → run under Bun or `bun build --compile` yourself** (the source-build cost is reduced to a dep-pin, not removed); `mnemopi` memory engine that **collides with Hindsight** (turn it off); 15.x churn; you'd set subagent `isolation.mode=none` to avoid double-isolation with OpenShell (UNKNOWN: whether `@oh-my-pi/pi-natives` Rust isoStart/isoStop interferes — needs testing). **Best fit if RightClaw wants a complete TS harness** and accepts the fork's governance and Bun runtime.
3. **`pi --mode rpc` subprocess (vanilla CLI).** Process boundary, NDJSON. Heavier than embedding, fewer guarantees than the SDK. Only if RightClaw wants a hard process boundary.
4. **`pi-acp` adapter.** MVP; strictly dominated for RightClaw's needs (adds an ACP protocol hop for no benefit over direct embedding) — same conclusion ACP got vs `claude -p`.
5. **vs opencode-MiMo (HTTP/SDK) / rig (in-process Rust):** opencode is harness-only; rig is toolkit-only. Pi uniquely offers **both layers** and a **native fork primitive rig lacks** (round-3's "state is a plain array like rig" was wrong — pi has a proper non-mutating session tree). rig keeps its edge only on **Rust-native in-process** (no Node/Bun runtime in the sandbox, no TS toolchain in RightClaw's Rust workspace).

---

## 5. FINAL RE-RANK

For a RightClaw needing **skills + subagents + forks + robust multi-model signaling + MCP**:

**Tier 1 — rig (build-your-own, Rust).** Still #1, but **by a thin and narrowing margin**, and the margin is now almost entirely *operability/language-fit*, not capability:
- Rust-native, in-process, no second runtime in the sandbox, no TS toolchain bolted onto a Rust workspace, compile-time invariants on the invocation contract.
- Cost: you build *everything* (forks, caching normalization, multi-provider, skills loader). Pi narrows this because pi *gives* you forks + caching + 35 providers + a skills loader that rig does not.

**Tier 1 (co-equal) — Pi, embedded `pi-agent-core` (vanilla).** A genuine peer to rig and opencode-MiMo, **not** a duplicate:
- **For:** mechanism C native (validates the structured-output thesis); native session-tree fork (stronger than rig's clone-the-vec); prompt caching + 966-model multi-provider built in; SKILL.md loader free; layered adopt-at-any-level; explicitly defers sandboxing to OpenShell (README: "Pi does not include a built-in permission system… OpenShell: run the whole `pi` process in a policy-controlled sandbox" [C19]) — **architectural alignment with RightClaw is unusually tight**; multi-maintainer + npm-published with compiled type declarations.
- **Against:** TS runtime in the sandbox; you build MCP-adapter + subagents + envelope-policy + abort-after-3 + reflection; pre-1.0 API churn; no Hermes-grade repair (it throws).

**Tier 1 (co-equal) — opencode-MiMo (adopt TS harness).** Complete harness, MCP, subagents, forks (`forkSession`), self-host via MiMo. Cost: ~49k-token default-agent overhead, `allowed-tools` ignored, emulated-forced-tool structured output (the fragile path). Pi's vanilla layer is *cleaner to embed*; opencode is *more complete to adopt*.

**Tier 1 (co-equal) — oh-my-pi (adopt TS harness, fork).** Now that it's npm-published it jumps up — but **less than round-4's framing claimed**: MCP + subagents + `.claude` skills + forks + (task-tool-scoped) per-field repair, all installable. Cost: single-maintainer bus-factor; **TS-source runtime entry (Bun) → self-compile or Bun-host, not a drop-in compiled lib**; `mnemopi`/Hindsight collision; 15.x fork-internal churn; isolation-vs-OpenShell question. **The most "batteries-included" pi**, but you inherit Can Bölük's governance and a Bun runtime requirement.

**Tier 2 — Hermes-pattern.** The cleanest *philosophy* (tool-call-as-control + `[SILENT]` + repair-never-reject), and the **only option with true repair-never-reject** as a stated stance. But it's a Python reference pattern, not a Rust/TS toolkit RightClaw would adopt wholesale — its *ideas* (sentinel, coerce-repair) are what RightClaw should borrow, and Pi already embodies the control half.

**Tier 3 — claude-agent-sdk.** Strong single-vendor structured output, but locks to Anthropic models — fails RightClaw's multi-model + self-host requirement; out of contention for the model-agnostic target.

**Deciding factors, honest:**
- If RightClaw commits to **Rust-native in-process** → **rig** stays #1; Pi's capabilities don't overcome the language/runtime boundary.
- If RightClaw is willing to run a **TS agent-runner in the sandbox** → **Pi (embedded vanilla) is co-equal with opencode-MiMo and arguably the better *toolkit*** (cleaner packages, native forks, tighter OpenShell alignment), while **oh-my-pi is the better *harness*** — now installable, but Bun-oriented and self-compiled, so the operability gap to opencode (which `bun build --compile`s to a single binary) is narrowed, not erased.
- **The structured-output direction is the real fork in the road.** If RightClaw keeps the every-turn `--json-schema` envelope, **no** pi ports cleanly (mechanism C, no forced envelope). If RightClaw moves to **text-reply + tool-call-as-control + MCP side-channels** (the user's stated leaning, which Pi *proves in production*), Pi becomes a near-ideal substrate.

**Don't let "popular with builders" inflate Pi:** the 62.3k stars and "agent-builder toolkit" framing are real, but the **OpenClaw-powers claim is unverified (and possibly a namesake)**, the **API is pre-1.0**, vanilla **has no MCP/subagents/learning-loop/envelope**, and the fork is **single-maintainer** and **Bun-runtime**. Pi earns co-equal status on *verified capability and architectural fit*, not popularity.

---

## 6. If Pi is adopted or borrowed — concrete next step

**The single decisive unmeasured datapoint (do this first):** a **pi spike paralleling the MiMo spike** — embed `@earendil-works/pi-agent-core`, define RightClaw's **cron `oneOf{notify|silent}+run_note`** and **prefilter `classify(skip|patch|create)`** as `defineTool` schemas with `terminate:true` (mechanism C), and run them against the **weak self-host models** (Qwen/GLM/Kimi via pi-ai's OpenAI-compatible baseUrl). Measure: (a) does the model reliably emit the nested cron `oneOf` as tool args; (b) **does pi's `Value.Convert` (TypeBox path) salvage a stringified-nested-object or does it throw** — the schema-kind question §2 left open; (c) prompt-cache hit rate vs RightClaw's current `--system-prompt-file` path. This answers the one question every prior round left open and directly compares to the MiMo numbers.

**If the spike passes**, the minimal-risk borrow (independent of full adoption):
1. **Port pi-ai's two-layer coerce/repair** (`json-parse.ts` repairJson + `validation.ts` `Value.Convert`/coerceWithJsonSchema) under RightClaw's MCP tool-arg path — reusable regardless of harness, stronger than Hermes' two functions on the layers it covers (but it throws rather than never-rejecting).
2. **Adopt the `terminate:true` tool-result `details` pattern** for the reply envelope, replacing the forced `--json-schema` envelope — composes with RightClaw's existing `mcp__right__send_message`/`send_progress` tools.
3. **Wire RightClaw's abort-after-3 + reflection into a `shouldStopAfterTurn` callback** (since pi has no native loop bound) and `JsonlSessionRepo.fork` for probe-writer/background.

**Flagged UNKNOWNs the spike/adopt must still close:**
- **Schema-kind discriminator (§2):** which schema kind RightClaw's tool defs compile to (TypeBox vs plain JSON-Schema) and whether `Value.Convert` salvages the nested-stringification on RightClaw's *actual* tool schemas. This is now an open question, not a settled robustness win.
- Whether oh-my-pi's exported `task`/MCP/fork classes are importable **without a build step** — the `.d.ts` declarations confirm the classes are exported (`TaskTool`, `HttpTransport`, `MCPHttpServerConfig`), but they are exported *from `.ts` source* (`main: ./src/index.ts`), so a non-Bun consumer needs a compile step.
- `used_skill_receipts[]` mapping onto pi tool-calls (per-invocation signal — same gap as opencode).
- oh-my-pi OAuth-cred storage in `agent.db` vs RightClaw's gateway-credential-isolation invariant (potential collision).
- Anthropic prompt-cache **parity** for the tool-call path vs RightClaw's current `--system-prompt-file` caching (must be measured live, not assumed).
- pi pre-1.0 breaking-change cadence over the adoption horizon; and whether pi's `openclaw/openclaw` is RightClaw's OpenClaw or a namesake.

**Primary sources verified this round (not trusting the draft):**
- `https://registry.npmjs.org/@oh-my-pi/pi-coding-agent/15.12.4` — `main:"./src/index.ts"`, `exports["."]={types:"./dist/types/index.d.ts", import:"./src/index.ts"}`, `bin.omp="dist/cli.js"` → **TS-source runtime entry; only CLI + type-decls are compiled JS.**
- `https://registry.npmjs.org/@oh-my-pi/pi-agent-core/15.12.4` — `main:"./src/index.ts"`, `import→"./src/index.ts"`, no JS dist `.` entry.
- `https://registry.npmjs.org/@oh-my-pi/pi-coding-agent` / `/pi-agent-core` — `latest 15.12.4`, **468 / 460** published versions (publish confirmed; overrides stale verdict C20).
- `https://raw.githubusercontent.com/earendil-works/pi/main/packages/agent/src/agent-loop.ts` — `while(true)`, no turn/iteration/retry bound (throw→retry recovery inferred, not yet read in `executeToolCalls`).
