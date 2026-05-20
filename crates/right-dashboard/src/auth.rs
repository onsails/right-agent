use std::{collections::BTreeSet, fmt};

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::Deserialize;
use sha2::Sha256;
use subtle::ConstantTimeEq as _;
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

pub fn validate_init_data(raw: &str, cfg: &InitDataValidation) -> Result<DashboardUser, AuthError> {
    if raw.trim().is_empty() {
        return Err(AuthError::MissingInitData);
    }

    let mut hash = None;
    let mut user = None;
    let mut auth_date = None;
    let mut data_pairs = Vec::new();

    for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        let key = key.into_owned();
        let value = value.into_owned();

        match key.as_str() {
            "hash" => hash = Some(value),
            "signature" => {}
            "auth_date" => {
                auth_date = Some(value.clone());
                data_pairs.push((key, value));
            }
            "user" => {
                user = Some(value.clone());
                data_pairs.push((key, value));
            }
            _ => data_pairs.push((key, value)),
        }
    }

    let supplied_hash = hash.ok_or(AuthError::MalformedInitData)?;
    let auth_date = auth_date
        .ok_or(AuthError::MalformedInitData)?
        .parse::<i64>()
        .map_err(|_| AuthError::MalformedInitData)?;

    data_pairs.sort_by(|(left_key, _), (right_key, _)| left_key.cmp(right_key));
    let data_check_string = data_pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");

    let secret_key = Hmac::<Sha256>::new_from_slice(b"WebAppData")
        .map_err(|_| AuthError::MalformedInitData)?
        .chain_update(cfg.bot_token.as_bytes())
        .finalize()
        .into_bytes();
    let expected_hash = Hmac::<Sha256>::new_from_slice(&secret_key)
        .map_err(|_| AuthError::MalformedInitData)?
        .chain_update(data_check_string.as_bytes())
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    if !bool::from(expected_hash.as_bytes().ct_eq(supplied_hash.as_bytes())) {
        return Err(AuthError::InvalidHash);
    }

    let age_secs = cfg.now.timestamp() - auth_date;
    if age_secs < 0 || age_secs > cfg.max_age_secs {
        return Err(AuthError::Expired);
    }

    let user = user.ok_or(AuthError::MissingUser)?;
    let user: TelegramInitDataUser =
        serde_json::from_str(&user).map_err(|_| AuthError::MalformedInitData)?;

    Ok(DashboardUser {
        id: user.id,
        username: user.username,
        first_name: user.first_name,
    })
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

#[derive(Deserialize)]
struct TelegramInitDataUser {
    id: i64,
    username: Option<String>,
    first_name: String,
}

#[cfg(test)]
mod tests {
    use hmac::{Hmac, KeyInit as _, Mac as _};
    use serde_json::json;
    use sha2::Sha256;

    use chrono::TimeZone;

    use super::*;

    fn cfg(bot_token: &str) -> InitDataValidation {
        InitDataValidation {
            bot_token: bot_token.to_string(),
            now: Utc
                .with_ymd_and_hms(2026, 5, 20, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            max_age_secs: 300,
        }
    }

    fn signed_init_data(bot_token: &str, pairs: &[(&str, String)]) -> String {
        let mut data_pairs = pairs.to_vec();
        data_pairs.sort_by(|(left, _), (right, _)| left.cmp(right));

        let data_check_string = data_pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n");

        let secret_key = Hmac::<Sha256>::new_from_slice(b"WebAppData")
            .unwrap()
            .chain_update(bot_token.as_bytes())
            .finalize()
            .into_bytes();
        let hash = Hmac::<Sha256>::new_from_slice(&secret_key)
            .unwrap()
            .chain_update(data_check_string.as_bytes())
            .finalize()
            .into_bytes();
        let hash = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();

        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.append_pair("hash", &hash);
        serializer.finish()
    }

    fn signed_user_init_data(bot_token: &str, auth_date: i64) -> String {
        let user = json!({
            "id": 42,
            "username": "forty_two",
            "first_name": "Douglas",
        })
        .to_string();

        signed_init_data(
            bot_token,
            &[("auth_date", auth_date.to_string()), ("user", user)],
        )
    }

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

    #[test]
    fn valid_init_data_returns_user() {
        let bot_token = "123456:secret-token";
        let raw = signed_user_init_data(bot_token, 1_779_278_300);

        let user = validate_init_data(&raw, &cfg(bot_token)).expect("valid init data");

        assert_eq!(
            user,
            DashboardUser {
                id: 42,
                username: Some("forty_two".to_string()),
                first_name: "Douglas".to_string(),
            }
        );
    }

    #[test]
    fn invalid_hash_is_rejected() {
        let bot_token = "123456:secret-token";
        let mut raw = signed_user_init_data(bot_token, 1_779_278_300);
        raw.push_str("00");

        let err = validate_init_data(&raw, &cfg(bot_token)).expect_err("tampered hash");

        assert_eq!(err, AuthError::InvalidHash);
    }

    #[test]
    fn expired_auth_date_is_rejected() {
        let bot_token = "123456:secret-token";
        let raw = signed_user_init_data(bot_token, 1_779_277_999);

        let err = validate_init_data(&raw, &cfg(bot_token)).expect_err("expired auth_date");

        assert_eq!(err, AuthError::Expired);
    }

    #[test]
    fn missing_user_is_rejected() {
        let bot_token = "123456:secret-token";
        let raw = signed_init_data(bot_token, &[("auth_date", "1779278300".to_string())]);

        let err = validate_init_data(&raw, &cfg(bot_token)).expect_err("missing user");

        assert_eq!(err, AuthError::MissingUser);
    }

    #[test]
    fn authorize_user_requires_allowlist_membership() {
        let user = DashboardUser {
            id: 42,
            username: None,
            first_name: "Douglas".to_string(),
        };
        let trusted_user_ids = BTreeSet::from([7]);

        let err = authorize_user(user, &trusted_user_ids).expect_err("user not allowlisted");

        assert_eq!(err, AuthError::UnauthorizedUser);
    }
}
