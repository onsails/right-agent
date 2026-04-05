# Phase 2: CLI Runtime and Sandboxing - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-22
**Phase:** 2-CLI Runtime and Sandboxing
**Areas discussed:** Codegen strategy, OpenShell wrapping, Shutdown & cleanup, PC integration

---

## Codegen Strategy

### Generated file location

| Option | Description | Selected |
|--------|-------------|----------|
| /tmp/rightclaw/<hash>/ | Temp dir with hash, cleaned on down | |
| $RIGHTCLAW_HOME/run/ | Under home dir, persistent, inspectable | ✓ |
| XDG runtime dir | $XDG_RUNTIME_DIR/rightclaw/ | |

**User's choice:** $RIGHTCLAW_HOME/run/

### Identity file handling

| Option | Description | Selected |
|--------|-------------|----------|
| Concatenate all | Cat IDENTITY.md + SOUL.md + AGENTS.md into one prompt | |
| Only IDENTITY.md | Pass just IDENTITY.md via --append-system-prompt-file | ✓ |
| You decide | Claude's discretion | |

**User's choice:** Only IDENTITY.md — asked how OpenClaw does it first. Confirmed OpenClaw doesn't concatenate either; CC reads workspace files naturally from cwd.

### Agent working directory

| Option | Description | Selected |
|--------|-------------|----------|
| Agent dir as cwd | Each agent's cwd = its agent folder | ✓ |
| User's home as cwd | Agent cwd = ~ | |
| Let me think | Different idea | |

**User's choice:** Agent dir as cwd

---

## OpenShell Wrapping

### Missing OpenShell behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Fail hard | Refuse to start | |
| Warn and run | Log warning, start without sandbox | |
| Flag to skip | Fail by default, --no-sandbox allows running without | ✓ |

**User's choice:** --no-sandbox flag

### Claude Code permissions

| Option | Description | Selected |
|--------|-------------|----------|
| Always skip | --dangerously-skip-permissions always | ✓ |
| Configurable | Per-agent in agent.yaml | |
| Never skip | Let CC prompts fire | |

**User's choice:** Always skip — OpenShell is the security layer

---

## Shutdown & Cleanup

### Cleanup on down

| Option | Description | Selected |
|--------|-------------|----------|
| Full cleanup | Destroy sandboxes + delete run/ | |
| Keep run/ files | Destroy sandboxes, keep run/ for debugging | ✓ |
| You decide | Claude's discretion | |

**User's choice:** Keep run/ files — overwritten on next up

---

## PC Integration

### API vs CLI

| Option | Description | Selected |
|--------|-------------|----------|
| REST API | reqwest via Unix socket | ✓ |
| Shell out to CLI | process-compose CLI | |
| You decide | Claude's discretion | |

**User's choice:** REST API via Unix socket

### TUI Attach

| Option | Description | Selected |
|--------|-------------|----------|
| Exec into PC | exec process-compose attach | ✓ |
| Passthrough | Spawn as child, forward stdio | |
| You decide | Claude's discretion | |

**User's choice:** exec into process-compose directly

---

## Claude's Discretion

- Exact PC REST API endpoints and error handling
- Sandbox name tracking for cleanup
- Shell wrapper template format
- PC version compatibility

## Deferred Ideas

None
