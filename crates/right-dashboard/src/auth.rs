use std::{collections::BTreeSet, fmt};

use chrono::{DateTime, Utc};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardUser {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: String,
}

#[derive(Clone, Eq, PartialEq)]
pub struct InitDataValidation {
    pub bot_token: String,
    pub now: DateTime<Utc>,
    pub max_age_secs: i64,
}

impl fmt::Debug for InitDataValidation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InitDataValidation")
            .field("bot_token", &"<redacted>")
            .field("now", &self.now)
            .field("max_age_secs", &self.max_age_secs)
            .finish()
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AuthError {
    #[error("missing init data")]
    MissingInitData,
    #[error("malformed init data")]
    MalformedInitData,
    #[error("invalid init data hash")]
    InvalidHash,
    #[error("init data expired")]
    Expired,
    #[error("missing init data user")]
    MissingUser,
    #[error("unauthorized dashboard user")]
    UnauthorizedUser,
}

pub fn validate_init_data(
    raw: &str,
    _cfg: &InitDataValidation,
) -> Result<DashboardUser, AuthError> {
    if raw.trim().is_empty() {
        return Err(AuthError::MissingInitData);
    }

    Err(AuthError::MalformedInitData)
}

pub fn authorize_user(
    user: DashboardUser,
    trusted_user_ids: &BTreeSet<i64>,
) -> Result<DashboardUser, AuthError> {
    if trusted_user_ids.contains(&user.id) {
        Ok(user)
    } else {
        Err(AuthError::UnauthorizedUser)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn init_data_validation_debug_redacts_bot_token() {
        let token = "123456:secret-token";
        let cfg = InitDataValidation {
            bot_token: token.to_string(),
            now: Utc
                .with_ymd_and_hms(2026, 5, 20, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            max_age_secs: 300,
        };

        let debug = format!("{cfg:?}");

        assert!(!debug.contains(token));
        assert!(debug.contains("<redacted>"));
    }
}
