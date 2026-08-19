//! Declarative sandbox specification.
//!
//! [`SandboxSpec`] is everything create needs: name, image, resources,
//! egress, secret bindings, TLS bypass extras, and guest defaults. It
//! validates before any SDK call and compiles into an SDK builder in
//! [`SandboxSpec::to_builder`].

use microsandbox::Sandbox;
use microsandbox::sandbox::SandboxBuilder;

use crate::egress::Egress;
use crate::error::SandboxError;
use crate::resources::Resources;
use crate::secrets::{SecretBinding, tls_bypass_list};

/// Everything needed to create an Agent Sandbox.
///
/// `SandboxSpec::new` starts from Right's defaults (permissive egress,
/// [`Resources::default`], no secrets); callers override fields directly.
/// Validation is explicit: [`SandboxHandle::create_or_attach`](crate::SandboxHandle::create_or_attach)
/// validates, and `validate` is public so config-load paths can fail early.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// Sandbox name. Must pass the SDK's name validation; use
    /// [`crate::sandbox_name`]/[`crate::fit_sandbox_name`] to derive one from
    /// an agent name.
    pub name: String,

    /// OCI image reference (e.g. `"node:22-slim"`).
    pub image: String,

    /// vCPU / memory / writable-layer sizing.
    pub resources: Resources,

    /// Egress policy. Create-time only — the SDK cannot change network policy
    /// on a running sandbox.
    pub egress: Egress,

    /// Provider credential bindings (source references only).
    pub secrets: Vec<SecretBinding>,

    /// Extra TLS-bypass hosts on top of [`crate::TLS_BYPASS_HOSTS`]. Only
    /// consulted when at least one secret is bound (no secrets means
    /// interception is off entirely).
    pub tls_bypass_extra: Vec<String>,

    /// Default guest working directory.
    pub workdir: Option<String>,

    /// Guest shell used by `shell` execs (default: the image's `/bin/sh`).
    pub shell: Option<String>,

    /// Extra guest environment variables. The `MSB_` prefix is reserved by
    /// the SDK and rejected.
    pub env: Vec<(String, String)>,

    /// Sandbox-wide guest user (`"1000"`, `"sandbox"`, `"1000:1000"`).
    /// Absent means root; Right's provisioning sets this to the unprivileged
    /// `sandbox` user.
    pub user: Option<String>,
}

impl SandboxSpec {
    /// A spec with Right's defaults for the given name and image.
    pub fn new(name: impl Into<String>, image: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            image: image.into(),
            resources: Resources::default(),
            egress: Egress::default(),
            secrets: Vec::new(),
            tls_bypass_extra: Vec::new(),
            workdir: None,
            shell: None,
            env: Vec::new(),
            user: None,
        }
    }

    /// Validate every field before any SDK call.
    pub fn validate(&self) -> Result<(), SandboxError> {
        if self.image.trim().is_empty() {
            return Err(SandboxError::InvalidSpec {
                field: "image",
                reason: "must be a non-empty OCI image reference".to_owned(),
            });
        }
        microsandbox::sandbox::validate_sandbox_name(&self.name).map_err(|err| {
            SandboxError::InvalidSpec {
                field: "name",
                reason: format!("{err}"),
            }
        })?;
        self.resources.validate()?;
        self.egress.validate()?;
        let mut seen_env_vars: Vec<&str> = Vec::with_capacity(self.secrets.len());
        for binding in &self.secrets {
            binding.validate()?;
            if seen_env_vars.contains(&binding.env_var.as_str()) {
                return Err(SandboxError::InvalidSpec {
                    field: "secrets",
                    reason: format!("duplicate guest env var {:?}", binding.env_var),
                });
            }
            seen_env_vars.push(&binding.env_var);
        }
        for (key, _) in &self.env {
            if key.is_empty() || key.contains(['=', '\0']) {
                return Err(SandboxError::InvalidSpec {
                    field: "env",
                    reason: "keys must be non-empty and contain no '=' or NUL".to_owned(),
                });
            }
            if key.starts_with("MSB_") {
                return Err(SandboxError::InvalidSpec {
                    field: "env",
                    reason: format!("key {key:?} uses the SDK-reserved MSB_ prefix"),
                });
            }
        }
        Ok(())
    }

    /// Compile into an SDK builder.
    ///
    /// Ordering matters: `.network()` is applied before `.secret_entry(..)`
    /// because the latter also enables TLS interception as a side effect; the
    /// explicit `tls(|t| t.enabled(!secrets.is_empty()))` keeps interception
    /// deterministic regardless of SDK defaults.
    pub(crate) fn to_builder(&self) -> Result<SandboxBuilder, SandboxError> {
        self.validate()?;

        let policy = self.egress.to_policy()?;
        let interception = !self.secrets.is_empty();
        let bypass = tls_bypass_list(&self.tls_bypass_extra);

        let mut builder = Sandbox::builder(&self.name)
            .image(self.image.as_str())
            .cpus(self.resources.cpus)
            .memory(self.resources.memory_mib)
            .root_disk(self.resources.writable_layer_mib)
            .detached(true)
            .network(|network| {
                network.policy(policy).tls(|tls| {
                    let mut tls = tls.enabled(interception);
                    for host in &bypass {
                        tls = tls.bypass(host);
                    }
                    tls
                })
            });
        if let Some(workdir) = &self.workdir {
            builder = builder.workdir(workdir);
        }
        if let Some(shell) = &self.shell {
            builder = builder.shell(shell);
        }
        if let Some(user) = &self.user {
            builder = builder.user(user);
        }
        for (key, value) in &self.env {
            builder = builder.env(key, value);
        }
        for binding in &self.secrets {
            builder = builder.secret_entry(binding.sdk_builder().build());
        }
        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::sandbox_name;

    #[test]
    fn default_spec_validates() {
        let spec = SandboxSpec::new(sandbox_name("hal"), "node:22-slim");
        spec.validate().expect("default spec is valid");
    }

    #[test]
    fn invalid_names_are_rejected() {
        for name in ["", "-lead", "has space", &"x".repeat(129)] {
            let spec = SandboxSpec::new(name, "node:22-slim");
            let err = spec.validate().expect_err("invalid name must fail");
            assert!(
                matches!(err, SandboxError::InvalidSpec { field: "name", .. }),
                "{name:?}: {err:?}"
            );
        }
    }

    #[test]
    fn empty_image_is_rejected() {
        let spec = SandboxSpec::new("right-a", "  ");
        assert!(spec.validate().is_err());
    }

    #[test]
    fn msb_prefixed_env_is_rejected() {
        let mut spec = SandboxSpec::new("right-a", "node:22-slim");
        spec.env.push(("MSB_HOME".to_owned(), "/tmp/x".to_owned()));
        let err = spec.validate().expect_err("MSB_ env must fail");
        assert!(matches!(
            err,
            SandboxError::InvalidSpec { field: "env", .. }
        ));
    }

    #[test]
    fn duplicate_secret_env_vars_are_rejected() {
        let mut spec = SandboxSpec::new("right-a", "node:22-slim");
        for source in ["HOST_KEY_A", "HOST_KEY_B"] {
            let mut binding = SecretBinding::new("KEY", source);
            binding.allowed_hosts = vec!["api.example.com".to_owned()];
            spec.secrets.push(binding);
        }
        let err = spec
            .validate()
            .expect_err("duplicate secret env vars must fail");
        assert!(matches!(
            err,
            SandboxError::InvalidSpec {
                field: "secrets",
                ..
            }
        ));
    }

    #[test]
    fn spec_with_secrets_and_restrictive_egress_validates() {
        let mut spec = SandboxSpec::new(sandbox_name("hal"), "node:22-slim");
        spec.egress = Egress::Restrictive {
            allow: vec!["example.com".to_owned()],
        };
        let mut binding = SecretBinding::new("KEY", "HOST_KEY");
        binding.allowed_hosts = vec!["api.example.com".to_owned()];
        spec.secrets = vec![binding];
        spec.user = Some("sandbox".to_owned());
        spec.validate().expect("full spec is valid");
    }
}
