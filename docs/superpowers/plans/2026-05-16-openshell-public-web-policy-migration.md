# OpenShell Public Web Policy Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate Right Agent permissive OpenShell policies from rejected `host: "**.*"` rules to OpenShell v0.0.37+ compatible public-web `allowed_ips` rules in generated policies, backup restore, and live test fixtures.

**Architecture:** `right-codegen` owns the canonical policy shape and exposes a small structured YAML migration helper for legacy wildcard endpoints. Normal startup/init paths get the new policy by regeneration; backup restore additionally rewrites any copied legacy policy file before sandbox creation, so restored backups and custom `sandbox.policy_file` values do not reach OpenShell with `**.*`. Live sandbox fixtures use the same valid hostless `allowed_ips` endpoint shape after an empirical OpenShell validation gate.

**Tech Stack:** Rust 2024, OpenShell policy YAML, `serde_saphyr`, `serde_json`, `miette`, `cargo test`, live OpenShell v0.0.37+ sandbox validation.

---

## Context

OpenShell v0.0.37 introduced a breaking gateway/runtime upgrade path and v0.0.42 is the latest release observed during this work. The restore attempt failed because OpenShell rejected the generated permissive policy with:

```text
InvalidArgument: policy contains unsafe content: network policy 'outbound': TLD wildcard '**.*' is not allowed; use subdomain wildcards like '*.example.com' instead
```

Relevant external references:

- OpenShell v0.0.37 release notes require recreating gateway/sandboxes after the breaking runtime change: `https://github.com/NVIDIA/OpenShell/releases/tag/v0.0.37`
- OpenShell releases page showed v0.0.42 as latest on 2026-05-16: `https://github.com/NVIDIA/OpenShell/releases`
- TLD wildcard issue documents why `*.com`-style patterns were rejected: `https://github.com/NVIDIA/OpenShell/issues/787`
- Policy schema documents `allowed_ips` and the `tls` deprecation: `https://docs.nvidia.com/openshell/latest/reference/policy-schema`
- Security best practices document SSRF behavior and `allowed_ips` risks: `https://docs.nvidia.com/openshell/latest/security/best-practices`

Stop condition: if the live validation in Task 1 shows hostless endpoints with `allowed_ips` are no longer accepted by installed OpenShell, stop implementation and ask the user to choose a different permissive-policy contract. Do not implement a broad TLD list fallback.

## File Structure

- Modify `crates/right-codegen/src/policy.rs`
  - Replace permissive `host: "**.*"` generation with hostless public-web `allowed_ips` endpoints for ports 443 and 80.
  - Add a deterministic IPv4 public CIDR generator from a non-public denylist plus IPv6 `2000::/3`.
  - Add `migrate_legacy_permissive_policy_yaml()` to rewrite legacy generated `**.*` endpoints in structured YAML.
  - Add regression tests for new generation and migration.

- Modify `crates/right-codegen/src/pipeline.rs`
  - Update pipeline tests that assert permissive codegen contains `host: "**.*"`.

- Modify `crates/right/src/main.rs`
  - Add `migrate_restored_policy_if_needed(policy_path: &Path)`.
  - Call it after resolving the restore policy path and before `openshell sandbox create`.
  - Add focused unit tests for file migration and no-op behavior.

- Modify `crates/right-openshell/src/test_support.rs`
  - Replace the minimal live sandbox fixture wildcard endpoint with a valid hostless `allowed_ips` endpoint.

- Modify `crates/right/src/right_backend_tests.rs`
  - Replace the local live sandbox fixture wildcard endpoint with a valid hostless `allowed_ips` endpoint.

- Modify `ARCHITECTURE.md`
  - Update live sandbox test helper description and OpenShell policy gotchas.

- Modify `docs/architecture/lifecycle.md`
  - Update normal policy generation and backup restore flow to mention legacy policy migration before sandbox creation.

- Modify `docs/SECURITY.md`
  - Update permissive network policy description to public internet via `allowed_ips`, not global DNS wildcard.

## Verification Commands

Use `devenv shell --` for all project commands.

Targeted commands during implementation:

```bash
devenv shell -- cargo test -p right-codegen policy::tests::permissive_policy_uses_public_allowed_ips --lib
devenv shell -- cargo test -p right-codegen policy::tests::migrates_legacy_permissive_wildcard_policy --lib
devenv shell -- cargo test -p right-codegen pipeline::tests::run_single_agent_codegen_generates_policy --lib
devenv shell -- cargo test -p right restore_migrates_legacy_permissive_policy_file --lib
devenv shell -- cargo test -p right restore_policy_migration_is_noop_without_legacy_wildcard --lib
devenv shell -- cargo test -p right --test cli_integration ci_openshell_policy_validates_against_openshell -- --ignored --exact
```

Final verification after all code changes:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

UAT after final verification:

```bash
devenv shell -- target/debug/right agent init him --from-backup /Users/molt/.right/backups/him/20260516-0115 --preserve-source-bindings --yes
devenv shell -- target/debug/right agent init right --from-backup /Users/molt/.right/backups/right/20260516-0115 --preserve-source-bindings --yes
devenv shell -- target/debug/right up --detach
devenv shell -- target/debug/right agent list
```

If a partial `him` agent directory exists from the failed restore, get explicit user approval before removing it. The cleanup command is destructive:

```bash
devenv shell -- target/debug/right agent destroy him --force
```

## Task 1: Validate Hostless `allowed_ips` With Installed OpenShell

**Files:**
- No repository files changed.

- [ ] **Step 1: Create an empirical policy file in `/tmp`**

Run:

```bash
cat >/tmp/right-hostless-allowed-ips-policy.yaml <<'YAML'
version: 1

filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /var/log
  read_write:
    - /dev/null
    - /tmp
    - /sandbox

landlock:
  compatibility: best_effort

process:
  run_as_user: sandbox
  run_as_group: sandbox

network_policies:
  public_web:
    endpoints:
      - port: 443
        allowed_ips:
          - "1.1.1.1/32"
        protocol: rest
        access: full
    binaries:
      - path: "**"
YAML
```

Expected: `/tmp/right-hostless-allowed-ips-policy.yaml` exists and contains no `host:` field under `network_policies.public_web.endpoints`.

- [ ] **Step 2: Create and delete a probe sandbox**

Run:

```bash
openshell sandbox create right-policy-probe --policy /tmp/right-hostless-allowed-ips-policy.yaml --keep
openshell sandbox delete right-policy-probe
```

Expected: `openshell sandbox create` accepts the policy. If it fails with a schema or validation error for missing `host`, stop and ask the user for approval of a different policy model.

- [ ] **Step 3: Remove the probe policy file**

Run:

```bash
rm /tmp/right-hostless-allowed-ips-policy.yaml
```

Expected: file is gone. This cleanup is for `/tmp` only; do not delete any agent sandbox.

## Task 2: Add Failing Policy Generation Tests

**Files:**
- Modify: `crates/right-codegen/src/policy.rs`

- [ ] **Step 1: Replace the permissive wildcard assertions with a failing public `allowed_ips` test**

In `crates/right-codegen/src/policy.rs`, replace `allows_all_outbound_https_and_http` with:

```rust
    #[test]
    fn permissive_policy_uses_public_allowed_ips() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        let parsed: serde_json::Value =
            serde_saphyr::from_str(&policy).expect("policy must be valid YAML");
        let outbound = &parsed["network_policies"]["outbound"];
        let endpoints = outbound["endpoints"]
            .as_array()
            .expect("outbound endpoints must be a list");

        assert_eq!(endpoints.len(), 2, "permissive policy has HTTP and HTTPS endpoints");
        assert!(
            !policy.contains(r#"host: "**.*""#),
            "OpenShell v0.0.37+ rejects TLD wildcard endpoints"
        );

        for port in [443_u64, 80] {
            let endpoint = endpoints
                .iter()
                .find(|endpoint| endpoint["port"].as_u64() == Some(port))
                .unwrap_or_else(|| panic!("missing permissive endpoint for port {port}"));
            assert!(
                endpoint.get("host").is_none(),
                "public-web endpoint must be hostless so DNS names are not filtered by TLD wildcard"
            );
            let allowed_ips = endpoint["allowed_ips"]
                .as_array()
                .expect("public-web endpoint must use allowed_ips");
            assert!(
                allowed_ips
                    .iter()
                    .any(|cidr| cidr.as_str() == Some("1.0.0.0/8")),
                "public-web IPv4 CIDRs must include normal public ranges"
            );
            assert!(
                allowed_ips
                    .iter()
                    .any(|cidr| cidr.as_str() == Some("2000::/3")),
                "public-web IPv6 CIDRs must include global unicast"
            );
            for forbidden in [
                "0.0.0.0/0",
                "10.0.0.0/8",
                "127.0.0.0/8",
                "169.254.0.0/16",
                "172.16.0.0/12",
                "192.168.0.0/16",
            ] {
                assert!(
                    !allowed_ips.iter().any(|cidr| cidr.as_str() == Some(forbidden)),
                    "public-web endpoint must not allow {forbidden}"
                );
            }
        }
    }
```

- [ ] **Step 2: Update the permissive-specific test**

In `crates/right-codegen/src/policy.rs`, replace `permissive_policy_allows_all_https` with:

```rust
    #[test]
    fn permissive_policy_allows_public_web_without_domain_wildcard() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        assert!(
            !policy.contains(r#"host: "**.*""#),
            "permissive policy must not emit OpenShell-rejected TLD wildcard"
        );
        assert!(
            !policy.contains(r#"host: "*.anthropic.com""#),
            "permissive policy uses public allowed_ips, not restrictive domain entries"
        );
        assert!(policy.contains("allowed_ips:"));
        assert!(policy.contains("port: 443"));
        assert!(policy.contains("port: 80"));
    }
```

- [ ] **Step 3: Update the wildcard test name and assertion text**

In `crates/right-codegen/src/policy.rs`, replace `no_bare_star_host_wildcards` with:

```rust
    /// OpenShell v0.0.37+ rejects TLD-wide wildcard hosts.
    #[test]
    fn no_tld_wide_host_wildcards() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        for line in policy.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("host:") {
                let host_val = trimmed.trim_start_matches("host:").trim().trim_matches('"');
                assert_ne!(host_val, "*", "bare '*' wildcard rejected by OpenShell");
                assert_ne!(
                    host_val, "**.*",
                    "TLD-wide '**.*' wildcard rejected by OpenShell v0.0.37+"
                );
            }
        }
    }
```

- [ ] **Step 4: Run the new generation tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-codegen policy::tests::permissive_policy_uses_public_allowed_ips --lib
```

Expected: FAIL because current generation still emits `host: "**.*"` and no `allowed_ips` on permissive public-web endpoints.

## Task 3: Implement Public-Web Policy Generation

**Files:**
- Modify: `crates/right-codegen/src/policy.rs`

- [ ] **Step 1: Add CIDR generation helpers below `restrictive_endpoints()`**

In `crates/right-codegen/src/policy.rs`, insert this code below `restrictive_endpoints()`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv4Range {
    start: u32,
    end: u32,
}

const NON_PUBLIC_IPV4_CIDRS: &[(&str, u8)] = &[
    ("0.0.0.0", 8),
    ("10.0.0.0", 8),
    ("100.64.0.0", 10),
    ("127.0.0.0", 8),
    ("169.254.0.0", 16),
    ("172.16.0.0", 12),
    ("192.0.0.0", 24),
    ("192.0.2.0", 24),
    ("192.88.99.0", 24),
    ("192.168.0.0", 16),
    ("198.18.0.0", 15),
    ("198.51.100.0", 24),
    ("203.0.113.0", 24),
    ("224.0.0.0", 4),
    ("240.0.0.0", 4),
];

const PUBLIC_IPV6_CIDRS: &[&str] = &["2000::/3"];

fn ipv4_cidr_to_range(base: &str, prefix: u8) -> Ipv4Range {
    let base = u32::from(
        base.parse::<std::net::Ipv4Addr>()
            .expect("static IPv4 CIDR base must parse"),
    );
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let start = base & mask;
    Ipv4Range {
        start,
        end: start | !mask,
    }
}

fn range_to_ipv4_cidrs(start: u32, end: u32) -> Vec<String> {
    let mut cidrs = Vec::new();
    let mut cursor = u64::from(start);
    let end = u64::from(end);

    while cursor <= end {
        let alignment = cursor & cursor.wrapping_neg();
        let mut block_size = if alignment == 0 {
            1_u64 << 32
        } else {
            alignment
        };
        let remaining = end - cursor + 1;
        while block_size > remaining {
            block_size >>= 1;
        }

        let prefix = 32 - block_size.trailing_zeros();
        cidrs.push(format!(
            "{}/{}",
            std::net::Ipv4Addr::from(cursor as u32),
            prefix
        ));
        cursor += block_size;
    }

    cidrs
}

fn public_ipv4_cidrs() -> Vec<String> {
    let mut ranges = NON_PUBLIC_IPV4_CIDRS
        .iter()
        .map(|(base, prefix)| ipv4_cidr_to_range(base, *prefix))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    let mut merged = Vec::<Ipv4Range>::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end.saturating_add(1)
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }

    let mut cidrs = Vec::new();
    let mut cursor = 0_u32;
    for range in merged {
        if cursor < range.start {
            cidrs.extend(range_to_ipv4_cidrs(cursor, range.start - 1));
        }
        if range.end == u32::MAX {
            return cidrs;
        }
        cursor = cursor.max(range.end + 1);
    }
    cidrs.extend(range_to_ipv4_cidrs(cursor, u32::MAX));
    cidrs
}

pub fn public_web_allowed_ip_cidrs() -> Vec<String> {
    public_ipv4_cidrs()
        .into_iter()
        .chain(PUBLIC_IPV6_CIDRS.iter().map(|cidr| (*cidr).to_owned()))
        .collect()
}

fn public_web_allowed_ips_yaml(indent: usize) -> String {
    let pad = " ".repeat(indent);
    public_web_allowed_ip_cidrs()
        .into_iter()
        .map(|cidr| format!("{pad}- \"{cidr}\""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn permissive_endpoints() -> String {
    let allowed_ips = public_web_allowed_ips_yaml(10);
    format!(
        r#"      - port: 443
        allowed_ips:
{allowed_ips}
        protocol: rest
        access: full
      - port: 80
        allowed_ips:
{allowed_ips}
        protocol: rest
        access: full"#
    )
}
```

- [ ] **Step 2: Use `permissive_endpoints()` in `generate_policy()`**

In `crates/right-codegen/src/policy.rs`, replace the `NetworkPolicy::Permissive` arm with:

```rust
        NetworkPolicy::Permissive => {
            format!(
                "  outbound:\n    endpoints:\n{}\n    binaries:\n      - path: \"**\"",
                permissive_endpoints()
            )
        }
```

- [ ] **Step 3: Run targeted generation tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen policy::tests::permissive_policy_uses_public_allowed_ips --lib
devenv shell -- cargo test -p right-codegen policy::tests::permissive_policy_allows_public_web_without_domain_wildcard --lib
devenv shell -- cargo test -p right-codegen policy::tests::no_tld_wide_host_wildcards --lib
```

Expected: PASS.

## Task 4: Add Legacy Policy YAML Migration Helper

**Files:**
- Modify: `crates/right-codegen/src/policy.rs`

- [ ] **Step 1: Add the migration function below `generate_policy()`**

In `crates/right-codegen/src/policy.rs`, insert:

```rust
/// Rewrite legacy generated permissive endpoints that OpenShell v0.0.37+
/// rejects (`host: "**.*"`) into public-web `allowed_ips` endpoints.
///
/// Returns `Ok(Some(yaml))` when it changed the document and `Ok(None)` when
/// the input has no legacy public-web endpoint.
pub fn migrate_legacy_permissive_policy_yaml(yaml: &str) -> miette::Result<Option<String>> {
    let mut doc: serde_json::Value = serde_saphyr::from_str(yaml)
        .map_err(|e| miette::miette!("failed to parse policy.yaml for migration: {e:#}"))?;

    let replacement_ips = serde_json::Value::Array(
        public_web_allowed_ip_cidrs()
            .into_iter()
            .map(serde_json::Value::String)
            .collect(),
    );

    let Some(policies) = doc
        .get_mut("network_policies")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Ok(None);
    };

    let mut changed = false;
    for policy in policies.values_mut() {
        let Some(endpoints) = policy
            .get_mut("endpoints")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };

        for endpoint in endpoints {
            let is_legacy_host = endpoint
                .get("host")
                .and_then(serde_json::Value::as_str)
                == Some("**.*");
            let is_public_web_port = matches!(
                endpoint.get("port").and_then(serde_json::Value::as_u64),
                Some(80 | 443)
            );

            if is_legacy_host && is_public_web_port {
                let endpoint = endpoint.as_object_mut().ok_or_else(|| {
                    miette::miette!("policy.yaml contains a non-mapping endpoint")
                })?;
                endpoint.remove("host");
                endpoint.insert("allowed_ips".to_owned(), replacement_ips.clone());
                changed = true;
            }
        }
    }

    if !changed {
        return Ok(None);
    }

    serde_saphyr::to_string(&doc)
        .map(Some)
        .map_err(|e| miette::miette!("failed to serialize migrated policy.yaml: {e:#}"))
}
```

- [ ] **Step 2: Add migration regression tests**

In `crates/right-codegen/src/policy.rs`, add these tests inside `mod tests`:

```rust
    #[test]
    fn migrates_legacy_permissive_wildcard_policy() {
        let legacy = r#"version: 1
network_policies:
  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
      - host: "**.*"
        port: 80
        protocol: rest
        access: full
    binaries:
      - path: "**"
  right:
    endpoints:
      - host: "host.openshell.internal"
        port: 8100
        allowed_ips:
          - "192.168.65.254/32"
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#;

        let migrated = migrate_legacy_permissive_policy_yaml(legacy)
            .expect("migration must parse")
            .expect("legacy wildcard must be migrated");

        assert!(
            !migrated.contains(r#"host: "**.*""#),
            "legacy TLD wildcard must be removed"
        );

        let parsed: serde_json::Value =
            serde_saphyr::from_str(&migrated).expect("migrated policy must be valid YAML");
        let endpoints = parsed["network_policies"]["outbound"]["endpoints"]
            .as_array()
            .expect("outbound endpoints must remain a list");

        for port in [443_u64, 80] {
            let endpoint = endpoints
                .iter()
                .find(|endpoint| endpoint["port"].as_u64() == Some(port))
                .unwrap_or_else(|| panic!("missing migrated endpoint for port {port}"));
            assert!(endpoint.get("host").is_none());
            assert!(
                endpoint["allowed_ips"]
                    .as_array()
                    .expect("allowed_ips must be a list")
                    .iter()
                    .any(|cidr| cidr.as_str() == Some("1.0.0.0/8"))
            );
        }

        assert_eq!(
            parsed["network_policies"]["right"]["endpoints"][0]["host"].as_str(),
            Some("host.openshell.internal"),
            "non-public-web host endpoint must be preserved"
        );
    }

    #[test]
    fn migration_is_noop_for_current_policy() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive, None);
        let migrated = migrate_legacy_permissive_policy_yaml(&policy)
            .expect("current policy must parse");
        assert!(migrated.is_none(), "current policy must not be rewritten");
    }
```

- [ ] **Step 3: Run migration tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen policy::tests::migrates_legacy_permissive_wildcard_policy --lib
devenv shell -- cargo test -p right-codegen policy::tests::migration_is_noop_for_current_policy --lib
```

Expected: PASS.

## Task 5: Update Pipeline Policy Test

**Files:**
- Modify: `crates/right-codegen/src/pipeline.rs`

- [ ] **Step 1: Replace wildcard assertion in `run_single_agent_codegen_generates_policy`**

In `crates/right-codegen/src/pipeline.rs`, replace:

```rust
        assert!(
            policy.contains(r#"host: "**.*""#),
            "permissive policy must allow all HTTPS"
        );
```

with:

```rust
        assert!(
            !policy.contains(r#"host: "**.*""#),
            "permissive policy must not emit OpenShell-rejected TLD wildcard"
        );
        assert!(
            policy.contains("allowed_ips:"),
            "permissive policy must allow public web through allowed_ips"
        );
        assert!(policy.contains("port: 443"));
        assert!(policy.contains("port: 80"));
```

- [ ] **Step 2: Run the pipeline test**

Run:

```bash
devenv shell -- cargo test -p right-codegen pipeline::tests::run_single_agent_codegen_generates_policy --lib
```

Expected: PASS.

## Task 6: Migrate Copied Restore Policy Before Sandbox Create

**Files:**
- Modify: `crates/right/src/main.rs`

- [ ] **Step 1: Add helper import in tests**

In `crates/right/src/main.rs`, update the `use super::{ ... }` block inside `mod tests` from:

```rust
    use super::{
        ConfigCommands, MemoryCommands, cleanup_failed_restore_agent_dir, resolve_agent_db,
        truncate_content, write_managed_settings,
    };
```

to:

```rust
    use super::{
        ConfigCommands, MemoryCommands, cleanup_failed_restore_agent_dir,
        migrate_restored_policy_if_needed, resolve_agent_db, truncate_content,
        write_managed_settings,
    };
```

- [ ] **Step 2: Add restore migration unit tests**

In `crates/right/src/main.rs`, add these tests after `cleanup_failed_restore_agent_dir_removes_partial_agent_state()`:

```rust
    #[test]
    fn restore_migrates_legacy_permissive_policy_file() {
        let tmp = TempDir::new().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        fs::write(
            &policy_path,
            r#"version: 1
network_policies:
  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
      - host: "**.*"
        port: 80
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#,
        )
        .unwrap();

        migrate_restored_policy_if_needed(&policy_path).unwrap();

        let migrated = fs::read_to_string(&policy_path).unwrap();
        assert!(!migrated.contains(r#"host: "**.*""#));
        assert!(migrated.contains("allowed_ips:"));
        assert!(migrated.contains("1.0.0.0/8"));
    }

    #[test]
    fn restore_policy_migration_is_noop_without_legacy_wildcard() {
        let tmp = TempDir::new().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        let policy = right_codegen::policy::generate_policy(
            8100,
            &right_agent_config::NetworkPolicy::Permissive,
            None,
        );
        fs::write(&policy_path, &policy).unwrap();

        migrate_restored_policy_if_needed(&policy_path).unwrap();

        let after = fs::read_to_string(&policy_path).unwrap();
        assert_eq!(after, policy);
    }
```

- [ ] **Step 3: Run restore tests and verify they fail**

Run:

```bash
devenv shell -- cargo test -p right restore_migrates_legacy_permissive_policy_file --lib
```

Expected: FAIL because `migrate_restored_policy_if_needed` does not exist.

- [ ] **Step 4: Add `migrate_restored_policy_if_needed()` near `cleanup_failed_restore_agent_dir()`**

In `crates/right/src/main.rs`, insert this function immediately before `cleanup_failed_restore_agent_dir()`:

```rust
fn migrate_restored_policy_if_needed(policy_path: &Path) -> miette::Result<()> {
    use miette::IntoDiagnostic;

    let policy = std::fs::read_to_string(policy_path)
        .into_diagnostic()
        .map_err(|e| miette::miette!("failed to read policy file {}: {e:#}", policy_path.display()))?;

    if let Some(migrated) = right_codegen::policy::migrate_legacy_permissive_policy_yaml(&policy)? {
        std::fs::write(policy_path, migrated)
            .into_diagnostic()
            .map_err(|e| {
                miette::miette!(
                    "failed to write migrated policy file {}: {e:#}",
                    policy_path.display()
                )
            })?;
        tracing::warn!(
            policy = %policy_path.display(),
            "migrated legacy OpenShell permissive policy wildcard to public allowed_ips"
        );
    }

    Ok(())
}
```

- [ ] **Step 5: Call the helper in sandboxed restore**

In `crates/right/src/main.rs`, inside `cmd_agent_restore`, after:

```rust
        if !policy_path.exists() {
            return Err(miette::miette!(
                "policy file not found at {} — cannot create sandbox",
                policy_path.display()
            ));
        }
```

insert:

```rust
        migrate_restored_policy_if_needed(&policy_path)?;
```

- [ ] **Step 6: Run restore tests**

Run:

```bash
devenv shell -- cargo test -p right restore_migrates_legacy_permissive_policy_file --lib
devenv shell -- cargo test -p right restore_policy_migration_is_noop_without_legacy_wildcard --lib
```

Expected: PASS.

## Task 7: Update Live Sandbox Test Fixtures

**Files:**
- Modify: `crates/right-openshell/src/test_support.rs`
- Modify: `crates/right/src/right_backend_tests.rs`

- [ ] **Step 1: Update `TestSandbox::create` fixture policy**

In `crates/right-openshell/src/test_support.rs`, replace the fixture comment and endpoint block:

```rust
        // Minimal policy — fast startup, permissive network (wildcard 443).
```

with:

```rust
        // Minimal policy — fast startup, public HTTPS to a probe IP.
```

and replace:

```yaml
      - host: \"**.*\"
        port: 443
        protocol: rest
        access: full
```

with:

```yaml
      - port: 443
        allowed_ips:
          - \"1.1.1.1/32\"
        protocol: rest
        access: full
```

- [ ] **Step 2: Update `right_backend_tests.rs` fixture policy**

In `crates/right/src/right_backend_tests.rs`, replace:

```yaml
      - host: \"**.*\"
        port: 443
        protocol: rest
        access: full
```

with:

```yaml
      - port: 443
        allowed_ips:
          - \"1.1.1.1/32\"
        protocol: rest
        access: full
```

- [ ] **Step 3: Search for remaining active wildcard code**

Run:

```bash
rg -n 'host: \\"?\\*\\*\\.\\*|\\*\\*\\.\\*' crates/right-codegen/src crates/right/src crates/right-openshell/src ARCHITECTURE.md docs/SECURITY.md docs/architecture/lifecycle.md
```

Expected: only legacy migration tests and docs explaining old rejected behavior remain. No generated policy path or live sandbox fixture emits `host: "**.*"`.

## Task 8: Update Architecture and Security Docs

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/lifecycle.md`
- Modify: `docs/SECURITY.md`

- [ ] **Step 1: Update live sandbox helper docs**

In `ARCHITECTURE.md`, replace the live sandbox helper bullet:

```markdown
- Generates a unique `right-test-<name>` sandbox with a minimal permissive policy (wildcard `"**.*"` host on port 443, `binaries: "**"`).
```

with:

```markdown
- Generates a unique `right-test-<name>` sandbox with a minimal public-HTTPS policy (hostless endpoint with `allowed_ips: ["1.1.1.1/32"]`, `binaries: "**"`). OpenShell v0.0.37+ rejects TLD-wide host wildcards such as `"**.*"`.
```

- [ ] **Step 2: Update OpenShell policy gotchas**

In `ARCHITECTURE.md`, replace:

```markdown
- Wildcard domains (`*.anthropic.com`) work — the earlier 403 was caused by the binaries restriction, not wildcard matching.
```

with:

```markdown
- Scoped wildcard domains (`*.anthropic.com`) work. TLD-wide or global wildcard hosts (`*.com`, `"**.*"`) are invalid in OpenShell v0.0.37+ and must not be emitted.
- `network_policy: permissive` is implemented as hostless HTTP/HTTPS endpoints with public `allowed_ips` CIDRs, not as a DNS wildcard. Private, loopback, link-local, carrier-grade NAT, documentation, multicast, and reserved IPv4 ranges are excluded; IPv6 uses global unicast `2000::/3`.
```

- [ ] **Step 3: Update lifecycle doc policy generation and restore flow**

In `docs/architecture/lifecycle.md`, under bot per-agent codegen, replace:

```markdown
  │   ├─ TOOLS.md, skills install, policy.yaml
```

with:

```markdown
  │   ├─ TOOLS.md, skills install, policy.yaml (OpenShell v0.0.37+ public-web policy shape)
```

In the `right agent init <name> --from-backup <path>` flow, replace:

```markdown
  ├─ Create new sandbox with timestamped name
```

with:

```markdown
  ├─ Migrate copied legacy OpenShell permissive policy (`host: "**.*"`) to public `allowed_ips`
  ├─ Create new sandbox with timestamped name
```

- [ ] **Step 4: Update security doc network policy section**

In `docs/SECURITY.md`, replace:

```markdown
- **Domain allowlists** — wildcard patterns (e.g., `*.anthropic.com`, `*.claude.ai`) control which endpoints agents can reach
```

with:

```markdown
- **Endpoint allowlists** — restrictive mode uses scoped domain wildcards (e.g., `*.anthropic.com`, `*.claude.ai`); permissive mode uses hostless public `allowed_ips` endpoints for HTTP/HTTPS because OpenShell v0.0.37+ rejects global DNS wildcards
```

Replace:

```markdown
**Default behavior:** Out of the box with `network_policy: permissive`, agents can reach any HTTPS endpoint. All traffic still goes through OpenShell's proxy with TLS termination for inspection — but no domain restrictions apply.
```

with:

```markdown
**Default behavior:** Out of the box with `network_policy: permissive`, agents can reach public HTTP/HTTPS endpoints. The generated policy uses public `allowed_ips` CIDRs instead of a global DNS wildcard, so private, loopback, link-local, carrier-grade NAT, documentation, multicast, and reserved IPv4 ranges are not part of the permissive public-web rule.
```

- [ ] **Step 5: Run docs search**

Run:

```bash
rg -n '\\*\\*\\.\\*|TLD-wide|public `allowed_ips`|public-web' ARCHITECTURE.md docs/SECURITY.md docs/architecture/lifecycle.md
```

Expected: references to `**.*` only describe the rejected legacy behavior or restore migration.

## Task 9: Validate Generated Policy Against Live OpenShell

**Files:**
- No source changes unless this test exposes a real defect.

- [ ] **Step 1: Run all right-codegen tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen --lib
```

Expected: PASS.

- [ ] **Step 2: Run restore unit tests**

Run:

```bash
devenv shell -- cargo test -p right restore_migrates_legacy_permissive_policy_file --lib
devenv shell -- cargo test -p right restore_policy_migration_is_noop_without_legacy_wildcard --lib
```

Expected: PASS.

- [ ] **Step 3: Run the ignored live policy validation test**

Run:

```bash
devenv shell -- cargo test -p right --test cli_integration ci_openshell_policy_validates_against_openshell -- --ignored --exact
```

Expected: PASS against installed OpenShell. If it fails with policy validation output, stop and inspect the exact OpenShell error before changing the policy model.

## Task 10: Final Verification and Commit

**Files:**
- All modified files from prior tasks.

- [ ] **Step 1: Format**

Run:

```bash
devenv shell -- cargo fmt --all
```

Expected: command exits 0.

- [ ] **Step 2: Full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If there are pre-existing unrelated failures, capture exact failing test names and output before deciding whether to continue.

- [ ] **Step 3: Full workspace build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 4: Review diff for scope**

Run:

```bash
git diff -- crates/right-codegen/src/policy.rs crates/right-codegen/src/pipeline.rs crates/right/src/main.rs crates/right-openshell/src/test_support.rs crates/right/src/right_backend_tests.rs ARCHITECTURE.md docs/architecture/lifecycle.md docs/SECURITY.md docs/superpowers/plans/2026-05-16-openshell-public-web-policy-migration.md
```

Expected: diff only covers public-web policy generation, legacy restore migration, active test fixture updates, and docs.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/right-codegen/src/policy.rs crates/right-codegen/src/pipeline.rs crates/right/src/main.rs crates/right-openshell/src/test_support.rs crates/right/src/right_backend_tests.rs ARCHITECTURE.md docs/architecture/lifecycle.md docs/SECURITY.md docs/superpowers/plans/2026-05-16-openshell-public-web-policy-migration.md
git commit -m "fix(openshell): migrate permissive public web policy"
```

Expected: commit succeeds.

## Task 11: Restore UAT for `him` and `right`

**Files:**
- Runtime state under `~/.right`, not repository files.

- [ ] **Step 1: Confirm backup directories exist**

Run:

```bash
test -f /Users/molt/.right/backups/him/20260516-0115/sandbox.tar.gz
test -f /Users/molt/.right/backups/him/20260516-0115/agent.yaml
test -f /Users/molt/.right/backups/right/20260516-0115/sandbox.tar.gz
test -f /Users/molt/.right/backups/right/20260516-0115/agent.yaml
```

Expected: all commands exit 0.

- [ ] **Step 2: Check for partial agents before restore**

Run:

```bash
devenv shell -- target/debug/right agent list
```

Expected: `him` and `right` are absent. If either exists from the failed restore, ask the user before running `right agent destroy <name> --force`.

- [ ] **Step 3: Restore `him`**

Run:

```bash
devenv shell -- target/debug/right agent init him --from-backup /Users/molt/.right/backups/him/20260516-0115 --preserve-source-bindings --yes
```

Expected: restore creates a new OpenShell sandbox, uploads `sandbox.tar.gz`, writes `sandbox.name` to `agent.yaml`, and does not fail on `host: "**.*"`.

- [ ] **Step 4: Restore `right`**

Run:

```bash
devenv shell -- target/debug/right agent init right --from-backup /Users/molt/.right/backups/right/20260516-0115 --preserve-source-bindings --yes
```

Expected: restore creates a new OpenShell sandbox, uploads `sandbox.tar.gz`, writes `sandbox.name` to `agent.yaml`, and does not fail on `host: "**.*"`.

- [ ] **Step 5: Start agents**

Run:

```bash
devenv shell -- target/debug/right up --detach
```

Expected: process-compose starts `him-bot`, `right-bot`, MCP server, and cloudflared without policy validation errors.

- [ ] **Step 6: Inspect runtime state**

Run:

```bash
devenv shell -- target/debug/right agent list
openshell sandbox list
```

Expected: `him` and `right` agents exist, their sandboxes exist, and no new sandbox is stuck in failed creation.

- [ ] **Step 7: Verify policy files no longer contain rejected wildcard**

Run:

```bash
rg -n '\*\*\.\*' /Users/molt/.right/agents/him/policy.yaml /Users/molt/.right/agents/right/policy.yaml
```

Expected: no matches.

## Self-Review

- Spec coverage: normal policy generation is covered by Tasks 2, 3, 5, and 9; backup recovery migration is covered by Tasks 4, 6, and 11; live sandbox fixture drift is covered by Task 7; docs are covered by Task 8.
- Placeholder scan: the plan contains no deferred implementation blanks. Conditional stop points are explicit safety gates for invalid OpenShell policy behavior and partial destructive runtime cleanup.
- Type consistency: `public_web_allowed_ip_cidrs()`, `migrate_legacy_permissive_policy_yaml()`, and `migrate_restored_policy_if_needed()` are defined before use in later tasks; tests reference the same names and exact file paths.
