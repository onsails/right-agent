use crate::{
    BG_CONTINUATION_SCHEMA_JSON, BOOTSTRAP_SCHEMA_JSON, CRON_SCHEMA_JSON, REPLY_SCHEMA_JSON,
    generate_system_prompt,
};

fn enum_values(field: &serde_json::Value) -> Vec<&str> {
    field["enum"]
        .as_array()
        .expect("field enum must be an array")
        .iter()
        .map(|value| value.as_str().expect("enum value must be a string"))
        .collect()
}

#[test]
fn reply_schema_json_is_valid() {
    let parsed: serde_json::Value =
        serde_json::from_str(REPLY_SCHEMA_JSON).expect("REPLY_SCHEMA_JSON must be valid JSON");
    assert!(parsed.get("required").is_some());
}

#[test]
fn reply_schema_contains_learned_skill_fields() {
    let parsed: serde_json::Value =
        serde_json::from_str(REPLY_SCHEMA_JSON).expect("REPLY_SCHEMA_JSON must be valid JSON");
    let properties = parsed["properties"]
        .as_object()
        .expect("schema properties must be an object");
    assert!(
        properties.contains_key("used_skill_receipts"),
        "schema must include used_skill_receipts"
    );
    assert!(
        properties.contains_key("learning_signal"),
        "schema must include learning_signal"
    );
    assert!(
        properties.contains_key("skill_issue_signal"),
        "schema must include skill_issue_signal"
    );

    let required = parsed["required"]
        .as_array()
        .expect("schema required must be an array");
    let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(required_strs.contains(&"content"), "must require content");
    assert!(
        !required_strs.contains(&"used_skill_receipts"),
        "used_skill_receipts must be optional"
    );

    for path in [
        &["learning_signal", "trigger"][..],
        &["learning_signal", "reason_not_written"][..],
        &["skill_issue_signal", "issue"][..],
        &["skill_issue_signal", "reason_not_patched"][..],
        &["skill_issue_signal", "observed_effect"][..],
    ] {
        let field = &properties[path[0]]["properties"][path[1]];
        assert!(
            field.get("enum").and_then(|v| v.as_array()).is_some(),
            "{}.{} must use a JSON Schema enum",
            path[0],
            path[1]
        );
    }

    let trigger_enum = enum_values(&properties["learning_signal"]["properties"]["trigger"]);
    assert_eq!(
        trigger_enum,
        vec![
            "explicit_user_request",
            "multi_step_workflow",
            "recovered_surprise",
            "user_correction",
            "repeated_tool_pattern",
        ]
    );

    let issue_enum = enum_values(&properties["skill_issue_signal"]["properties"]["issue"]);
    assert_eq!(
        issue_enum,
        vec![
            "missing_step",
            "stale_command",
            "wrong_api_assumption",
            "overbroad_activation",
            "broken_script",
            "unsafe_instruction",
        ]
    );
}

#[test]
fn bootstrap_schema_json_is_valid() {
    let parsed: serde_json::Value = serde_json::from_str(BOOTSTRAP_SCHEMA_JSON)
        .expect("BOOTSTRAP_SCHEMA_JSON must be valid JSON");
    let required = parsed.get("required").unwrap().as_array().unwrap();
    let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(required_strs.contains(&"content"), "must require content");
    assert!(
        required_strs.contains(&"bootstrap_complete"),
        "must require bootstrap_complete"
    );
}

#[test]
fn bootstrap_schema_has_bootstrap_complete_field() {
    let parsed: serde_json::Value = serde_json::from_str(BOOTSTRAP_SCHEMA_JSON).unwrap();
    let props = parsed.get("properties").unwrap();
    assert!(
        props.get("bootstrap_complete").is_some(),
        "must have bootstrap_complete property"
    );
}

#[test]
fn system_prompt_contains_agent_name() {
    let result = generate_system_prompt(
        "mybot",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(result.contains("mybot"));
}

#[test]
fn system_prompt_contains_right_description() {
    let result = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(result.contains("Right Agent"));
    assert!(result.contains("multi-agent runtime"));
}

#[test]
fn system_prompt_contains_sandbox_mode() {
    let openshell = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(openshell.contains("OpenShell"));

    let none = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::None,
        "/test/agent/home",
    );
    assert!(none.contains("no sandbox"));
}

#[test]
fn system_prompt_mentions_right_mcp() {
    let result = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(result.contains("right"));
    assert!(result.contains("MCP"));
}

#[test]
fn system_prompt_delegates_remember_routing_to_right_memory_skill() {
    let result = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );

    for needle in [
        "Identity files are always-loaded durable context",
        "`SOUL.md`",
        "agent-authored durable voice",
        "/right-memory",
    ] {
        assert!(
            result.contains(needle),
            "system prompt must preserve identity ownership and right-memory delegation: missing {needle:?}"
        );
    }

    for forbidden in [
        concat!("compact ", "operating contract"),
        "\"Remember\" requests are routed by semantic type before storage. Tool/API/env rules go to",
    ] {
        assert!(
            !result.contains(forbidden),
            "system prompt must not duplicate detailed routing or prescribe SOUL defaults: found {forbidden:?}"
        );
    }
}

#[test]
fn system_prompt_contains_ssh_block_for_openshell() {
    let result = generate_system_prompt(
        "mybot",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );
    assert!(
        result.contains("right agent ssh mybot"),
        "openshell prompt must include SSH command"
    );
    assert!(
        result.contains("interactive terminal"),
        "openshell prompt must explain when to use SSH"
    );
}

#[test]
fn system_prompt_no_ssh_block_for_no_sandbox() {
    let result = generate_system_prompt(
        "mybot",
        &right_agent_config::SandboxMode::None,
        "/test/agent/home",
    );
    assert!(
        !result.contains("right agent ssh"),
        "no-sandbox prompt must NOT include SSH command"
    );
}

#[test]
fn system_prompt_openshell_mentions_user_local_bin_contract() {
    let result = generate_system_prompt(
        "mybot",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );

    assert!(result.contains("## User-Installed CLI Tools"));
    assert!(result.contains("/sandbox/.local/bin"));
    assert!(result.contains("Do not install tools into `~/bin`"));
    assert!(result.contains("Do not use sudo for tool installs"));
    assert!(result.contains("NPM_CONFIG_PREFIX=/sandbox/.local"));
    assert!(result.contains("NPM_CONFIG_CACHE=/sandbox/.npm"));
    assert!(result.contains("npm install -g"));
}

#[test]
fn system_prompt_no_sandbox_omits_sandbox_user_local_bin_contract() {
    let result = generate_system_prompt(
        "mybot",
        &right_agent_config::SandboxMode::None,
        "/Users/example/.right/agents/mybot",
    );

    assert!(!result.contains("/sandbox/.local/bin"));
    assert!(!result.contains("User-Installed CLI Tools"));
    assert!(!result.contains("NPM_CONFIG_PREFIX=/sandbox/.local"));
    assert!(!result.contains("Do not install tools into `~/bin`"));
    assert!(!result.contains("Do not use sudo for tool installs"));
}

#[test]
fn operating_instructions_constant_is_non_empty() {
    assert!(
        !crate::OPERATING_INSTRUCTIONS.is_empty(),
        "OPERATING_INSTRUCTIONS must not be empty"
    );
    assert!(
        crate::OPERATING_INSTRUCTIONS.contains("## Your Files"),
        "OPERATING_INSTRUCTIONS must contain Your Files section"
    );
    assert!(
        crate::OPERATING_INSTRUCTIONS.contains("## MCP Management"),
        "OPERATING_INSTRUCTIONS must contain MCP Management section"
    );
}

#[test]
fn operating_instructions_document_inbound_reply_metadata() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    for needle in [
        "reply_to_id: <telegram message id being replied to, optional>",
        "quoted_text: <selected Telegram partial reply quote text, optional>",
        "reply_to:",
        "author: <full author block for replied-to non-bot message>",
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must document inbound reply metadata: missing {needle:?}"
        );
    }
}

#[test]
fn operating_instructions_keep_soul_agent_authored_and_delegate_remember_routing() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    for needle in [
        "`SOUL.md`",
        "agent-authored durable voice",
        "Do not invent platform-default content for this file",
        "Use the `/right-memory` skill to classify the correct persistence target",
        "smallest accurate edit",
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must describe ownership-safe SOUL routing: missing {needle:?}"
        );
    }

    for forbidden in [
        concat!("compact ", "operating contract"),
        "Edit the always-loaded file when the fact belongs in one",
        "`TOOLS.md` for tool/API/environment rules",
        "`USER.md` for user profile",
        "`SOUL.md` for your voice",
        "Tool-selection rules or integration quirks",
        "Your identity, values, style",
        "Stable user preferences",
        "Procedures and reusable workflows",
    ] {
        assert!(
            !ops.contains(forbidden),
            "OPERATING_INSTRUCTIONS must not duplicate detailed routing or platform SOUL defaults: found {forbidden:?}"
        );
    }
}

#[test]
fn operating_instructions_route_reusable_workflows_to_right_learn_skill() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    for needle in [
        "/right-learn-skill",
        "When you discover a reusable procedure",
        "needs repair",
        right_mcp::LEARNED_SKILL_PREFIX,
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must mention {needle:?}"
        );
    }
}

#[test]
fn right_memory_skills_route_remember_requests_by_storage_layer() {
    let hindsight = include_str!("../skills/right-memory-hindsight/SKILL.md");
    let file = include_str!("../skills/right-memory-file/SKILL.md");

    for (name, skill) in [
        ("right-memory-hindsight", hindsight),
        ("right-memory-file", file),
    ] {
        for needle in [
            "When the user says \"remember\"",
            "first classify what kind of persistent fact it is",
            "`TOOLS.md`",
            "`USER.md`",
            "`SOUL.md`",
            "`IDENTITY.md`",
            "memory is the fallback",
        ] {
            assert!(
                skill.contains(needle),
                "{name} must route remember requests by storage layer: missing {needle:?}"
            );
        }
    }
}

#[test]
fn operating_instructions_teach_used_learned_skill_receipts() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    for needle in [
        right_mcp::LEARNED_SKILL_PREFIX,
        "used_skill_receipts",
        "materially guides your answer",
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must teach learned-skill receipt rule: missing {needle:?}"
        );
    }
}

#[test]
fn right_learn_skill_prompt_uses_current_prefix_and_exact_signal_rules() {
    let skill = include_str!("../skills/right-learn-skill/SKILL.md");
    assert!(
        skill.contains("description: >-\n  Use when"),
        "right-learn-skill description should start with Use when"
    );
    assert!(
        skill.contains(right_mcp::LEARNED_SKILL_PREFIX),
        "right-learn-skill must mention learned-skill prefix {:?}",
        right_mcp::LEARNED_SKILL_PREFIX
    );
    for needle in [
        "used_skill_receipts",
        "at most one",
        "successful `mcp__right__skill_learning_finish`",
        "1 non-empty event ref",
        "2+ non-empty event refs",
    ] {
        assert!(
            skill.contains(needle),
            "right-learn-skill must mention {needle:?}"
        );
    }
}

#[test]
fn learned_skill_prompt_text_has_no_old_or_invalid_prefixes() {
    let learn_skill = include_str!("../skills/right-learn-skill/SKILL.md");
    let agent_texts = [
        ("OPERATING_INSTRUCTIONS", crate::OPERATING_INSTRUCTIONS),
        ("right-learn-skill", learn_skill),
    ];
    for (name, text) in agent_texts {
        for forbidden in ["rl-", "_right-"] {
            assert!(
                !text.contains(forbidden),
                "{name} must not mention old or invalid learned-skill prefix {forbidden:?}"
            );
        }
    }
}

/// Pin the hardcoded cron-idle minutes in OPERATING_INSTRUCTIONS.md to the
/// `IDLE_THRESHOLD_MIN` constant. The template is included verbatim via
/// `include_str!`, so the number cannot be templated — this test fails when
/// the constant changes without a matching prose update.
///
/// Whitespace is normalized before matching so markdown line-wrapping
/// inside a paragraph doesn't break the check (`"2\nminutes"` still
/// matches `"2 minutes"`).
#[test]
fn operating_instructions_cron_idle_threshold_matches_const() {
    let normalized: String = crate::OPERATING_INSTRUCTIONS
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let needle = format!(
        "idle for **{} minutes**",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
    assert!(
        normalized.contains(&needle),
        "OPERATING_INSTRUCTIONS must mention `idle for **{} minutes**` to match \
         right_platform_knobs::IDLE_THRESHOLD_MIN",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
    let promise_needle = format!(
        "sooner than {} minutes",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
    assert!(
        normalized.contains(&promise_needle),
        "OPERATING_INSTRUCTIONS must spell out the \"never promise sooner than {} minutes\" rule",
        right_platform_knobs::IDLE_THRESHOLD_MIN
    );
}

#[test]
fn bootstrap_instructions_constant_is_non_empty() {
    assert!(
        !crate::BOOTSTRAP_INSTRUCTIONS.is_empty(),
        "BOOTSTRAP_INSTRUCTIONS must not be empty"
    );
    assert!(
        crate::BOOTSTRAP_INSTRUCTIONS.contains("First-Time Setup"),
        "BOOTSTRAP_INSTRUCTIONS must contain bootstrap header"
    );
    assert!(
        crate::BOOTSTRAP_INSTRUCTIONS.contains("### IDENTITY.md"),
        "BOOTSTRAP_INSTRUCTIONS must contain IDENTITY.md structure"
    );
    assert!(
        crate::BOOTSTRAP_INSTRUCTIONS.contains("### SOUL.md"),
        "BOOTSTRAP_INSTRUCTIONS must contain SOUL.md structure"
    );
}

#[test]
fn bootstrap_instructions_do_not_invent_platform_soul_contract() {
    let bootstrap = crate::BOOTSTRAP_INSTRUCTIONS;
    for needle in [
        "Personality based only on chosen vibe and explicit bootstrap signals.",
        "Suggested headings when there is evidence:",
        "If the user gave no signal for a section, omit it or keep it minimal. Do not invent a platform-default operating contract.",
    ] {
        assert!(
            bootstrap.contains(needle),
            "BOOTSTRAP_INSTRUCTIONS must keep SOUL user/agent-authored: missing {needle:?}"
        );
    }

    for forbidden in [
        "**Operating Contract**",
        "act on reversible low-risk work",
        "credential/security, or private-data actions",
        "usable outcomes over polished artifacts",
        "match the user's language",
        "ask, don't guess",
    ] {
        assert!(
            !bootstrap.contains(forbidden),
            "BOOTSTRAP_INSTRUCTIONS must not prescribe platform SOUL content: found {forbidden:?}"
        );
    }
}

#[test]
fn cron_instructions_const_is_nonempty() {
    assert!(
        !crate::CRON_INSTRUCTIONS.is_empty(),
        "CRON_INSTRUCTIONS must not be empty"
    );
}

#[test]
fn cron_instructions_contains_delivery_contract_header() {
    assert!(
        crate::CRON_INSTRUCTIONS.contains("## Cron Delivery Contract"),
        "CRON_INSTRUCTIONS must contain Cron Delivery Contract header"
    );
}

#[test]
fn cron_instructions_contains_delivery_rule_marker() {
    assert!(
        crate::CRON_INSTRUCTIONS.contains("structured output IS the Telegram message"),
        "CRON_INSTRUCTIONS must explain that structured output IS the Telegram message"
    );
}

#[test]
fn cron_instructions_contains_no_clarifying_questions_rule() {
    assert!(
        crate::CRON_INSTRUCTIONS.contains("No clarifying questions"),
        "CRON_INSTRUCTIONS must contain No clarifying questions section"
    );
}

#[test]
fn operating_instructions_teach_sparse_progress_updates() {
    let ops = crate::OPERATING_INSTRUCTIONS;

    for needle in [
        "mcp__right__send_progress",
        "30 seconds",
        "complex, long-running",
        "parallel or sequential subagents",
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must mention {needle:?}"
        );
    }
}

#[test]
fn operating_instructions_teach_agent_tool_delegation() {
    let ops = crate::OPERATING_INSTRUCTIONS;

    for needle in [
        "`Agent` tool",
        "intermediate results",
        "main session is accountable",
        "synthesize for the user",
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must teach Agent-tool delegation: missing {needle:?}"
        );
    }
}

#[test]
fn cron_instructions_forbid_progress_updates() {
    let cron = crate::CRON_INSTRUCTIONS;

    for needle in ["mcp__right__send_progress", "must not send progress"] {
        assert!(
            cron.contains(needle),
            "CRON_INSTRUCTIONS must mention {needle:?}"
        );
    }
}

#[test]
fn system_prompt_contains_home_dir() {
    let result = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::Openshell,
        "/my/custom/home",
    );
    assert!(
        result.contains("/my/custom/home"),
        "system prompt must contain the passed home_dir"
    );
}

fn attachments_item_schema(schema_json: &str, path: &[&str]) -> serde_json::Value {
    let mut node: serde_json::Value = serde_json::from_str(schema_json).unwrap();
    for key in path {
        node = node
            .get(*key)
            .unwrap_or_else(|| panic!("missing key {key}"))
            .clone();
    }
    node
}

fn assert_has_nullable_media_group_id(items: &serde_json::Value) {
    let props = items.get("properties").expect("items.properties");
    let field = props
        .get("media_group_id")
        .expect("media_group_id property missing");
    let ty = field.get("type").expect("media_group_id.type missing");
    let arr = ty
        .as_array()
        .expect("media_group_id.type must be an array for nullable");
    let kinds: Vec<&str> = arr
        .iter()
        .map(|v| {
            v.as_str()
                .expect("type array element must be a string JSON value")
        })
        .collect();
    assert!(
        kinds.contains(&"string"),
        "must allow string, got {kinds:?}"
    );
    assert!(kinds.contains(&"null"), "must allow null, got {kinds:?}");
}

#[test]
fn reply_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(REPLY_SCHEMA_JSON, &["properties", "attachments", "items"]);
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn bootstrap_schema_attachments_item_has_media_group_id() {
    let items = attachments_item_schema(
        BOOTSTRAP_SCHEMA_JSON,
        &["properties", "attachments", "items"],
    );
    assert_has_nullable_media_group_id(&items);
}

#[test]
fn cron_schema_attachments_item_has_media_group_id() {
    let v: serde_json::Value = serde_json::from_str(CRON_SCHEMA_JSON).unwrap();
    let branches = v["properties"]["delivery"]["oneOf"].as_array().unwrap();
    let notify_branch = branches
        .iter()
        .find(|b| b["properties"]["kind"]["const"] == "notify")
        .expect("notify delivery branch missing");
    assert_has_nullable_media_group_id(&notify_branch["properties"]["attachments"]["items"]);
}

#[test]
fn cron_schema_requires_delivery_and_run_note() {
    let v: serde_json::Value = serde_json::from_str(CRON_SCHEMA_JSON).unwrap();
    let required = v["required"].as_array().unwrap();
    let names: Vec<&str> = required.iter().filter_map(|x| x.as_str()).collect();
    assert!(names.contains(&"delivery"), "delivery must be required");
    assert!(names.contains(&"run_note"), "run_note must be required");
}

#[test]
fn cron_schema_has_notify_and_silent_delivery_branches() {
    let v: serde_json::Value = serde_json::from_str(CRON_SCHEMA_JSON).unwrap();
    let branches = v["properties"]["delivery"]["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2);
    let kinds: Vec<&str> = branches
        .iter()
        .filter_map(|b| b["properties"]["kind"]["const"].as_str())
        .collect();
    assert!(kinds.contains(&"notify"));
    assert!(kinds.contains(&"silent"));

    let notify_branch = branches
        .iter()
        .find(|b| b["properties"]["kind"]["const"] == "notify")
        .expect("notify delivery branch missing");
    let notify_required = notify_branch["required"].as_array().unwrap();
    let notify_required_names: Vec<&str> =
        notify_required.iter().filter_map(|x| x.as_str()).collect();
    assert!(notify_required_names.contains(&"kind"));
    assert!(notify_required_names.contains(&"content"));
    assert_eq!(
        notify_branch["properties"]["content"]["minLength"].as_i64(),
        Some(1)
    );

    let silent_branch = branches
        .iter()
        .find(|b| b["properties"]["kind"]["const"] == "silent")
        .expect("silent delivery branch missing");
    let silent_required = silent_branch["required"].as_array().unwrap();
    let silent_required_names: Vec<&str> =
        silent_required.iter().filter_map(|x| x.as_str()).collect();
    assert!(silent_required_names.contains(&"kind"));
    assert!(silent_required_names.contains(&"reason"));
    assert_eq!(
        silent_branch["properties"]["reason"]["minLength"].as_i64(),
        Some(1)
    );
}

#[test]
fn bg_continuation_schema_requires_notify_delivery_and_run_note() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let required = v["required"].as_array().unwrap();
    let names: Vec<&str> = required.iter().filter_map(|x| x.as_str()).collect();
    assert!(names.contains(&"delivery"), "delivery must be required");
    assert!(names.contains(&"run_note"), "run_note must be required");

    let delivery = &v["properties"]["delivery"];
    assert!(
        delivery.get("oneOf").is_none(),
        "background delivery must not have a silent oneOf branch"
    );
    let kind = v["properties"]["delivery"]["properties"]["kind"]["const"]
        .as_str()
        .unwrap();
    assert_eq!(kind, "notify");

    let min_len = v["properties"]["delivery"]["properties"]["content"]["minLength"]
        .as_i64()
        .unwrap();
    assert_eq!(min_len, 1);
}

#[test]
fn schemas_do_not_use_old_cron_output_names() {
    for schema in [CRON_SCHEMA_JSON, BG_CONTINUATION_SCHEMA_JSON] {
        let v: serde_json::Value = serde_json::from_str(schema).unwrap();
        let props = v["properties"].as_object().unwrap();
        assert!(!props.contains_key("summary"));
        assert!(!props.contains_key("notify"));
        assert!(!props.contains_key("no_notify_reason"));
    }
}

#[test]
fn bg_continuation_schema_attachments_item_has_media_group_id() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let items = &v["properties"]["delivery"]["properties"]["attachments"]["items"];
    assert_has_nullable_media_group_id(items);
}

#[test]
fn operating_instructions_documents_media_groups() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    assert!(ops.contains("Media Groups"), "missing media-group docs");
    assert!(
        ops.contains("media_group_id"),
        "missing media_group_id mention"
    );
    assert!(
        ops.contains("2–10") || ops.contains("2-10"),
        "must mention the 2–10 item limit"
    );
}
