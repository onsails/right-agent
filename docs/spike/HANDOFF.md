# HANDOFF — continue the structured-output-capable model whitelist research

*Post-compaction start-here (2026-06-15). Read this first; it is self-contained. Deep detail in the sibling docs (pointers at the end).*

## Premise / strategy under exploration

**Keep structured output** (RightClaw's every-turn `claude -p --json-schema` contract) and **support ONLY the models that reliably produce it — a whitelist.** The user accepts NOT supporting models that can't. This is the **low-effort path**: no signaling rearchitecture (you keep the json-schema envelope), you just restrict the model set + add a retry loop. The spike validated this as viable.

This is **orthogonal to the harness choice.** Harness rank (under the user's hard constraint "won't build MCP or skills myself") is **1) Goose, 2) opencode/MiMo, 3) oh-my-pi**; rig and vanilla-Pi excluded (build-your-own). See `FINAL-RANKING.md`. Any of those harnesses can emit structured output on capable models, so the whitelist work applies regardless.

## Current whitelist (measured)

Conditions: **mimo's emulated forced-tool path** (synthetic `StructuredOutput` tool + `toolChoice:required`) + **Venice** provider, real RightClaw schemas (`docs/spike/schemas/`), **hard CRON `oneOf` as the gating schema**.

**✅ Reliably do structured output (pass the hard CRON oneOf) — 9/13, all strong open families:**
- Clean (easy + hard): `qwen3-235b-a22b-instruct-2507`, `qwen3-next-80b`, `deepseek-v3.2`, `zai-org-glm-5`, `zai-org-glm-4.7`, `mistral-small-3-2-24b-instruct`
- Pass hard, flaked easy (need retry): `qwen3-coder-480b-a35b-instruct-turbo`, `llama-3.3-70b`, `minimax-m3`
- (+ frontier, for reference: OpenAI `gpt-5.4` / `gpt-5.4-mini` — all pass)

**❌ Did NOT pass (4) — none is "model can't":**
- `kimi-k2-6`, `qwen3-5-9b` → `Grammar error: Unimplemented keys: ["propertyNames"]` — injected by the stack (NOT in our schemas), Venice-Kimi-endpoint-specific → **likely a Venice-side tooling bug; Kimi's true ability is UNMEASURED.**
- `deepseek-v4-pro`, `hermes-3-llama-3.1-405b` → Venice `400 Bad Request` (provider param incompatibility, not model).

**The whitelist is `(model × provider × mechanism)`-specific — see the caveat below.**

## THE caveat that gates everything

The whitelist was measured on **mimo-emulated-forced-tool + Venice**. The production path may differ materially:
- **rig's native `response_format json_schema`** (server-side constrained decoding) could pass MORE models AND **dodge `propertyNames`** entirely (our raw schemas contain no `propertyNames`; it's injected by the mimo/Venice stack).
- **Provider matters:** the 4 failures are Venice-specific (grammar engine / param validation). Kimi/DeepSeek-v4 may pass via **Moonshot / Together / Fireworks / a self-host vLLM** instead.

→ **Re-measure the whitelist on the actual chosen prod mechanism+provider before trusting it as config.** The strong families (Qwen / DeepSeek / GLM / Llama / Mistral / MiniMax) almost certainly hold (their pass is model-intrinsic tool-calling strength); the weak/small ones and Kimi need a fresh measurement.

## Open work (continuation TODO, priority order)

1. **NEW KIMI (user flagged a new release).** (a) `mimo models venice | grep -i kimi` for a newer id — currently only `kimi-k2-5` / `kimi-k2-6`; the new one may be `kimi-k2-7` / `k3` / a thinking variant, or only on Moonshot's own API yet. (b) Test it **both via Venice AND via a non-Venice provider** (Moonshot/Together) — this **isolates** whether the old `propertyNames` failure was Venice-tooling (then new-Kimi-via-Venice still fails) vs the model. If it passes anywhere → add to whitelist AND it confirms the failure was tooling, not Kimi.
2. **Re-measure the whitelist on the prod mechanism+provider** (spike was mimo+Venice). Re-run `docs/spike/harness/run_mimo_broad.py` (adapt for rig-native-`response_format` if that's the chosen path).
3. **`propertyNames` fix.** Injected by the stack, NOT our schema, Venice-Kimi-specific, **likely Venice-server-side** (the `venice-ai-sdk-provider` adapter passes the schema as-is; client request is identical for GLM-5 which passes). Confirm the injecting layer: if client-side → strip unsupported JSON-Schema keywords before the grammar compile → unblocks Kimi/qwen-9b (→ ~11/13); if Venice-server → can't fix, route those models via another provider.
4. **Retry loop (required even for whitelist models).** 3/9 passed the HARD schema but flaked the EASY one (EMPTY / ignored-tool / INVALID) → structured output is **not deterministic**; "supports it" ≠ "always emits it." Need retry-on-miss (mimo has `retryCount`; on rig build your own) + consider a **coerce/repair** layer (borrow Pi `validation.ts`/`json-parse.ts`, Goose `toolshim.rs`, Hermes `coerce_tool_args`). Decide policy + cap.
5. **Full schema coverage.** Gate was CRON oneOf (the hardest, nested discriminated union). Confirm `reply` / `bootstrap` / `bg_continuation` / `prefilter` on whitelist candidates too (cron-pass strongly implies the rest, but verify per model).
6. **Provider matrix.** The whitelist is per-`(model, provider)`. Decide which providers RightClaw supports and re-measure the matrix per provider.

## How to re-run (preserved harness — `docs/spike/harness/`)

The spike scripts are committed under `docs/spike/harness/` (they were in an ephemeral job tmp dir). See `harness/README.md`. Quick version:
1. scratch dir + `echo '{}' > opencode.json`; `mimo serve --port 4096 --print-logs &`
2. auth the target provider in mimo (creds stay in mimo; scripts hold none)
3. `uv run --with jsonschema python docs/spike/harness/run_mimo_broad.py` — **edit the `models` list** to add the new Kimi / candidates.
- Drive: `POST /session/{id}/message` with `format:{type:"json_schema",schema:<one of docs/spike/schemas/*.json>}`; **poll** `GET /session/{id}/message` for `info.finish`; structured payload in `info.structured`. NOT `mimo run` (no schema flag).

## Gotchas (don't rediscover)

- `propertyNames` = injected by the stack, Venice-Kimi-specific, NOT our schema, NOT model incapacity.
- `/message` POST is not reliably synchronous → poll for `info.finish`.
- flaky empty first-call (`in:0`) → retry-on-empty.
- ~49k input tokens/turn default "build" agent overhead → use a minimal agent to measure/run cheaply.
- `GET /config` on a running mimo leaks configured MCP creds in plaintext → don't fetch it.
- structured-output reliability is a **MODEL property**; coerce/repair is the robustness layer.
- subscription-OAuth self-attach is **ToS-prohibited** (settled; the ToS-clean Claude-subscription path is spawning the real `claude -p`). Off-topic for the whitelist but don't relitigate.

## Pointers

- `docs/spike/SPIKE-RESULTS.md` — full conformance data + the two MiMo failure modes + the broad-sweep table/tally.
- `docs/spike/FINAL-RANKING.md` — harness rank (Goose #1 under the "won't build MCP/skills" constraint; superseded-banner explains the two ranks).
- `docs/spike/HERMES-CONTROL.md`, `PI-REEVAL.md`, `GOOSE-EVAL.md`, `docs/harness-migration-research.md` — the full arc (rounds 1–12).
- `~/.claude/.../memory/project_harness_migration.md` — the running memory (decision state, all corrections).
- Branch: `worktree-harness-migration-research`.

## One-line restart

*Keep structured output, whitelist the models that do it (9/13 strong open families pass today via mimo+Venice); the next concrete steps are: test the new Kimi (on Venice + a non-Venice provider), re-measure the whitelist on the chosen prod mechanism/provider, and add a retry/coerce-repair loop — because even whitelist models flake.*
