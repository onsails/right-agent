use crate::generate_settings;

#[test]
fn generates_behavioral_flags() {
    let settings = generate_settings().unwrap();
    assert_eq!(settings["skipDangerousModePermissionPrompt"], true);
    assert_eq!(settings["spinnerTipsEnabled"], false);
    assert_eq!(settings["prefersReducedMotion"], true);
    assert_eq!(settings["autoMemoryEnabled"], true);
}

#[test]
fn no_sandbox_section() {
    let settings = generate_settings().unwrap();
    assert!(
        settings.get("sandbox").is_none(),
        "settings should not contain sandbox section, got: {:?}",
        settings.get("sandbox")
    );
}

#[test]
fn never_enables_telegram_plugin() {
    let settings = generate_settings().unwrap();
    assert!(settings.get("enabledPlugins").is_none());
}
