# Share TestSandbox Across Crates; Fix `sandbox_upgrade` Tests

## Problem

`crates/bot/tests/sandbox_upgrade.rs` (added in `cdf26b7`) hardcodes a sandbox
name `rightclaw-rightclaw-test-lifecycle` that was a one-off developer fixture.
The sandbox is not created by any test setup in the repo, so all four tests
fail with gRPC `NotFound, message: "sandbox not found"` on machines where the
fixture is absent.

The repo already has a proper ephemeral-sandbox helper — `TestSandbox` in
`crates/rightclaw/src/openshell_tests.rs` — but it is `pub(crate)` and the
module is included under `#[cfg(test)]`, so it is reachable only from
`rightclaw`'s own unit tests, not from `rightclaw-bot`'s integration tests.

## Goals

1. Make `TestSandbox` reusable from any crate's tests without duplicating it.
2. Replace the four hardcoded-name tests in `sandbox_upgrade.rs` with a single
   self-sufficient integration test that creates its own ephemeral sandbox.
3. Document the convention so future sandbox-touching tests use the helper.

## Non-goals

- Changing any behavior in `crates/bot/src/upgrade.rs` (the scheduler under
  test). Its unit tests already pass.
- Any `#[ignore]` — user feedback (memory `feedback_no_ignore_tests`) explicitly
  forbids ignoring integration tests.
- Parallelizing sandbox tests further. Parallel caps (`7095f14`) are untouched.

## Design

### Module extraction

Move `TestSandbox` (struct, `create`, `exec`, `name`, `Drop`) out of
`crates/rightclaw/src/openshell_tests.rs` into a new file
`crates/rightclaw/src/test_support.rs`.

The module is gated on either the standard test cfg or an opt-in feature:

```rust
// crates/rightclaw/src/lib.rs
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
```

With `cfg(test)` OR `feature = "test-support"`:
- `rightclaw`'s own `cargo test` picks it up automatically (no feature needed).
- External consumers opt in by enabling the feature (see below).

`openshell_tests.rs` switches to `use crate::test_support::TestSandbox;` and
retains only the mock server and actual test bodies.

### Feature flag

Add to `crates/rightclaw/Cargo.toml`:

```toml
[features]
test-support = []
```

The feature is empty — it only toggles compilation of the `test_support` module.

### Cross-crate wiring

Add to `crates/bot/Cargo.toml`:

```toml
[dev-dependencies]
rightclaw = { path = "../rightclaw", features = ["test-support"] }
```

The regular `[dependencies]` entry stays `{ path = "../rightclaw" }`. Cargo
unifies dev-dep features with the normal dep, so `test-support` is present only
during `cargo test` builds of `rightclaw-bot`.

### Test rewrite

Delete the four existing tests in `crates/bot/tests/sandbox_upgrade.rs` and
replace them with one combined test:

```rust
use rightclaw::test_support::TestSandbox;

#[tokio::test]
async fn claude_upgrade_lifecycle() {
    let sbox = TestSandbox::create("claude-upgrade").await;

    // 1. Run `claude upgrade`, assert success + expected stdout.
    let (stdout, exit) = sbox.exec(&["claude", "upgrade"]).await;
    assert_eq!(exit, 0, "claude upgrade failed; stdout: {stdout}");
    assert!(
        stdout.contains("Successfully updated") || stdout.contains("Current version"),
        "unexpected upgrade output: {stdout}"
    );

    // 2. Symlink exists in .local/bin.
    let (_, exit) = sbox.exec(&["test", "-L", "/sandbox/.local/bin/claude"]).await;
    assert_eq!(exit, 0, "/sandbox/.local/bin/claude symlink missing");

    // 3. Upgraded binary reports a valid version.
    let (stdout, exit) = sbox.exec(&["/sandbox/.local/bin/claude", "--version"]).await;
    assert_eq!(exit, 0, "upgraded binary failed to run");
    assert!(
        stdout.contains("Claude Code"),
        "expected 'Claude Code' in version output, got: {stdout}"
    );

    // 4. PATH precedence resolves to the upgraded binary.
    let (stdout, exit) = sbox
        .exec(&["bash", "-c", "PATH=/sandbox/.local/bin:$PATH which claude"])
        .await;
    assert_eq!(exit, 0, "`which claude` failed");
    assert_eq!(stdout.trim(), "/sandbox/.local/bin/claude");
}
```

No `sandbox_exec` helper, no CLI invocations — all exec goes through
`TestSandbox::exec`, which uses gRPC, satisfying the
`NEVER use CLI for exec` rule in CLAUDE.md.

### Panic-safety unchanged

`TestSandbox::create` already:
- calls `pkill_test_orphans(&name)` before creating,
- calls `register_test_sandbox(&name)` so the panic hook cleans up under
  `panic = "abort"`,
- and `Drop` calls `unregister_test_sandbox` + `delete_sandbox_sync`.

Moving the file does not change any of this.

### Network policy coverage

`TestSandbox::create` builds a permissive policy:

```yaml
network_policies:
  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: "**"
```

`**.*` matches `storage.googleapis.com`, which is the CDN `claude upgrade`
downloads from. `binaries: "**"` permits the `claude` binary at
`/usr/local/bin/claude` to initiate the outbound request. No policy change
needed.

### ARCHITECTURE.md update

Add a new subsection **Integration Tests Using Live Sandboxes**, placed between
the existing "SQLite Rules" and "Security Model" sections:

> Any test that needs a live OpenShell sandbox MUST create it via
> `rightclaw::test_support::TestSandbox::create("<test-name>")`. The helper:
>
> - Generates a unique `rightclaw-test-<name>` sandbox with a minimal
>   permissive policy.
> - Registers the sandbox in `test_cleanup` so sandboxes are deleted even
>   under `panic = "abort"`.
> - Cleans up leftovers from prior SIGKILLed runs via `pkill_test_orphans`.
> - Exposes `.exec(&[...])` which uses gRPC (the project bans the
>   `openshell sandbox exec` CLI).
>
> Consumers outside `rightclaw`'s own test binary depend on the `test-support`
> feature:
>
> ```toml
> [dev-dependencies]
> rightclaw = { path = "...", features = ["test-support"] }
> ```
>
> Never hardcode sandbox names (no `rightclaw-foo-test-lifecycle` fixtures),
> never invoke the `openshell` CLI from tests, and never add `#[ignore]` to
> sandbox tests.

## Alternatives Considered

- **Dedicated `rightclaw-test-support` crate.** Cleaner separation but more
  churn: new crate, new workspace member, two `rightclaw` crates import it.
  Not justified for one helper struct.
- **Re-export via `pub(crate)` → `pub use`.** Cannot work: the module is
  under `#[cfg(test)]`, so it does not exist in `rightclaw`'s library
  artifact when another crate consumes it.
- **Inline the helper in `sandbox_upgrade.rs`.** Duplicates ~50 LoC of
  subtle lifecycle logic (policy, panic-hook registry, gRPC wait-for-ready).
  First duplication sets the pattern for future tests — avoid.

## Test Plan

1. `cargo test -p rightclaw --lib` — existing `TestSandbox`-using tests still
   pass.
2. `cargo test -p rightclaw-bot --test sandbox_upgrade` — the new combined
   test creates an ephemeral sandbox, runs the upgrade flow, and cleans up.
3. `cargo test --workspace` — overall suite stays green.

## Risks

- `claude upgrade` hits `storage.googleapis.com`; transient network failures
  will fail the test. Acceptable — we want signal on outbound network policy
  regressions. No retry layer needed.
- Per-run sandbox creation adds ~1-2 min to the test. Acceptable trade-off;
  one combined test (not four) keeps it bounded.
