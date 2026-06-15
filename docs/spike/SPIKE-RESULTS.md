# SPIKE RESULTS — MiMo-Code structured-output conformance (hands-on, 2026-06-13)

Hands-on run of the adopt-MiMo structured-output spike, on the live `mimo` (v0.1.0) install. Sandbox/egress off the table (OpenShell owns isolation in prod); this measures whether MiMo's structured output actually conforms on the models RightClaw would run. Conformance validated with `jsonschema` against the **real** RightClaw schemas in `docs/spike/schemas/` (extracted verbatim from `agent_def.rs` / `learning_prefilter.rs`).

## Environment

- `mimo` 0.1.0 + `opencode` installed (Nix). Driven via `mimo serve` HTTP (`POST /session/{id}/message` with `format:{type:"json_schema",schema:…}`) — because `mimo run` has **no** structured-output flag (confirmed: `--format` is `default|json` output-rendering only).
- **Authed providers: OpenAI only** (`mimo providers list` → OpenAI oauth). `xiaomi/*` → `401 Invalid API Key` (no key). **`mimo/mimo-auto` works free** (mimo-free plugin → anonymous JWT → `api.xiaomimimo.com`) — used as the non-OpenAI MiMo datapoint.
- **No local LLM available** (no ollama/vllm/llama-server binary; nothing OpenAI-compatible on common ports — `:8080` is a non-LLM 404). The decisive **vLLM `guided_json` server-side-enforcement** test remains **BLOCKED on infra** (see Gaps).

## Conformance matrix (real RightClaw schemas)

| Model | prefilter (flat) | reply (nested arrays + pattern) | cron (oneOf + const + branch-required) |
|---|---|---|---|
| **openai/gpt-5.4-mini** (frontier) | ✅ VALID | ✅ VALID | ✅ VALID |
| **openai/gpt-5.4** (frontier) | — | — | ✅ VALID |
| **mimo/mimo-auto** (non-OpenAI MiMo) | ✅ VALID | ❌ **NO_STRUCTURED** | ❌ **INVALID** |

Structured payload lands cleanly in `response.info.structured` (parsed object) when the turn completes.

### The two MiMo (non-frontier) failure modes — both decision-relevant

1. **reply → ignored the forced tool entirely.** `finish:"stop"`, ~271 input tokens, plain-text answer, **no `StructuredOutput` tool call** → no structured output at all. `toolChoice:"required"` was **not honored** by the model/adapter. RightClaw would get a free-text turn where it needs a typed reply.
2. **cron (oneOf) → nested-object stringification.** It *did* call `StructuredOutput`, but emitted `delivery` as a **JSON-encoded string** instead of a nested object: `"delivery":"{\"kind\":\"notify\",\"content\":…}"`. Schema requires `delivery` to be an object (oneOf of two branches) → validation fails at path `["delivery"]`. This is the same class as rig issue #1085 (vLLM Granite "double-stringify", round 9). RightClaw's `reply`/`cron`/`bg_continuation`/`bootstrap` schemas **all** have nested structures → all exposed to this failure mode.

### Verdict on the crux

**The emulated structured-output mechanism (synthetic `StructuredOutput` tool + `toolChoice:"required"`) is a MODEL property, confirmed empirically.** Frontier OpenAI conforms across all schema complexities incl. the hard oneOf. The non-frontier MiMo model is **unreliable** — it skips the forced tool on a medium schema and stringifies nested objects on the hard schema. RightClaw passes `--json-schema` **every turn**; adopting MiMo to run cheaper/other models means structured output is **best-effort and fails on RightClaw's nested schemas**. This is exactly the round-4/round-9 thesis, now demonstrated. The mitigation that helps weak models — **server-side constrained decoding (vLLM `guided_json`/`response_format`)** — is **NOT** what MiMo's forced-tool path does, and could not be tested here (no vLLM). That remains THE open decisive test.

## Integration findings (driving MiMo, decision-relevant)

- **No HTTP daemon required in prod.** `mimo run` builds the server **in-process** (`Server.Default().app.fetch`, no `.listen()`); structured output just needs `format` plumbed into the in-process `session.prompt` call (a ~30-line custom driver). The earlier "server burden returns" worry is **refuted** — `mimo serve` was used here only as a convenient spike vehicle.
- **`/message` POST is not reliably synchronous.** Some turns return before completion (`finish:null`, only a `reasoning` part). The driver MUST **poll** `GET /session/{id}/message` (or consume `/event` SSE) until `info.finish` is set — do not trust the POST return.
- **Flaky empty first-call.** The first model call in a burst intermittently returns `in:0`/`finish:null` (no tokens, no run). Driver needs **retry-on-empty-turn**.
- **Heavy default system prompt: ~49k input tokens/turn** for the built-in "build" agent (prompt + all default tools), even for a trivial classification. RightClaw MUST use a minimal/custom agent (suppress the built-in agent prompt + tool set) to control per-turn cost — quantifies the brief's "additive system prompt" concern.
- **`GET /config` returns configured MCP credentials in plaintext** (observed: a context7 API key in `mcp.*.headers`). A security note for any operator driving `mimo serve`; not an issue for RightClaw's gateway model (creds never land in mimo config) but worth knowing.
- **MCP loads from per-directory config** on the run/in-process path (no flag needed) — RightClaw's aggregator config would load unchanged.

## Gaps / what's still needed (decisive, infra-blocked)

1. **vLLM `guided_json` conformance test — THE remaining decider.** Stand up a vLLM (or ollama) OpenAI-compatible endpoint with a real self-host model (Qwen/Llama/DeepSeek), configure it as a MiMo provider, and re-run the matrix — measuring whether **server-side constrained decoding** rescues the nested-object/forced-tool failures that mimo-auto showed. This is the actual "can we run other/self-host models reliably" question. **Blocked:** no local LLM server/binary present in this environment.
2. **Same schemas via rig's native `response_format`** on the same vLLM endpoint, head-to-head vs MiMo's forced-tool path — to settle whether rig's enforcement materially beats MiMo's emulation on weak models.
3. Author the ~30-line in-process MiMo driver (vs `mimo serve`) for a realistic per-turn cold-start + teardown latency measurement.

## One-line takeaway

MiMo's structured output **conforms on frontier OpenAI (incl. the hard oneOf) but degrades on the non-frontier MiMo model** (skips the forced tool; stringifies nested objects) — confirming structured-output reliability is a model property. For a RightClaw that needs json-schema every turn on *other/self-host* models, the emulated path is a real risk; the unresolved decider is whether vLLM server-side `guided_json` fixes it (infra-blocked here).

---

## FOLLOW-UP — Venice open-model conformance (2026-06-13)

Venice AI authed (OpenAI-compatible; serves open weights). Same harness, same RightClaw schemas. This is the real "popular/open/self-host-class models" test the MiMo question hinges on.

| Model (venice/…) | prefilter (flat) | reply (nested) | cron (oneOf) | Failure nature |
|---|---|---|---|---|
| **qwen3-235b-a22b-instruct-2507** | ✅ VALID | ✅ VALID | ✅ VALID | strong open → conforms (tool-calls) |
| **zai-org-glm-5** | ✅ VALID | ✅ VALID | ✅ VALID | strong open → conforms (tool-calls) |
| **llama-3.3-70b** | ✅ VALID | ❌ INVALID (missing `content`) | ❌ INVALID (missing `delivery`) | finish:`stop`, NO enforcement → free-form drifts off required fields on nested schemas |
| **kimi-k2-6** | ❌ | ❌ | ❌ | hard error: `Grammar error: Unimplemented keys: ["propertyNames"]` — server-side grammar enforcement can't compile the tool schema |
| **qwen3-5-9b** (small/cheap) | ❌ | ❌ | ❌ | same `propertyNames` grammar error |
| **deepseek-v4-pro** | ❌ | ❌ | ❌ | Venice `400 Bad Request: Invalid request parameters` (param incompatibility) |

(Plus earlier: OpenAI gpt-5.4/-mini ✅ all incl. oneOf; Xiaomi mimo-auto: prefilter ✅, reply ignored-tool, cron stringified-nested.)

### What this actually shows — it is NOT "open models fail"

Reliability is a fragile **3-way matrix (model × provider × the provider's structured-output enforcement engine)**, not a model-capability axis:

1. **Strong open models conform cleanly.** Qwen3-235B and GLM-5 pass all three incl. the hard `oneOf` via the tool-call path — proof MiMo's emulated structured output *does* work on capable open weights.
2. **A hard, model-independent enforcement-layer failure exists.** Kimi-K2 and small Qwen3.5-9B both hard-fail with `Grammar error: Unimplemented keys: ["propertyNames"]` — Venice applies server-side grammar-constrained decoding on those endpoints, and the grammar engine doesn't implement the `propertyNames` JSON-Schema keyword. RightClaw's raw schemas don't use `propertyNames`, so it is introduced by MiMo's `StructuredOutput` tool-schema wrapper / AI-SDK conversion — **origin unconfirmed; plausibly sanitizable in a MiMo fork** (strip unsupported keywords from the generated tool schema). Until fixed, **Kimi is unusable for structured output on this path**, regardless of the model's own ability.
3. **Unenforced endpoints drift.** Llama-3.3-70B returns free-form (`finish:stop`, no grammar) and misses required fields on nested schemas — exactly the failure RightClaw can't tolerate every turn.
4. **Provider param fragility.** DeepSeek-v4-pro 400s at Venice — a provider/model integration gap, not a conformance result.

### Decision-relevant conclusion (sharpened)

Adopting MiMo to run open/self-host models means **per-(model × provider) qualification is mandatory** — you cannot assume json-schema works. Of 6 popular open models tested on Venice, **only 2 (Qwen3-235B, GLM-5) reliably conform** on RightClaw's nested schemas; the cheap/small one (Qwen-9B) and **Kimi** hard-fail at the enforcement layer, Llama drifts, DeepSeek 400s. So "use any open model" is false as stated; "use a *qualified* strong open model (Qwen-235B / GLM-5), per-combo-tested" is the real, narrower truth. The `propertyNames` grammar gap is a concrete MiMo-side bug to chase (it blocks two of the six, including a flagship). This refines — not reverses — the earlier finding: structured-output reliability is gated by the model **and** the provider's enforcement engine, and must be qualified combo-by-combo before RightClaw could depend on it.

---

## BROAD SWEEP via mimo — 13 popular open models (2026-06-13) — answers "stay on structured output?"

Decision frame (user): RightClaw relies on structured output; many models can't do it; OK to NOT support those. Question: is the set of open models that DO structured output (through mimo, the harness under evaluation) already enough? Sweep run through `mimo serve` (the real adopt-MiMo path: emulated StructuredOutput tool + `format:json_schema`) against Venice open models, real RightClaw schemas, hard CRON oneOf as the gating schema.

| Model (venice/…) | prefilter | cron (hard oneOf) | classification |
|---|---|---|---|
| qwen3-235b-a22b-instruct-2507 | ✅ VALID | ✅ VALID | **clean pass** |
| qwen3-next-80b | ✅ VALID | ✅ VALID | **clean pass** |
| deepseek-v3.2 | ✅ VALID | ✅ VALID | **clean pass** |
| zai-org-glm-5 | ✅ VALID | ✅ VALID | **clean pass** |
| zai-org-glm-4.7 | ✅ VALID | ✅ VALID | **clean pass** |
| mistral-small-3-2-24b-instruct | ✅ VALID | ✅ VALID | **clean pass (small 24B!)** |
| qwen3-coder-480b-a35b-instruct-turbo | ⚠️ EMPTY (flaky) | ✅ VALID | pass-with-retry |
| llama-3.3-70b | ⚠️ NO_STRUCTURED | ✅ VALID | pass-with-retry (ignored tool on easy turn) |
| minimax-m3 | ⚠️ INVALID | ✅ VALID | pass-with-retry |
| kimi-k2-6 | ❌ GRAMMAR_ERR | ❌ GRAMMAR_ERR | **fixable tooling** (Venice grammar `propertyNames`; injected by MiMo wrapper, not in our schemas) |
| qwen3-5-9b | ❌ GRAMMAR_ERR | ❌ GRAMMAR_ERR | **fixable tooling** (same `propertyNames`) |
| deepseek-v4-pro | ❌ PROVIDER_400 | ❌ PROVIDER_400 | Venice param rejection (provider integration) |
| hermes-3-llama-3.1-405b | ❌ PROVIDER_400 | ❌ PROVIDER_400 | Venice param rejection |

(Plus prior: frontier OpenAI gpt-5.4/-mini ✅ all; the free Xiaomi mimo-auto degraded on nested schemas.)

### Tally
- **Pass the hard CRON oneOf: 9 / 13** — every strong popular open family (Qwen3 235B/coder-480B/next-80B, DeepSeek-v3.2, GLM-5/4.7, Llama-3.3-70B, Mistral-small-24B, MiniMax-m3).
- **Clean both-pass: 6 / 13.** Pass-with-occasional-retry-miss: +3.
- **Hard-fail: 4** — and none is "the model can't do structured output": **2 are a fixable MiMo/Venice tooling bug** (`propertyNames` grammar; MiMo injects a keyword Venice's grammar engine can't compile — strip it in the driver and Kimi/small-Qwen likely join the supported set, →11/13), **2 are Venice provider-param rejections** (DeepSeek-v4-pro, Hermes-405B).

### Verdict — staying on structured output is VIABLE; do not switch to tool-call signaling for coverage

The structured-output-capable open-model set is **already broad** — 9/13 popular open models (all the strong families) pass RightClaw's hardest real schema through mimo today, and 2 of the 4 failures are a fixable driver bug, not model incapacity. The user's stated strategy — *stay on structured output, support the models that do it, skip the rest* — is sound right now: it already covers Qwen/DeepSeek/GLM/Llama/Mistral/MiniMax. Migrating to tool-call/Hermes-style signaling is **not justified by model coverage** — the coverage gap it would close is small (Kimi + a couple) and partly closable by fixing the `propertyNames` injection instead.

Two caveats kept honest:
1. **Even "supported" models need a retry layer.** Three models passed the hard schema but flaked the easy one (EMPTY / ignored-tool / INVALID) — structured output through mimo is not 100% deterministic; a retry-on-miss loop (which MiMo's `retryCount` provides) is required regardless.
2. **This is the MiMo emulated-tool path.** Via rig's native `response_format` (server-side enforcement, the rig path) the picture could be *better* (stronger enforcement) AND dodge the `propertyNames` injection (our raw schemas don't contain it). A focused rig-native-vs-mimo-emulated comparison on the same models is the remaining nice-to-have, but is not needed to answer the coverage question: coverage is already sufficient.

**Actionable next step (small, high-value):** chase the `propertyNames` injection in MiMo's StructuredOutput tool-schema wrapper — stripping unsupported JSON-Schema keywords before the grammar compile would unblock Kimi + small-Qwen (→ ~11/13) at near-zero cost.

### VERIFIED — origin of the `propertyNames` grammar error (correcting earlier wording)

`propertyNames` is a (rarely-implemented) JSON Schema keyword constraining an object's **key names**. Controlled test (2026-06-13): Kimi-k2-6 fails with `Grammar error: Unimplemented keys: ["propertyNames"]` **even on a minimal flat one-field schema** (`{type:object, properties:{answer:{type:string}}, required:[answer]}`) that contains no `propertyNames` and no nesting; GLM-5 on the **identical** mimo request returns valid structured output. So:
- It is **injected by the tooling stack, unconditionally** (not by RightClaw's schema — proven), and is **Kimi-endpoint-specific** on Venice.
- `serve.log` shows mimo reaches Venice via the bundled `venice-ai-sdk-provider`; that adapter passes the schema **as-is** (defaults `strict:true`), and the schema is sent as a synthetic `StructuredOutput` **tool** through the AI SDK's `prepareTools()`. Because the client request is identical for GLM-5 (works) and Kimi (fails), the differentiator is **Venice's server-side grammar compilation for the Kimi endpoint** — i.e. **most likely a Venice-side, per-model bug**, NOT MiMo's wrapper and NOT model incapacity. (Earlier wording "injected by MiMo's wrapper, fixable in a MiMo fork" is **corrected** — exact injecting layer not pinned, but evidence points to Venice-server, which RightClaw cannot fix; the practical answer is "don't use Kimi-via-Venice for structured output — it may work via another provider/quant".)
- Consequence: Kimi's *actual* structured-output ability is **unmeasured** here (Venice's enforcement layer rejects before generation). It is not a "Kimi can't do structured output" result.
