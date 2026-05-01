use right_agent::agent::allowlist::AllowlistHandle;
use teloxide::types::{ChatKind, Message};

use super::mention::{AddressKind, BotIdentity, is_bot_addressed};

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub address: Option<AddressKind>,
    /// True iff the sender is in the global trusted-users list.
    pub sender_trusted: bool,
    /// Set to `true` for group messages when the group is opened. `false` for DM.
    pub group_open: bool,
}

// Return shape note: dptree 0.5.1 `filter_map` inserts the closure's `Option<T>`
// into the DI bag as a single value — it does **not** unpack tuples. Since
// `Update::filter_message()` already places `Message` in the bag, we return
// only `Option<RoutingDecision>`. Returning `Option<(Message, RoutingDecision)>`
// would leave `RoutingDecision` unreachable from downstream handlers.
pub fn make_routing_filter(
    allowlist: AllowlistHandle,
    identity: BotIdentity,
) -> impl Fn(Message) -> Option<RoutingDecision> + Send + Sync + Clone + 'static {
    move |msg: Message| {
        // No `from` means channel post or anonymous — ignore.
        let sender = msg.from.as_ref()?;
        let sender_id = sender.id.0 as i64;
        let chat_id = msg.chat.id.0;

        let state = allowlist.0.read().expect("allowlist lock poisoned");
        let sender_trusted = state.is_user_trusted(sender_id);
        let group_open = state.is_group_open(chat_id);
        drop(state);

        let addressed = is_bot_addressed(&msg, &identity);

        match &msg.chat.kind {
            ChatKind::Private(_) => {
                if !sender_trusted {
                    return None;
                }
                Some(RoutingDecision {
                    address: Some(AddressKind::DirectMessage),
                    sender_trusted: true,
                    group_open: false,
                })
            }
            _ => {
                if !sender_trusted && !group_open {
                    return None;
                }
                // Non-album group messages still require an explicit address.
                // Album siblings are admitted unaddressed; the worker aggregates
                // them and applies a final addressed-batch gate before invoking CC.
                if addressed.is_none()
                    && msg.media_group_id().is_none()
                    && msg.forward_origin().is_none()
                {
                    return None;
                }
                Some(RoutingDecision {
                    address: addressed,
                    sender_trusted,
                    group_open,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use right_agent::agent::allowlist::{AllowedGroup, AllowedUser, AllowlistFile, AllowlistState};
    use std::sync::Arc;

    fn allowlist_with(users: Vec<i64>, groups: Vec<i64>) -> AllowlistHandle {
        let now = Utc::now();
        let users = users
            .into_iter()
            .map(|id| AllowedUser {
                id,
                label: None,
                added_by: None,
                added_at: now,
            })
            .collect();
        let groups = groups
            .into_iter()
            .map(|id| AllowedGroup {
                id,
                label: None,
                opened_by: None,
                opened_at: now,
            })
            .collect();
        let file = AllowlistFile {
            version: right_agent::agent::allowlist::CURRENT_VERSION,
            users,
            groups,
        };
        AllowlistHandle(Arc::new(std::sync::RwLock::new(
            AllowlistState::from_file(file),
        )))
    }

    fn group_msg_with_media_group(
        chat_id: i64,
        sender_id: i64,
        media_group_id: Option<&str>,
        caption_with_mention: bool,
        bot_username: &str,
    ) -> teloxide::types::Message {
        let mut payload = serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "photo": [{
                "file_id": "AgAD",
                "file_unique_id": "u",
                "width": 1, "height": 1
            }],
        });
        if let Some(mgid) = media_group_id {
            payload["media_group_id"] = serde_json::Value::String(mgid.to_string());
        }
        if caption_with_mention {
            let cap = format!("@{bot_username} hi");
            payload["caption"] = serde_json::Value::String(cap.clone());
            payload["caption_entities"] = serde_json::json!([{
                "type": "mention",
                "offset": 0,
                "length": bot_username.len() as i64 + 1
            }]);
        }
        serde_json::from_value(payload).unwrap()
    }

    #[test]
    fn routing_decision_constructs() {
        let d = RoutingDecision {
            address: Some(AddressKind::DirectMessage),
            sender_trusted: true,
            group_open: false,
        };
        assert!(d.sender_trusted);
        assert!(!d.group_open);
    }

    #[test]
    fn media_group_sibling_without_mention_passes_for_open_group() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        let msg = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("alb"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );

        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("media-group sibling should pass in open group");
        assert!(d.address.is_none());
        assert!(d.group_open);
    }

    #[test]
    fn ordinary_group_message_without_mention_still_dropped() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        // No media_group_id, no caption mention — a plain text post.
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "text": "hello there"
        }))
        .unwrap();

        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }

    #[test]
    fn media_group_sibling_without_mention_dropped_for_untrusted_sender() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        // No trusted users, no open groups → sender is neither trusted nor in an open group.
        let allowlist = allowlist_with(vec![], vec![]);

        let msg = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("alb"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );

        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }

    #[test]
    fn media_group_sibling_with_mention_passes_with_some_address() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        let msg = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("alb"),
            /*caption_with_mention=*/ true,
            &identity.username,
        );

        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("captioned sibling must pass");
        assert!(matches!(
            d.address,
            Some(AddressKind::GroupMentionText)
        ));
    }

    #[test]
    fn vonder_repro_three_album_siblings_all_routed() {
        // Reproduces the bug from ~/.right/logs/him.log.2026-04-27 lines 137-152:
        // three messages sharing media_group_id, only the third carries the @mention.
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -4996137249;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        let f = make_routing_filter(allowlist, identity.clone());

        let s1 = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("vonder-album"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );
        let s2 = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("vonder-album"),
            /*caption_with_mention=*/ false,
            &identity.username,
        );
        let s3 = group_msg_with_media_group(
            chat_id,
            sender_id,
            Some("vonder-album"),
            /*caption_with_mention=*/ true,
            &identity.username,
        );

        assert!(f(s1).is_some(), "sibling 1 must reach handle_message");
        assert!(f(s2).is_some(), "sibling 2 must reach handle_message");
        let d3 = f(s3).expect("captioned sibling must reach handle_message");
        assert!(d3.address.is_some());
    }

    #[test]
    fn forward_origin_passes_in_open_group() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        // Forwarded document, no caption, no @mention anywhere.
        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "forward_origin": {
                "type": "user",
                "date": 0,
                "sender_user": {"id": 99999, "is_bot": false, "first_name": "Sender"}
            },
            "document": {
                "file_id": "BAAD",
                "file_unique_id": "uniq",
                "file_name": "edf.pdf",
                "mime_type": "application/pdf",
                "file_size": 1024
            }
        }))
        .unwrap();

        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("forward should pass in open group");
        assert!(d.address.is_none());
        assert!(d.group_open);
    }

    #[test]
    fn forward_origin_dropped_for_untrusted_sender_and_closed_group() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        // No trusted users, no open groups.
        let allowlist = allowlist_with(vec![], vec![]);

        let msg: teloxide::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "forward_origin": {
                "type": "user",
                "date": 0,
                "sender_user": {"id": 99999, "is_bot": false, "first_name": "Sender"}
            },
            "document": {
                "file_id": "BAAD",
                "file_unique_id": "uniq",
                "file_name": "edf.pdf",
                "mime_type": "application/pdf",
                "file_size": 1024
            }
        }))
        .unwrap();

        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }
}
