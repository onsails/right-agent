pub(super) fn learning_outcome_kind(
    action: &str,
    status: Option<&str>,
    hint_outcome: Option<&str>,
) -> &'static str {
    match (action, status, hint_outcome) {
        (_, _, Some("refused")) => "skill_refused",
        (_, Some("failed"), _) => "skill_failed",
        (_, Some("aborted"), _) => "skill_aborted",
        ("create", Some("created"), _) => "skill_created",
        ("update", Some("updated"), _) => "skill_updated",
        ("create", _, _) => "skill_created",
        ("update", _, _) => "skill_updated",
        _ => "skill_learned",
    }
}

pub(super) fn learning_outcome_severity(
    status: Option<&str>,
    hint_outcome: Option<&str>,
) -> &'static str {
    match (status, hint_outcome) {
        (_, Some("refused")) => "warn",
        (Some("failed" | "aborted"), _) => "bad",
        _ => "info",
    }
}

pub(super) fn learning_outcome_title(
    action: &str,
    status: Option<&str>,
    hint_outcome: Option<&str>,
) -> &'static str {
    match (action, status, hint_outcome) {
        (_, _, Some("refused")) => "Learning refused",
        ("create", Some("created"), _) => "Skill created",
        ("update", Some("updated"), _) => "Skill updated",
        (_, Some("failed"), _) => "Learning failed",
        (_, Some("aborted"), _) => "Learning aborted",
        _ => "Learning finished",
    }
}
