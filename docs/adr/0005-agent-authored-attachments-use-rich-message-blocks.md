# Agent-authored attachments use rich-message blocks without captions

Status: accepted

Agent-authored captions preserved a Markdown parser beside the typed Rich Content contract, so literal financial notation could still become formatting. Right removes attachment captions and sends supported single media as typed Telegram rich-message blocks; `sendMediaGroup`, sticker, and video-note remain captionless legacy transports because Telegram has no equivalent rich-message semantics for those cases.

## Consequences

Agent-authored formatting is declared only through Rich Content. Message text stays separate from attachments, albums retain their established grouping behavior, and progress text is literal plain text.