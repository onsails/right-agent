use miette::Diagnostic;
use thiserror::Error;

/// Render an error together with its full `source()` chain as a single
/// colon-separated string.
///
/// `thiserror`-derived errors and raw `reqwest::Error` types do not walk the
/// source chain in their `Display` output, so `{:#}` alone often hides the
/// underlying cause (e.g. the real IO error behind
/// "error sending request for url"). Use this helper whenever logging errors
/// that cross library boundaries and could carry meaningful chained context.
pub fn display_error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        use std::fmt::Write as _;
        let _ = write!(out, ": {cause}");
        source = cause.source();
    }
    out
}

#[derive(Debug, Error, Diagnostic)]
pub enum AgentError {
    #[error("Agent '{name}' is missing required file: {file}")]
    #[diagnostic(code(rightclaw::agent::missing_file))]
    MissingRequiredFile { name: String, file: String },

    #[error("Failed to parse agent.yaml for '{name}': {reason}")]
    #[diagnostic(code(rightclaw::agent::invalid_config))]
    InvalidConfig { name: String, reason: String },

    #[error("Invalid agent directory name '{name}': must contain only alphanumeric characters, hyphens, or underscores")]
    #[diagnostic(code(rightclaw::agent::invalid_name))]
    InvalidName { name: String },

    #[error("Failed to read agents directory: {path}")]
    #[diagnostic(code(rightclaw::agent::io_error))]
    IoError {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_required_file_displays_agent_name_and_file() {
        let err = AgentError::MissingRequiredFile {
            name: "my-agent".to_string(),
            file: "IDENTITY.md".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("my-agent"), "expected agent name in: {msg}");
        assert!(
            msg.contains("IDENTITY.md"),
            "expected file name in: {msg}"
        );
    }

    #[test]
    fn invalid_name_displays_the_name() {
        let err = AgentError::InvalidName {
            name: "bad agent!".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("bad agent!"),
            "expected invalid name in: {msg}"
        );
    }

    #[test]
    fn display_error_chain_includes_sources() {
        use std::error::Error;
        use std::fmt;

        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("root cause")
            }
        }
        impl Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("top-level failure")
            }
        }
        impl Error for Outer {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&self.0)
            }
        }

        let err = Outer(Inner);
        assert_eq!(
            display_error_chain(&err),
            "top-level failure: root cause"
        );
    }

    #[test]
    fn display_error_chain_handles_no_source() {
        let err = AgentError::InvalidName { name: "x".into() };
        let rendered = display_error_chain(&err);
        assert_eq!(rendered, err.to_string(), "no source: unchanged");
    }
}
