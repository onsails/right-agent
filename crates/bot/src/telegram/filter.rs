use std::collections::HashSet;
use teloxide::types::Message;

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
