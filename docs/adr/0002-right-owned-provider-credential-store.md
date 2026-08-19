# Provider credentials live in a Right-owned store, not the sandbox runtime

Status: accepted

Provider credentials move from OpenShell's gateway database into `~/.right/providers.db`
(SQLite, mode 0600, plaintext), and reach the Agent Sandbox as microsandbox *source-ref*
secrets: microsandbox persists only the name of an environment variable and resolves the
real value at spawn from the spawning process, so no credential is ever written to the
sandbox runtime's own storage. Right's store is the single authority; revocation is one
delete.

## Considered options

- **Let microsandbox store the values.** It persists them verbatim in its SQLite catalog.
  That creates a second plaintext store Right does not control, breaks the standing rule
  that credentials never enter backups, and repeats the OpenShell coupling this migration
  exists to escape — provider state living in a vendor's database is exactly what broke
  before. Rejected despite its one advantage: value rotation without a sandbox restart.
- **OS keychain or encrypted file.** Protects against host compromise, which nothing else
  in the platform assumes; the host already holds Claude OAuth tokens in per-agent
  databases. Rejected for platform-specific dependencies and headless-server breakage.

## Consequences

- Rotating a credential may require restarting the Agent Sandbox, because source-ref
  secrets resolve at spawn. Acceptable: rotation is rare and operator-initiated.
- The bot process holds provider credentials in the environment it spawns sandboxes with,
  readable only by the owning user.
- Cross-agent sharing becomes a row reference with an owner column. `shared_from` is
  dropped from `agent.yaml`; configuration stops carrying derived state.
- Migration from OpenShell carries provider metadata only, and each credential is entered
  once more. Values *are* in fact recoverable — pre-2026-08-05 records predate OpenShell's
  credential-storage drivers and hold cleartext inline in the stored payload, while the
  driver-era envelope's KEK sits readable on disk — but importing them would mean parsing a
  vendor's private storage layout, the same class of move that caused an earlier outage. For
  five real credentials, re-entry is cheaper than that coupling.
