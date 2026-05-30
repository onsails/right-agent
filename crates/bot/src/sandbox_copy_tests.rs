use super::{back_online_message, unavailable_message};
use right_openshell::diagnosis::GatewayCause;

#[test]
fn unavailable_message_leads_with_consequence_and_includes_fix() {
    let d = GatewayCause::DockerDown.diagnose();
    let msg = unavailable_message(&d);
    assert!(msg.starts_with("⚠️"));
    assert!(msg.to_lowercase().contains("offline"));
    assert!(msg.to_lowercase().contains("docker"));
    // No raw CLI-style prefixes.
    assert!(!msg.contains("Failed:"));
    assert!(!msg.contains("Error:"));
}

#[test]
fn unavailable_message_html_escapes_dynamic_text() {
    let d = GatewayCause::SandboxNotFound {
        sandbox: "a<b>&c".to_owned(),
    }
    .diagnose();
    let msg = unavailable_message(&d);
    assert!(msg.contains("a&lt;b&gt;&amp;c"));
    assert!(!msg.contains("a<b>"));
}

#[test]
fn back_online_message_is_positive_and_short() {
    let msg = back_online_message();
    assert!(msg.starts_with("✅"));
    assert!(msg.to_lowercase().contains("online"));
}
