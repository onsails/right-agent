# Changelog
## [0.2.4] - 2026-04-27


### Miscellaneous

- Update Cargo.lock dependencies

## [0.2.3] - 2026-04-24


### Bug Fixes

- **bot**: Render agent-error stderr as HTML <pre> in Telegram
- **bot**: Check filesystem policy drift before hot-reload apply
- **doctor**: Remove AGENTS.md existence check
- **clippy**: Duplicated_attributes and never_loop
- **clippy**: Clone_on_copy on SandboxMode/NetworkPolicy
- **clippy**: Derivable_impls on SttConfig and AuthMethod
- **clippy**: Collapsible_if across cron_spec, init, proxy, attachments, handler
- **clippy**: Assorted mechanical lints
- Address review-loop findings (2 iterations)
- **aggregator**: Disable rmcp 1.4+ DNS-rebinding Host check
- **policy**: Drop deprecated tls: terminate from generated policies
- **clippy**: More mechanical fixes across rightclaw-cli
- **clippy**: Site-level allows for judgment-call lints

### Features

- **bot**: Warn on filesystem policy drift at startup
- **codegen**: Scaffold contract module with CodegenKind types
- **codegen/contract**: Add write_regenerated helper
- **codegen/contract**: Add write_agent_owned helper
- **codegen/contract**: Add write_merged_rmw helper
- **codegen/contract**: Add write_and_apply_sandbox_policy
- **codegen/contract**: Add per-agent and cross-agent registries
- **codegen/contract**: Add write_regenerated_bytes for binary skill content

### Refactor

- **bot**: Route policy apply through write_and_apply_sandbox_policy
- **codegen/pipeline**: Route static-content writes through write_regenerated
- **codegen/pipeline**: Route settings.local.json through write_agent_owned
- **codegen/pipeline**: Route agent secret injection through write_merged_rmw
- **codegen/pipeline**: Route policy.yaml seed through write_regenerated
- **codegen/pipeline**: Route cross-agent writes through write_regenerated
- **codegen/claude_json**: Route .claude.json through write_merged_rmw
- **codegen/mcp_config**: Route mcp.json writes through contract helpers
- **codegen/skills**: Route skill writes through write_regenerated
- **codegen/skills**: Use write_agent_owned for installed.json
- **codegen/contract**: Extract ensure_parent_dir, wire write_and_apply_sandbox_policy

### Testing

- **codegen/contract**: Assert Regenerated outputs are idempotent
- **codegen/contract**: Assert AgentOwned files not overwritten
- **codegen/contract**: Assert MergedRMW preserves unknown fields
- **codegen/contract**: Assert registry covers all per-agent writes
- **policy**: Integration test for live-sandbox policy apply
