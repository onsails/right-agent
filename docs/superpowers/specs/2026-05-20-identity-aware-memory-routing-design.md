# Identity-Aware Memory Routing Design

## Problem

The current PR correctly makes explicit "remember" requests identity-aware, but it also introduced managed `SOUL.md` operating-contract content. That crosses the ownership boundary: Right Agent must not decide or inject the contents of `SOUL.md` for the user or agent.

The requirement is to keep identity-aware routing while making ownership explicit:

- Right Agent may explain what identity files are for.
- Right Agent may route persistence requests to the correct mechanism.
- Right Agent must not write platform-default content into `SOUL.md`.

## Design

`/right-memory` is the single source of truth for detailed persistence routing. It classifies explicit "remember", "save this", and "don't forget" requests by semantic type:

- tool/API/environment rules -> `TOOLS.md`
- stable user facts/preferences -> `USER.md`
- agent voice/style/boundaries -> `SOUL.md`
- core agent identity -> `IDENTITY.md`
- reusable procedures -> learned skills
- residual durable context -> memory

System prompt text and operating instructions should not duplicate this full routing table. They should define identity-file purpose and instruct the agent to invoke `/right-memory` before choosing where to persist explicit remember requests.

## Identity File Ownership

Identity files are always-loaded durable context:

- `IDENTITY.md` contains agent identity and rarely-changing core facts.
- `SOUL.md` contains agent-authored durable voice, values, interaction style, and behavioral boundaries established by bootstrap or user intent.
- `USER.md` contains stable facts and preferences about the user.
- `TOOLS.md` contains durable tool, API, environment, and workflow constraints.

`SOUL.md` remains user/agent-authored. Right Agent must not add a platform-owned or managed block to it, and bootstrap must not invent a default operating contract when the user gave no signal.

## Prompt Changes

`OPERATING_INSTRUCTIONS.md` should:

- describe identity files and their ownership at a high level;
- say that explicit remember/save/don't-forget requests are persistence intent;
- direct the agent to use `/right-memory` to classify the persistence target;
- require smallest accurate edits that preserve existing user/agent-authored content;
- avoid embedding the detailed routing table.

`PROMPT_SYSTEM.md` should mirror the generated prompt contract:

- identity files are always-loaded durable context;
- Right Agent explains file purpose but does not own or prescribe identity-file contents;
- `SOUL.md` changes only from bootstrap/user intent or explicit conversation evidence;
- explicit remember requests must go through `/right-memory` before file edits or memory tool calls.

`BOOTSTRAP.md` should tell the agent to create `SOUL.md` from bootstrap choices. If the user gives no relevant signal, it should keep `SOUL.md` minimal rather than inventing a platform-default operating contract.

## Code Changes

Remove the managed SOUL migration helper and tests from `identity_mirror.rs`:

- `SOUL_OPERATING_CONTRACT_MARKER`
- `SOUL_OPERATING_CONTRACT_BLOCK`
- `with_soul_operating_contract`
- `migrate_host_soul_operating_contract`
- tests that assert platform-authored SOUL default insertion

Keep existing identity mirror behavior: `IDENTITY.md`, `SOUL.md`, and `USER.md` are mirrored between sandbox and host, but their contents remain agent-authored.

## Testing

Update tests to enforce the new boundary:

- codegen tests assert operating instructions direct remember requests to `/right-memory`;
- codegen tests assert prompts do not describe `SOUL.md` as platform-owned or require an operating contract;
- memory skill tests continue to assert the detailed identity-aware routing table;
- aggregator and memory server tests continue to assert `memory_retain` is residual fallback, but avoid making system prompt the detailed router.

Run targeted tests for changed prompt/codegen and MCP instructions, then final verification:

- `devenv shell -- cargo test -p right-codegen`
- `devenv shell -- cargo test -p right memory_retain_schema_marks_memory_as_residual_storage`
- `devenv shell -- cargo test -p right test_get_info_delegates_memory_routing_to_right_memory`
- `devenv shell -- cargo test -p right-agent identity_mirror`
- `devenv shell -- cargo test --workspace`
- `devenv shell -- cargo build --workspace`

## Non-Goals

- Do not remove `SOUL.md`.
- Do not weaken identity-aware routing.
- Do not make memory the default destination for explicit remember requests.
- Do not add automatic platform writes to existing identity files.
