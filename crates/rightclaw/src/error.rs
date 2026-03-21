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
