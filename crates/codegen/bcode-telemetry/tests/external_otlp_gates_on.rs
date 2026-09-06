//! Wire test for the external OTEL stream with **both content gates ON**, the higher-risk privacy path.
//! Here prompt text and tool parameters actually leave the process.
//! Asserts against an in-process OTLP collector that:
//!
//! - gated content (`prompt`, `tool_parameters`, `file_path`, verbatim `tool_name`/`mcp_server.name`) IS present when the gate is on,
//! - planted secret shapes are STILL scrubbed inside that gated content (gates loosen *which fields* export, never the secret scrub),
//! - identity attributes ride every record and metric once set,
//! - `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE=cumulative` and `OTEL_METRICS_INCLUDE_VERSION=1` take effect on the wire,
//! - the remote fleet kill switch stops emission in-process.
//!
//! Everything runs in one sequential `#[test]`: the `EXTERNAL` registry is a process-global `OnceLock`.
//! Each init-config scenario is its own test binary.

mod otlp_collector;

use bcode_telemetry::external::{self, ExternalOtelRemotePolicy, IdentityAttrs};
use otlp_collector as col;

// Secret shapes that MUST be scrubbed everywhere, even inside gated content
const SECRET_KEY: &str = "sk-LEAKaaaaaaaaaaaaaaaa1234567890";
const SECRET_MODEL: &str = "bcode-4-sk-LEAKmodel1234567890abcd";
// Benign markers: with the gate ON these MUST appear on the wire (proving the gated field is actually exported, not just that the scrub ran)
const PROMPT_MARK: &str = "promptbodymarker";
const PARAM_MARK: &str = "parammarker";
const LONG_CMD_MARK: &str = "longcmdmarker";
const DENY_CMD_MARK: &str = "denycmdmarker";
const RESPONSE_MARK: &str = "assistantresponsemarker";
const OAUTH_EMAIL: &str = "otel.parity.on@example.com";
const CLIENT_VERSION: &str = "9.9.9-cv";

#[test]
fn external_stream_gates_on_end_to_end() {
    let collected = col::Collected::default();
    let endpoint = col::start_collector(collected.clone());

    let mut cfg = external::ExternalOtelConfig::resolve_with(
        |name| match name {
            "BCODE_EXTERNAL_OTEL" => Some("1".into()),
            "OTEL_LOGS_EXPORTER" | "OTEL_METRICS_EXPORTER" => Some("otlp".into()),
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some(endpoint.clone()),
            // Both content gates ON.
            "OTEL_LOG_USER_PROMPTS"
            | "OTEL_LOG_TOOL_DETAILS"
            | "OTEL_LOG_ASSISTANT_RESPONSES"
            | "OTEL_LOG_TOOL_CONTENT" => Some("1".into()),
            "OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE" => Some("cumulative".into()),
            "OTEL_METRICS_INCLUDE_VERSION" => Some("1".into()),
            "OTEL_METRIC_EXPORT_INTERVAL" => Some("200".into()),
            "OTEL_BLRP_SCHEDULE_DELAY" => Some("100".into()),
            _ => None,
        },
        None,
    )
    .expect("double opt-in must resolve");
    assert!(cfg.gates.log_user_prompts && cfg.gates.log_tool_details);
    assert!(cfg.gates.log_tool_content, "content gate must be on");
    assert!(
        cfg.gates.log_assistant_responses,
        "assistant gate must be on in this binary"
    );
    cfg.client = external::config::ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: CLIENT_VERSION.into(),
        app_entrypoint: "cli".into(),
    };

    external::init(Some(cfg));
    assert!(external::is_active(), "gates-on config must activate");

    // Identity attrs (plain ids, never tokens) ride every record and metric
    external::set_identity(IdentityAttrs {
        user_id: Some("user-x".into()),
        email: Some(OAUTH_EMAIL.into()),
        organization_id: Some("org-acme".into()),
        team_id: Some("team-7".into()),
        deployment_id: Some("deploy-eu".into()),
    });

    // Product events stay disabled; the external sink must still emit through the real funnel
    assert!(!bcode_telemetry::is_enabled());

    bcode_telemetry::log_event(bcode_telemetry::events::SessionHarness {
        session_id: "sess-gates-on".into(),
        client_identifier: Some("bcode-pager".into()),
        model_id: "bcode-4".into(),
        agent_name: "bcode-plan".into(),
        permission_mode: bcode_telemetry::enums::PermissionMode::Ask,
        mcp_server_names: vec!["internal-mcp".into()],
        plugin_names: vec![],
        skill_names: vec![],
        lsp_server_names: vec![],
        hook_names: vec![],
        agents_md_dir_names: vec![],
        memory_enabled: false,
        memory_retrieval_mode: bcode_telemetry::events::MemoryRetrievalMode::Disabled,
        is_git_repo: true,
        auto_update: None,
    });
    bcode_telemetry::log_event(bcode_telemetry::events::PromptSubmitted {
        prompt_length: 100,
        model_id: "bcode-4".into(),
        client_identifier: None,
        screen_mode: None,
        prompt_text: Some(format!("refactor {PROMPT_MARK} with key {SECRET_KEY} now")),
        command_name: Some("compact".into()),
    });
    bcode_telemetry::log_event(bcode_telemetry::events::ModelResponseReceived {
        model_id: SECRET_MODEL.into(),
        duration_ms: 5,
        stop_reason: Some("stop".into()),
        prompt_tokens: Some(11),
        completion_tokens: Some(7),
        reasoning_tokens: Some(3),
        cached_prompt_tokens: Some(9),
        cache_creation_tokens: None,
        cost_usd_ticks: None,
    });
    bcode_telemetry::log_event(bcode_telemetry::events::ToolCallCompleted {
        tool_name: "github__create_issue".into(),
        outcome: bcode_session_events::types::ToolOutcome::Success,
        hook_rewrote: false,
        duration_ms: 12,
        tool_result_size_bytes: None,
        file_path: Some("/tmp/projectdir/config.toml".into()),
        parameters: Some(serde_json::json!({
            "marker": PARAM_MARK,
            "token": SECRET_KEY,
            "deep": {"a": {"b": "c"}},
        })),
        tool_use_id: Some("call-github".into()),
        tool_output: Some(format!("ok {PARAM_MARK}")),
        error_message: None,
    });
    let long_command = format!("{LONG_CMD_MARK}{}", "x".repeat(600));
    bcode_telemetry::log_event(bcode_telemetry::events::ToolCallCompleted {
        tool_name: "run_terminal_cmd".into(),
        outcome: bcode_session_events::types::ToolOutcome::Success,
        hook_rewrote: false,
        duration_ms: 8,
        tool_result_size_bytes: None,
        file_path: None,
        parameters: Some(serde_json::json!({ "command": long_command })),
        tool_use_id: Some("call-bash-long".into()),
        tool_output: None,
        error_message: None,
    });
    bcode_telemetry::log_event(bcode_telemetry::events::PermissionDecisionRecord {
        payload: bcode_telemetry::events::PermissionDecisionPayload {
            tool_name: "run_terminal_cmd".into(),
            access_kind: bcode_telemetry::events::AccessKind::Bash,
            decision: bcode_telemetry::events::PermissionOutcome::Deny,
            wait_ms: 10,
            permission_mode: bcode_telemetry::enums::PermissionMode::Ask,
            source: Some("user_reject".into()),
            subagent_session_id: None,
            subagent_type: None,
            manager_prompt_attempted: None,
            prompt_outcome: None,
            prompt_outcome_detail: None,
            remember_tool_approvals: None,
            decision_reason: None,
            classifier_source: None,
            classifier_verdict: None,
            security_findings: None,
            classifier_latency_ms: None,
            auto_denials_consecutive: None,
            auto_denials_total: None,
        },
        tool_input: bcode_telemetry::events::ExternalToolInput {
            parameters: Some(serde_json::json!({ "command": DENY_CMD_MARK })),
            tool_use_id: Some("call-deny-1".into()),
        },
    });
    bcode_telemetry::external::emit(&bcode_telemetry::events::AssistantResponse {
        response_length: RESPONSE_MARK.len(),
        response_text: Some(RESPONSE_MARK.into()),
    });

    external::flush();
    assert!(
        col::wait_until(std::time::Duration::from_secs(10), || {
            !collected.logs.lock().unwrap().is_empty()
                && !collected.metrics.lock().unwrap().is_empty()
        }),
        "collector must receive both signals"
    );

    // ── Resource + scope ────────────────────────────────────────────────
    let records = col::log_records(&collected);
    let harness = col::find_event(&collected, "bcode_code.session_start")
        .expect("session_start must be present");
    assert_eq!(harness.scope_name, "ai.bcode.bcode_code");
    assert_eq!(
        harness
            .resource
            .get("service.name")
            .and_then(|v| v.as_str()),
        Some("bcode-cli"),
        "service.name=bcode-cli is a wire commitment"
    );
    assert_eq!(
        harness
            .resource
            .get("bcode_code.schema.version")
            .and_then(|v| v.as_str()),
        Some("v1")
    );
    // External records carry no free-text body.
    assert!(
        records.iter().all(|r| !r.has_body),
        "no record may carry a body"
    );

    // ── Identity attrs on a record ──────────────────────────────────────
    assert_eq!(
        harness.attrs.get("user.id").and_then(|v| v.as_str()),
        Some("user-x")
    );
    assert_eq!(
        harness.attrs.get("user.email").and_then(|v| v.as_str()),
        Some(OAUTH_EMAIL)
    );
    assert_eq!(
        harness
            .attrs
            .get("organization.id")
            .and_then(|v| v.as_str()),
        Some("org-acme")
    );
    assert_eq!(
        harness.attrs.get("team.id").and_then(|v| v.as_str()),
        Some("team-7")
    );
    assert_eq!(
        harness.attrs.get("deployment.id").and_then(|v| v.as_str()),
        Some("deploy-eu")
    );

    // ── Prompt gate ON: text present, secret still scrubbed ─────────────
    let prompt =
        col::find_event(&collected, "bcode_code.user_prompt").expect("user_prompt present");
    let prompt_text = prompt
        .attrs
        .get("prompt")
        .and_then(|v| v.as_str())
        .expect("prompt attr present when OTEL_LOG_USER_PROMPTS=1");
    assert!(
        prompt_text.contains(PROMPT_MARK),
        "gated prompt body must export: {prompt_text:?}"
    );
    assert!(
        !prompt_text.contains(SECRET_KEY),
        "secret survived in prompt: {prompt_text:?}"
    );
    assert_eq!(
        prompt.attrs.get("command_name").and_then(|v| v.as_str()),
        Some("compact")
    );

    // ── Tool details gate ON: verbatim name + gated path/params, scrubbed ─
    let tool = col::find_event(&collected, "bcode_code.tool_result").expect("tool_result present");
    assert_eq!(
        tool.attrs.get("tool_name").and_then(|v| v.as_str()),
        Some("github__create_issue"),
        "details gate exposes the verbatim tool name"
    );
    assert_eq!(
        tool.attrs.get("mcp_tool.name").and_then(|v| v.as_str()),
        Some("create_issue")
    );
    assert_eq!(
        tool.attrs.get("mcp_server.name").and_then(|v| v.as_str()),
        Some("github")
    );
    assert_eq!(
        tool.attrs.get("file_extension").and_then(|v| v.as_str()),
        Some("toml"),
        "file_extension always exported"
    );
    assert!(
        tool.attrs.contains_key("file_path"),
        "full path exported under details gate"
    );
    let params = tool
        .attrs
        .get("tool_parameters")
        .and_then(|v| v.as_str())
        .expect("tool_parameters present under details gate");
    assert!(
        params.contains(PARAM_MARK),
        "gated params must export: {params:?}"
    );
    assert!(
        !params.contains(SECRET_KEY),
        "secret survived in params: {params:?}"
    );
    let tool_input = tool
        .attrs
        .get("tool_input")
        .and_then(|v| v.as_str())
        .expect("tool_input present under content gate");
    assert!(
        tool_input.contains(PARAM_MARK),
        "full tool_input must export: {tool_input:?}"
    );
    let expected_output = format!("ok {PARAM_MARK}");
    assert_eq!(
        tool.attrs.get("tool_output").and_then(|v| v.as_str()),
        Some(expected_output.as_str())
    );

    let bash = col::log_records(&collected)
        .into_iter()
        .find(|r| {
            r.event_name == "bcode_code.tool_result"
                && r.attrs.get("tool_use_id").and_then(|v| v.as_str()) == Some("call-bash-long")
        })
        .expect("long-command tool_result");
    let full_command = bash
        .attrs
        .get("full_command")
        .and_then(|v| v.as_str())
        .expect("full_command under content gate");
    assert!(
        full_command.starts_with(LONG_CMD_MARK),
        "full_command must keep the marker: {full_command:?}"
    );
    assert_eq!(
        full_command.len(),
        LONG_CMD_MARK.len() + 600,
        "full_command must not 512→128 collapse"
    );
    assert!(
        !full_command.contains("…[truncated]"),
        "full_command must not collapse: {full_command:?}"
    );

    let decision = col::find_event(&collected, "bcode_code.tool_decision").expect("tool_decision");
    assert_eq!(
        decision.attrs.get("decision").and_then(|v| v.as_str()),
        Some("deny")
    );
    assert_eq!(
        decision.attrs.get("tool_use_id").and_then(|v| v.as_str()),
        Some("call-deny-1")
    );
    let deny_params = decision
        .attrs
        .get("tool_parameters")
        .and_then(|v| v.as_str())
        .expect("deny tool_decision exports params under details gate");
    assert!(
        deny_params.contains(DENY_CMD_MARK),
        "deny params must export: {deny_params:?}"
    );
    assert_eq!(
        decision.attrs.get("full_command").and_then(|v| v.as_str()),
        Some(DENY_CMD_MARK)
    );

    let assistant = col::find_event(&collected, "bcode_code.assistant_response")
        .expect("assistant_response present");
    assert!(!assistant.has_body, "no record may carry a body");
    let response = assistant
        .attrs
        .get("response")
        .and_then(|v| v.as_str())
        .expect("gated response present");
    assert!(
        response.contains(RESPONSE_MARK),
        "gated response must export: {response:?}"
    );
    let response_length = assistant
        .attrs
        .get("response_length")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            assistant
                .attrs
                .get("response_length")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
        })
        .expect("response_length always-on");
    assert_eq!(response_length as usize, RESPONSE_MARK.len());

    // ── Metrics: cumulative temporality + app.version + scrubbed model ──
    let tokens = col::find_metric(&collected, "bcode_code.token.usage");
    assert!(!tokens.is_empty(), "token.usage must export");
    for p in &tokens {
        assert_eq!(
            p.temporality,
            col::TEMPORALITY_CUMULATIVE,
            "cumulative requested"
        );
        assert_eq!(
            p.attrs.get("app.version").and_then(|v| v.as_str()),
            Some(CLIENT_VERSION),
            "OTEL_METRICS_INCLUDE_VERSION=1 attaches app.version"
        );
        assert_eq!(
            p.attrs.get("user.id").and_then(|v| v.as_str()),
            Some("user-x")
        );
        assert_eq!(
            p.attrs.get("user.email").and_then(|v| v.as_str()),
            Some(OAUTH_EMAIL)
        );
        let model = p.attrs.get("model").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            !model.contains("sk-LEAKmodel"),
            "metric model must be scrubbed: {model:?}"
        );
    }
    let sessions = col::find_metric(&collected, "bcode_code.session.count");
    // SessionHarness has no session.count metric; that comes from SessionNew, which this test never emits
    // The token.usage checks above already cover metric identity
    let _ = sessions;

    // ── Canary scan at the raw HTTP layer (both signals) ────────────────
    let raw = collected.raw_text();
    assert!(!raw.contains(SECRET_KEY), "secret key reached the wire");
    assert!(
        !raw.contains("sk-LEAKmodel"),
        "secret model shape reached the wire"
    );

    // ── Remote fleet kill switch stops emission in-process ──────────────
    external::flush();
    col::wait_until(std::time::Duration::from_millis(500), || false);
    let logs_before = collected.logs_len();
    external::apply_remote_policy(ExternalOtelRemotePolicy {
        force_disable: true,
        lock_content_gates: false,
    });
    assert!(
        !external::is_active(),
        "kill switch must clear the emission gate"
    );
    bcode_telemetry::log_event(bcode_telemetry::events::PromptSubmitted {
        prompt_length: 1,
        model_id: "bcode-4".into(),
        client_identifier: None,
        screen_mode: None,
        prompt_text: Some("post-kill".into()),
        command_name: None,
    });
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(
        collected.logs_len(),
        logs_before,
        "no exports after the remote kill switch"
    );

    external::shutdown();
}
