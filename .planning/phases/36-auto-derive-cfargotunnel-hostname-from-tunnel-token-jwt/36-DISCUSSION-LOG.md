# Phase 36: auto-derive-cfargotunnel-hostname-from-tunnel-token-jwt - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions captured in CONTEXT.md — this log preserves the discussion.

**Date:** 2026-04-04
**Phase:** 36-auto-derive-cfargotunnel-hostname-from-tunnel-token-jwt
**Mode:** discuss

## Gray Areas Discussed

### --tunnel-hostname removal
| Question | Answer |
|----------|--------|
| What to do with existing config.yaml that has `hostname`? | Ignore silently (serde unknown fields) |

### Decoding approach
| Question | Answer |
|----------|--------|
| How to decode the cloudflared token? | base64 + serde_json (no new deps) |

### Config struct
| Question | Answer |
|----------|--------|
| Where to derive/store hostname? | Derive on the fly via `tunnel.hostname()` method |

### Validation placement
| Question | Answer |
|----------|--------|
| Where to validate/decode token? | Both at `init` (fail fast + print derived hostname) and at `up` (defensive re-validation) |

## User Notes

- "пока что" (for now) — hard remove `--tunnel-hostname`, no deprecation. Custom domain override is explicitly deferred.
