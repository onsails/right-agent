//! Update routing. Replaces teloxide's Dispatcher/dptree. The pure
//! classification fns here are unit-tested; route_update + HandlerCtx (Phase 2)
//! map an UpdateContent to a handler call.

/// Which callback handler an inline-button `callback_query.data` routes to.
/// Mirrors the `dptree` callback branch order in `dispatch.rs`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CallbackRoute {
    Model,
    Mode,
    Thinking,
    Bg,
    ErrorDetails,
    Stop,
}

/// Classify inline-button callback data by prefix. The `Stop` fallthrough
/// matches the prior `.endpoint(handle_stop_callback)` default branch (also
/// covers `None` data).
pub(crate) fn classify_callback(data: Option<&str>) -> CallbackRoute {
    match data {
        Some(d) if d.starts_with("model:") => CallbackRoute::Model,
        Some(d) if d.starts_with("mode:") || d.starts_with("modegroup:") => CallbackRoute::Mode,
        Some(d) if d.starts_with("think:") => CallbackRoute::Thinking,
        Some(d) if d.starts_with("bg:") => CallbackRoute::Bg,
        Some(d) if d.starts_with("errdet:") => CallbackRoute::ErrorDetails,
        _ => CallbackRoute::Stop,
    }
}

/// Outcome of authenticating + parsing an inbound webhook request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WebhookOutcome {
    Unauthorized,
    AckIgnore,
    Routed,
}

/// Decide how to handle a webhook POST: a missing or mismatched secret header
/// is `Unauthorized`; a valid secret with an unparseable body is `AckIgnore`
/// (200 to stop Telegram retries); a valid secret with a parseable body is
/// `Routed`.
pub(crate) fn webhook_outcome(
    secret_header: Option<&str>,
    expected_secret: &str,
    body_parses: bool,
) -> WebhookOutcome {
    match secret_header {
        Some(s) if s == expected_secret => {
            if body_parses {
                WebhookOutcome::Routed
            } else {
                WebhookOutcome::AckIgnore
            }
        }
        _ => WebhookOutcome::Unauthorized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_callback_routes_each_prefix() {
        assert_eq!(classify_callback(Some("model:opus")), CallbackRoute::Model);
        assert_eq!(classify_callback(Some("mode:all")), CallbackRoute::Mode);
        assert_eq!(classify_callback(Some("modegroup:x")), CallbackRoute::Mode);
        assert_eq!(classify_callback(Some("think:on")), CallbackRoute::Thinking);
        assert_eq!(classify_callback(Some("bg:123")), CallbackRoute::Bg);
        assert_eq!(
            classify_callback(Some("errdet:1")),
            CallbackRoute::ErrorDetails
        );
    }

    #[test]
    fn classify_callback_falls_through_to_stop() {
        assert_eq!(classify_callback(Some("stop:1")), CallbackRoute::Stop);
        assert_eq!(classify_callback(None), CallbackRoute::Stop);
    }

    #[test]
    fn webhook_outcome_unauthorized_without_matching_secret() {
        assert_eq!(
            webhook_outcome(None, "s", true),
            WebhookOutcome::Unauthorized
        );
        assert_eq!(
            webhook_outcome(Some("nope"), "s", true),
            WebhookOutcome::Unauthorized
        );
    }

    #[test]
    fn webhook_outcome_routes_valid_secret_and_body() {
        assert_eq!(
            webhook_outcome(Some("s"), "s", true),
            WebhookOutcome::Routed
        );
    }

    #[test]
    fn webhook_outcome_acks_valid_secret_unparseable_body() {
        assert_eq!(
            webhook_outcome(Some("s"), "s", false),
            WebhookOutcome::AckIgnore
        );
    }
}
