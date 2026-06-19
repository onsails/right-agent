# Spike harness — re-runnable structured-output conformance tester

Preserved from the live spike (was in an ephemeral job tmp dir). Drives `mimo serve`
HTTP `POST /session/{id}/message` with `format:{type:"json_schema",schema:…}` — the
emulated-StructuredOutput-tool path. NOT `mimo run` (no schema flag there).

## Run
1. Scratch dir + `echo '{}' > opencode.json`; `mimo serve --port 4096 --print-logs &`
2. Auth the target provider in mimo (`mimo providers` / dashboard). Creds stay in mimo — scripts hold none.
3. `uv run --with jsonschema python run_mimo_broad.py` (jsonschema via uv ephemeral env).
4. Schemas are read from `../schemas/*.json` (the real RightClaw `--json-schema` payloads).

## Scripts
- `run_mimo_broad.py` — THE original whitelist sweep (baseline 2026-06-13): models × [prefilter, cron] → VALID / INVALID / NO_STRUCTURED / GRAMMAR_ERR / PROVIDER_400. **Edit the `models` list to add candidates.** Hard CRON oneOf is the gating schema. (Reads schemas from a job tmp `{SC}/schemas`.)
- `run_kimi7_sweep.py` — **2026-06-19 re-measure** (new Kimi `kimi-k2-7-code` + new Venice models). Portable: reads schemas relative to itself (`../schemas`), mimo dir from `$SPIKE_SCRATCH` (default `~/.mimo-spike`). Results → `kimi7_sweep_results.json` (committed copy here). Verdict in `../SPIKE-RESULTS.md` "RE-MEASURE": new Kimi fails the same `propertyNames` as k2-6 (Venice per-model server bug, not version); +3 new clean passes (`glm-5-1`, `glm-5-2`, `qwen3-235b-thinking`).
- `run_poll2.py`, `run_venice.py` — per-model conformance with poll-for-`info.finish` + retry-on-empty-turn.
- `probe_propnames.py` — isolates the `propertyNames` failure (sends a MINIMAL flat schema; if it still errors → propertyNames is injected by the stack, not our schema).

## Mechanics learned
- `/message` POST is NOT reliably synchronous → poll `GET /session/{id}/message` until `info.finish` set; structured payload lands in `info.structured`.
- Flaky empty first-call (`in:0`) → retry.
- `propertyNames` grammar error = injected by the stack (Venice-Kimi-endpoint-specific), not in our schemas.
