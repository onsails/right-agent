use frankenstein::types::Message;
use right_agent::agent::allowlist::{AllowlistHandle, ResponseMode};

use super::mention::{AddressKind, BotIdentity, is_bot_addressed};
use super::msg_ext;

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub address: Option<AddressKind>,
    pub response_mode: ResponseMode,
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
        let sender_id = sender.id as i64;
        let chat_id = msg.chat.id;

        let state = allowlist.0.read().expect("allowlist lock poisoned");
        let sender_trusted = state.is_user_trusted(sender_id);
        let group_open = state.is_group_open(chat_id);
        // Response mode is a group/topic concept; DMs are always `Addressed`, so
        // skip the per-message group lookup for private chats (the routing filter
        // runs on every update).
        let response_mode = if msg_ext::is_private(&msg.chat) {
            ResponseMode::Addressed
        } else {
            state.response_mode(chat_id, super::session::effective_thread_id(&msg))
        };
        drop(state);

        let addressed = is_bot_addressed(&msg, &identity);

        if msg_ext::is_private(&msg.chat) {
            if !sender_trusted {
                return None;
            }
            return Some(RoutingDecision {
                address: Some(AddressKind::DirectMessage),
                response_mode: ResponseMode::Addressed,
                sender_trusted: true,
                group_open: false,
            });
        }

        // Group contexts never route bot senders; this loop guard
        // applies before both All-mode and addressed fallbacks.
        if sender.is_bot {
            return None;
        }
        // `All` mode in an open group answers everyone, no addressing.
        if response_mode == ResponseMode::All && group_open {
            return Some(RoutingDecision {
                address: addressed,
                response_mode,
                sender_trusted,
                group_open,
            });
        }
        if !sender_trusted && !group_open {
            return None;
        }
        // Non-album/non-forward group messages still require an explicit
        // address. Album siblings and forwards are admitted unaddressed;
        // the worker aggregates them and applies the post-debounce
        // invocation gate before invoking CC.
        if addressed.is_none() && msg.media_group_id.is_none() && msg.forward_origin.is_none() {
            return None;
        }
        Some(RoutingDecision {
            address: addressed,
            response_mode,
            sender_trusted,
            group_open,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use right_agent::agent::allowlist::{
        AllowedGroup, AllowedUser, AllowlistFile, AllowlistState, ResponseMode,
    };
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
                mode: ResponseMode::Addressed,
                topics: Vec::new(),
            })
            .collect();
        let file = AllowlistFile {
            version: right_agent::agent::allowlist::CURRENT_VERSION,
            users,
            groups,
        };
        AllowlistHandle(Arc::new(std::sync::RwLock::new(AllowlistState::from_file(
            file,
        ))))
    }

    fn open_group_with_mode(chat_id: i64, mode: ResponseMode) -> AllowlistHandle {
        let g = AllowedGroup {
            id: chat_id,
            label: None,
            opened_by: None,
            opened_at: Utc::now(),
            mode,
            topics: vec![],
        };
        let file = AllowlistFile {
            version: right_agent::agent::allowlist::CURRENT_VERSION,
            users: vec![],
            groups: vec![g],
        };
        AllowlistHandle(Arc::new(std::sync::RwLock::new(AllowlistState::from_file(
            file,
        ))))
    }

    fn group_msg_with_media_group(
        chat_id: i64,
        sender_id: i64,
        media_group_id: Option<&str>,
        caption_with_mention: bool,
        bot_username: &str,
    ) -> frankenstein::types::Message {
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

    fn plain_group_text(chat_id: i64, sender_id: i64, text: &str) -> frankenstein::types::Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "text": text
        }))
        .unwrap()
    }

    fn private_text_msg(chat_id: i64, sender_id: i64, text: &str) -> frankenstein::types::Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "private", "first_name": "User"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "User"},
            "text": text
        }))
        .unwrap()
    }

    fn private_photo_caption_msg(
        chat_id: i64,
        sender_id: i64,
        caption: &str,
    ) -> frankenstein::types::Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 2,
            "date": 0,
            "chat": {"id": chat_id, "type": "private", "first_name": "User"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "User"},
            "caption": caption,
            "photo": [{
                "file_id": "AgAD-private",
                "file_unique_id": "private-photo",
                "width": 1,
                "height": 1
            }]
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn routing_decision_constructs() {
        let d = RoutingDecision {
            address: Some(AddressKind::DirectMessage),
            response_mode: ResponseMode::Addressed,
            sender_trusted: true,
            group_open: false,
        };
        assert!(d.sender_trusted);
        assert!(!d.group_open);
    }

    #[tokio::test]
    async fn untrusted_private_text_message_is_dropped() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let sender_id = 42;
        let msg = private_text_msg(sender_id, sender_id, "spam text");
        let allowlist = allowlist_with(vec![], vec![]);

        let f = make_routing_filter(allowlist, identity);

        assert!(f(msg).is_none());
    }

    #[tokio::test]
    async fn untrusted_private_media_caption_message_is_dropped() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let sender_id = 42;
        let msg = private_photo_caption_msg(sender_id, sender_id, "spam caption");
        let allowlist = allowlist_with(vec![], vec![]);

        let f = make_routing_filter(allowlist, identity);

        assert!(f(msg).is_none());
    }

    #[tokio::test]
    async fn trusted_private_text_routes_as_direct_message() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let sender_id = 42;
        let msg = private_text_msg(sender_id, sender_id, "hello");
        let allowlist = allowlist_with(vec![sender_id], vec![]);

        let f = make_routing_filter(allowlist, identity);
        let decision = f(msg).expect("trusted private text should route");

        assert_eq!(decision.address, Some(AddressKind::DirectMessage));
        assert_eq!(decision.response_mode, ResponseMode::Addressed);
        assert!(decision.sender_trusted);
        assert!(!decision.group_open);
    }

    #[tokio::test]
    async fn trusted_private_media_caption_routes_as_direct_message() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let sender_id = 42;
        let msg = private_photo_caption_msg(sender_id, sender_id, "hello with media");
        let allowlist = allowlist_with(vec![sender_id], vec![]);

        let f = make_routing_filter(allowlist, identity);
        let decision = f(msg).expect("trusted private media should route");

        assert_eq!(decision.address, Some(AddressKind::DirectMessage));
        assert_eq!(decision.response_mode, ResponseMode::Addressed);
        assert!(decision.sender_trusted);
        assert!(!decision.group_open);
    }

    #[tokio::test]
    async fn all_mode_admits_untrusted_unaddressed_text() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let allowlist = open_group_with_mode(chat_id, ResponseMode::All);
        let msg = plain_group_text(chat_id, 42, "какие у нас кроны есть?");
        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("All mode admits plain text");
        assert!(d.address.is_none());
        assert_eq!(d.response_mode, ResponseMode::All);
        assert!(d.group_open);
    }

    #[tokio::test]
    async fn addressed_mode_drops_unaddressed_text_even_when_open() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let allowlist = open_group_with_mode(chat_id, ResponseMode::Addressed);
        let msg = plain_group_text(chat_id, 42, "just chatting");
        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }

    #[tokio::test]
    async fn all_mode_ignores_other_bots() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let allowlist = open_group_with_mode(chat_id, ResponseMode::All);
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": 5000, "is_bot": true, "first_name": "OtherBot"},
            "text": "loop bait"
        }))
        .unwrap();
        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none(), "All mode must not answer other bots");
    }

    #[tokio::test]
    async fn all_mode_ignores_addressed_other_bots() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let allowlist = open_group_with_mode(chat_id, ResponseMode::All);
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": 5000, "is_bot": true, "first_name": "OtherBot"},
            "text": "@rightaww_bot loop bait",
            "entities": [{
                "type": "mention",
                "offset": 0,
                "length": 13
            }]
        }))
        .unwrap();
        let f = make_routing_filter(allowlist, identity);
        assert!(
            f(msg).is_none(),
            "All mode must not answer addressed messages from other bots"
        );
    }

    #[tokio::test]
    async fn media_group_sibling_without_mention_passes_for_open_group() {
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

    #[tokio::test]
    async fn ordinary_group_message_without_mention_still_dropped() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        // No media_group_id, no caption mention — a plain text post.
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
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

    #[tokio::test]
    async fn unaddressed_group_message_still_dropped_by_routing_filter() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": chat_id, "type": "supergroup", "title": "g"},
            "from": {"id": sender_id, "is_bot": false, "first_name": "U"},
            "text": "unaddressed group message"
        }))
        .unwrap();

        let f = make_routing_filter(allowlist, identity);
        assert!(f(msg).is_none());
    }

    #[tokio::test]
    async fn media_group_sibling_without_mention_dropped_for_untrusted_sender() {
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

    #[tokio::test]
    async fn media_group_sibling_with_mention_passes_with_some_address() {
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
        assert!(matches!(d.address, Some(AddressKind::GroupMentionText)));
    }

    #[tokio::test]
    async fn vonder_repro_three_album_siblings_all_routed() {
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

    #[tokio::test]
    async fn forward_origin_passes_in_open_group() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        // Forwarded document, no caption, no @mention anywhere.
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
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

    #[tokio::test]
    async fn forward_origin_dropped_for_untrusted_sender_and_closed_group() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        // No trusted users, no open groups.
        let allowlist = allowlist_with(vec![], vec![]);

        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
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

    #[tokio::test]
    async fn forward_with_caption_mention_routes_as_addressed() {
        let identity = BotIdentity {
            username: "rightaww_bot".into(),
            user_id: 999,
        };
        let chat_id = -1001;
        let sender_id = 42;
        let allowlist = allowlist_with(vec![], vec![chat_id]);

        // Forwarded message with a fresh user-typed caption containing @mention.
        // Address detection must win — `address` should be Some(GroupMentionText),
        // not None (which would mean "admitted only by forward gate").
        let msg: frankenstein::types::Message = serde_json::from_value(serde_json::json!({
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
            },
            "caption": "@rightaww_bot вот этот",
            "caption_entities": [{
                "type": "mention",
                "offset": 0,
                "length": 13
            }]
        }))
        .unwrap();

        let f = make_routing_filter(allowlist, identity);
        let d = f(msg).expect("forward with caption mention must route");
        assert_eq!(d.address, Some(AddressKind::GroupMentionText));
    }
}
