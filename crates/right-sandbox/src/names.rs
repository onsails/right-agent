//! Sandbox naming.
//!
//! The SDK accepts names of 1..=128 bytes over `[A-Za-z0-9._-]` with an
//! alphanumeric first byte (`microsandbox::sandbox::validate_sandbox_name`).
//! [`fit_sandbox_name`] maps an arbitrary agent name into that space,
//! deterministically, and is total: every output passes the SDK validator.

use sha2::{Digest, Sha256};

/// Maximum sandbox-name length in bytes, re-exported from the pinned SDK so
/// the cap can never drift from the validator that enforces it.
pub const MAX_SANDBOX_NAME_BYTES: usize = microsandbox::MAX_SANDBOX_NAME_BYTES;

/// Hex characters of the SHA-256 of the raw name appended on truncation.
const FIT_HASH_HEX_CHARS: usize = 8;

/// Byte budget for the human-readable prefix when truncation is required:
/// prefix + `-` + hash must fit [`MAX_SANDBOX_NAME_BYTES`].
const FIT_PREFIX_MAX_BYTES: usize = MAX_SANDBOX_NAME_BYTES - 1 - FIT_HASH_HEX_CHARS;

/// Generate the deterministic sandbox name for an agent.
///
/// Returns `right-{agent_name}`, fitted via [`fit_sandbox_name`] when the
/// agent name is not already a valid SDK sandbox name.
pub fn sandbox_name(agent_name: &str) -> String {
    fit_sandbox_name(&format!("right-{agent_name}"))
}

/// Fit `raw` into the SDK's sandbox-name space.
///
/// Returns `raw` unchanged when it already validates. Otherwise invalid
/// characters collapse to a single `-` run, leading non-alphanumerics are
/// dropped, and an over-long result becomes `{prefix}-{hash8}` where `hash8`
/// is the first 8 lowercase hex chars of the SHA-256 of the full `raw` string
/// — the hash keeps names with long common prefixes distinct and is
/// deterministic across processes. When nothing valid survives sanitizing,
/// the name is `sandbox-{hash8}`.
///
/// Invariant: the output always passes
/// `microsandbox::sandbox::validate_sandbox_name`.
pub fn fit_sandbox_name(raw: &str) -> String {
    if microsandbox::sandbox::validate_sandbox_name(raw).is_ok() {
        return raw.to_owned();
    }
    let sanitized = sanitize_name(raw);
    if sanitized.is_empty() {
        return format!("sandbox-{}", name_hash(raw));
    }
    if sanitized.len() <= MAX_SANDBOX_NAME_BYTES {
        return sanitized;
    }
    // Truncate on a char boundary at or under the prefix budget, then strip
    // trailing separators so the hash join never produces a ragged name. The
    // sanitizer's leading-alnum rule guarantees the prefix stays non-empty.
    let boundary = sanitized
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|&index| index <= FIT_PREFIX_MAX_BYTES)
        .last()
        .unwrap_or(0);
    let prefix = sanitized[..boundary].trim_end_matches(['-', '.', '_']);
    format!("{prefix}-{}", name_hash(raw))
}

/// Map every character outside `[A-Za-z0-9._-]` into a single `-` run and
/// drop everything before the first alphanumeric (the SDK requires an
/// alphanumeric first byte). Interior separators and case are preserved.
fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if out.is_empty() {
            // Leading separators and invalid chars: the name must start
            // alphanumeric.
        } else if ch == '.' || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            // '-' and every invalid char collapse to one dash.
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// First [`FIT_HASH_HEX_CHARS`] lowercase hex chars of the SHA-256 of `raw`.
fn name_hash(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    let mut out = String::with_capacity(FIT_HASH_HEX_CHARS);
    for byte in &digest[..FIT_HASH_HEX_CHARS / 2] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_valid(name: &str) {
        microsandbox::sandbox::validate_sandbox_name(name)
            .unwrap_or_else(|err| panic!("fitted name {name:?} must validate: {err}"));
        assert!(
            name.len() <= MAX_SANDBOX_NAME_BYTES,
            "fitted name {name:?} exceeds {MAX_SANDBOX_NAME_BYTES} bytes"
        );
    }

    #[test]
    fn valid_names_pass_through_unchanged() {
        for name in ["right-a", "a", "right-Agent_1.2"] {
            assert_eq!(fit_sandbox_name(name), name);
        }
        let max_len = "x".repeat(MAX_SANDBOX_NAME_BYTES);
        assert_eq!(fit_sandbox_name(&max_len), max_len);
    }

    #[test]
    fn invalid_characters_collapse_to_single_dash_runs() {
        assert_eq!(fit_sandbox_name("my agent!!"), "my-agent");
        assert_eq!(fit_sandbox_name("a  --  b"), "a-b");
        assert_eq!(fit_sandbox_name("spaces\tand\nnewlines"), "spaces-and-newlines");
    }

    #[test]
    fn leading_non_alphanumerics_are_dropped() {
        assert_eq!(fit_sandbox_name("-oops"), "oops");
        assert_eq!(fit_sandbox_name("!!lead"), "lead");
        assert_eq!(fit_sandbox_name(".dot"), "dot");
        assert_eq!(fit_sandbox_name("_under"), "under");
    }

    #[test]
    fn all_invalid_input_falls_back_to_a_hashed_name() {
        let fitted = fit_sandbox_name("!!!");
        assert!(fitted.starts_with("sandbox-"), "got {fitted:?}");
        assert_valid(&fitted);
    }

    #[test]
    fn empty_input_falls_back_to_a_hashed_name() {
        let fitted = fit_sandbox_name("");
        assert!(fitted.starts_with("sandbox-"), "got {fitted:?}");
        assert_valid(&fitted);
    }

    #[test]
    fn long_names_truncate_with_a_stable_hash() {
        let raw = format!("right-{}", "a".repeat(200));
        let fitted = fit_sandbox_name(&raw);
        assert_eq!(fitted.len(), MAX_SANDBOX_NAME_BYTES);
        assert!(fitted.ends_with(&format!("-{}", name_hash(&raw))));
        assert_valid(&fitted);
        // Deterministic across calls.
        assert_eq!(fit_sandbox_name(&raw), fitted);
    }

    #[test]
    fn truncation_lands_on_a_char_boundary() {
        // A multibyte char straddling the prefix budget must snap back to the
        // boundary before it, so the multibyte char is dropped whole.
        let mut raw = "x".repeat(FIT_PREFIX_MAX_BYTES - 1);
        raw.push('é');
        raw.push_str(&"y".repeat(50));
        let fitted = fit_sandbox_name(&raw);
        assert_valid(&fitted);
        assert!(!fitted.contains('é'));
        assert!(fitted.starts_with(&"x".repeat(FIT_PREFIX_MAX_BYTES - 1)));
    }

    #[test]
    fn long_names_with_common_prefixes_stay_distinct() {
        let prefix = "shared-prefix-".repeat(8);
        let a = fit_sandbox_name(&format!("{prefix}-agent-one"));
        let b = fit_sandbox_name(&format!("{prefix}-agent-two"));
        assert_ne!(a, b);
        assert_valid(&a);
        assert_valid(&b);
    }

    #[test]
    fn fitting_is_idempotent() {
        for raw in ["right-a", "my agent!!", &"z".repeat(300), "!!!", ""] {
            let once = fit_sandbox_name(raw);
            assert_eq!(fit_sandbox_name(&once), once, "fit must be idempotent");
        }
    }

    #[test]
    fn sandbox_name_prefixes_right() {
        assert_eq!(sandbox_name("hal"), "right-hal");
        assert_valid(&sandbox_name("An Agent With Spaces"));
    }
}
