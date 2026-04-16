use std::collections::HashSet;
use teloxide::types::{ChatKind, Message};

use rightclaw::agent::allowlist::AllowlistHandle;
use super::mention::{AddressKind, BotIdentity, is_bot_addressed};

/// Returns a filter_map closure that passes a message through only when its
/// chat_id is in `allowed`. If `allowed` is empty, all messages are dropped
/// (per D-05: empty allowed_chat_ids = block all).
///
/// The closure is used in dptree Update handler chain. Returning None causes
/// dptree to skip all downstream handlers — no reply, no log (per D-06).
pub fn make_chat_id_filter(
    allowed: HashSet<i64>,
) -> impl Fn(Message) -> Option<Message> + Send + Sync + Clone + 'static {
    move |msg: Message| {
        let chat_id = msg.chat.id.0;
        if allowed.contains(&chat_id) {
            Some(msg)
        } else {
            // allowed set is logged once at startup (dispatch.rs) — no need to repeat it here
            tracing::warn!(chat_id, "message dropped: chat_id not in allow-list");
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub address: AddressKind,
    /// True iff the sender is in the global trusted-users list.
    pub sender_trusted: bool,
    /// Set to `true` for group messages when the group is opened. `false` for DM.
    pub group_open: bool,
}

pub fn make_routing_filter(
    allowlist: AllowlistHandle,
    identity: BotIdentity,
) -> impl Fn(Message) -> Option<(Message, RoutingDecision)> + Send + Sync + Clone + 'static {
    move |msg: Message| {
        // No `from` means channel post or anonymous — ignore.
        let sender = msg.from.as_ref()?;
        let sender_id = sender.id.0 as i64;
        let chat_id = msg.chat.id.0;

        // Synchronous read of the RwLock via blocking_read. Safe in teloxide
        // filter_map closures because they're sync and we only read.
        let state = allowlist.0.blocking_read();
        let sender_trusted = state.is_user_trusted(sender_id);
        let group_open = state.is_group_open(chat_id);
        drop(state);

        let is_group = !matches!(msg.chat.kind, ChatKind::Private(_));

        match is_bot_addressed(&msg, &identity) {
            None => None, // group non-mention dropped
            Some(AddressKind::DirectMessage) => {
                if !sender_trusted { return None; } // DM from non-trusted → drop
                Some((msg, RoutingDecision {
                    address: AddressKind::DirectMessage,
                    sender_trusted: true,
                    group_open: false,
                }))
            }
            Some(addr) => {
                debug_assert!(is_group);
                let _ = is_group;
                if !sender_trusted && !group_open { return None; }
                Some((msg, RoutingDecision { address: addr, sender_trusted, group_open }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_decision_constructs() {
        let d = RoutingDecision {
            address: AddressKind::DirectMessage,
            sender_trusted: true,
            group_open: false,
        };
        assert!(d.sender_trusted);
        assert!(!d.group_open);
    }
}
