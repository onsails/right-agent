use serde::Deserialize;

/// From CC JSON response.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct OutboundAttachment {
    #[serde(rename = "type")]
    pub kind: OutboundKind,
    pub path: String,
    pub filename: Option<String>,
    pub caption: Option<String>,
    #[serde(default)]
    pub media_group_id: Option<String>,
}

/// Attachment kinds CC can produce in output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundKind {
    Photo,
    Document,
    Video,
    Audio,
    Voice,
    VideoNote,
    Sticker,
    Animation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_kind_deserialize() {
        let json = r#"{"type":"photo","path":"/sandbox/outbox/img.png"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.kind, OutboundKind::Photo);
        assert_eq!(att.path, "/sandbox/outbox/img.png");
        assert!(att.filename.is_none());
        assert!(att.caption.is_none());
    }

    #[test]
    fn outbound_kind_deserialize_with_all_fields() {
        let json = r#"{"type":"document","path":"/sandbox/outbox/data.csv","filename":"results.csv","caption":"Here's the data"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.kind, OutboundKind::Document);
        assert_eq!(att.filename.as_deref(), Some("results.csv"));
        assert_eq!(att.caption.as_deref(), Some("Here's the data"));
    }

    #[test]
    fn outbound_kind_deserialize_snake_case() {
        let json = r#"{"type":"video_note","path":"/sandbox/outbox/note.mp4"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.kind, OutboundKind::VideoNote);
    }

    #[test]
    fn outbound_attachment_deserialize_without_media_group_id_defaults_none() {
        let json = r#"{"type":"photo","path":"/sandbox/outbox/a.jpg"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert!(att.media_group_id.is_none());
    }

    #[test]
    fn outbound_attachment_deserialize_with_media_group_id() {
        let json = r#"{"type":"photo","path":"/sandbox/outbox/a.jpg","media_group_id":"shots"}"#;
        let att: OutboundAttachment = serde_json::from_str(json).unwrap();
        assert_eq!(att.media_group_id.as_deref(), Some("shots"));
    }
}
