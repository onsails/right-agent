use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum AgentError {
    #[error("Agent '{name}' is missing required file: {file}")]
    #[diagnostic(code(right_agent::agent::missing_file))]
    MissingRequiredFile { name: String, file: String },

    #[error("Failed to parse agent.yaml for '{name}': {reason}")]
    #[diagnostic(code(right_agent::agent::invalid_config))]
    InvalidConfig { name: String, reason: String },

    #[error(
        "Invalid agent directory name '{name}': must contain only alphanumeric characters, hyphens, or underscores"
    )]
    #[diagnostic(code(right_agent::agent::invalid_name))]
    InvalidName { name: String },

    #[error("Failed to read agents directory: {path}")]
    #[diagnostic(code(right_agent::agent::io_error))]
    IoError {
        path: String,
        #[source]
        source: std::io::Error,
    },
}
