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
- `run_mimo_broad.py` — THE whitelist sweep: models × [prefilter, cron] → VALID / INVALID / NO_STRUCTURED / GRAMMAR_ERR / PROVIDER_400. **Edit the `models` list to add the new Kimi / other candidates.** Hard CRON oneOf is the gating schema.
- `run_poll2.py`, `run_venice.py` — per-model conformance with poll-for-`info.finish` + retry-on-empty-turn.
- `probe_propnames.py` — isolates the `propertyNames` failure (sends a MINIMAL flat schema; if it still errors → propertyNames is injected by the stack, not our schema).

## Mechanics learned
- `/message` POST is NOT reliably synchronous → poll `GET /session/{id}/message` until `info.finish` set; structured payload lands in `info.structured`.
- Flaky empty first-call (`in:0`) → retry.
- `propertyNames` grammar error = injected by the stack (Venice-Kimi-endpoint-specific), not in our schemas.
