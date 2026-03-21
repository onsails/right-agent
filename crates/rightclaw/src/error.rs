use miette::Diagnostic;
use thiserror::Error;

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
}
