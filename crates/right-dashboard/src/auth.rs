use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardUser {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitDataValidation {
    pub bot_token: String,
    pub now: DateTime<Utc>,
    pub max_age_secs: i64,
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
