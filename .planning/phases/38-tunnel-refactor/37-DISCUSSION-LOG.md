# Phase 37: fix-v3-2-uat-gaps - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions captured in CONTEXT.md — this log preserves the analysis.

**Date:** 2026-04-04
**Phase:** 37-fix-v3-2-uat-gaps-tunnel-setup-flow-tunnel-hostname-dns-rout
**Mode:** discuss
**Areas analyzed:** Tunnel hostname storage, DNS routing wrapper, Doctor checks, Bot healthcheck, MCP tracing, Warning visibility, Status labels

## Gray Areas Presented

### Tunnel hostname
| Question | Options | User chose |
|----------|---------|------------|
| `--tunnel-hostname` required vs optional | Required / Optional with UUID fallback | Required |

### DNS routing
| Question | Options | User chose |
|----------|---------|------------|
| Where/how to run `cloudflared tunnel route dns` | Wrapper script fatal / rightclaw up preflight / Wrapper script warn-only | Wrapper script, fatal |

### MCP status label
| Question | Options | User chose |
|----------|---------|------------|
| `AuthState::Missing` display text | "no token" / "auth required" / "unauthenticated" | "auth required" |

## Corrections Applied

### Tunnel hostname (reverting Phase 36 D-01..D-06)
- **Phase 36 decision:** Hard remove `--tunnel-hostname`, derive UUID from JWT
- **Phase 37 correction:** Add `--tunnel-hostname` back as required arg, store in config.yaml, use stored hostname everywhere
- **Reason:** UAT test 10 failed — UUID-based cfargotunnel.com healthcheck fails without proper DNS routing

## Context Inherited from Prior Phases

- Phase 36: `TunnelConfig::hostname()` decode logic (reused as `tunnel_uuid()`)
- Phase 34/35: dptree dispatch pattern, bot handler structure
- Phase 33: doctor check pattern (DoctorCheck struct)
