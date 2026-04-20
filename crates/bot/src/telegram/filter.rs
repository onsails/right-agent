use rightclaw::agent::allowlist::AllowlistHandle;
use teloxide::types::{ChatKind, Message};

use super::mention::{AddressKind, BotIdentity, is_bot_addressed};

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub address: AddressKind,
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

        match is_bot_addressed(&msg, &identity) {
            None => None, // group non-mention dropped
            Some(AddressKind::DirectMessage) => {
                if !sender_trusted {
                    return None;
                } // DM from non-trusted → drop
                Some(RoutingDecision {
                    address: AddressKind::DirectMessage,
                    sender_trusted: true,
                    group_open: false,
                })
            }
            Some(addr) => {
                debug_assert!(!matches!(msg.chat.kind, ChatKind::Private(_)));
                if !sender_trusted && !group_open {
                    return None;
                }
                Some(RoutingDecision {
                    address: addr,
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
