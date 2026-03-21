use std::path::PathBuf;

/// Resolve RIGHTCLAW_HOME: cli_home > env_home > ~/.rightclaw
pub fn resolve_home(cli_home: Option<&str>, env_home: Option<&str>) -> miette::Result<PathBuf> {
    if let Some(home) = cli_home {
        return Ok(PathBuf::from(home));
    }
    if let Some(home) = env_home {
        return Ok(PathBuf::from(home));
    }
    let home =
        dirs::home_dir().ok_or_else(|| miette::miette!("Could not determine home directory"))?;
    Ok(home.join(".rightclaw"))
}
