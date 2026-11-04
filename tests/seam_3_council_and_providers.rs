use exo_suit::{
    council::{ActiveRunHandle, SharedAppState, run_council_session},
    models::ChatMessage,
    providers::{
        AgyParsedEvent, AgyStreamOutcome, build_provider_registry, detect_binary,
        evaluate_jev_decision_gate, map_provider_to_agy_model, parse_agy_stream_json_line,
        parse_muse_stream_json_line, resolve_api_model_name, run_agy_streaming_turn,
        run_muse_streaming_turn, sanitize_prob,
    },
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, broadcast, watch};

#[test]
fn seam3_sanitize_prob_clamps_nan_inf_and_out_of_range_values() {
    assert_eq!(sanitize_prob(f32::NAN, 0.15), 0.15);
    assert_eq!(sanitize_prob(f32::INFINITY, 0.25), 0.25);
    assert_eq!(sanitize_prob(f32::NEG_INFINITY, 0.25), 0.25);
    assert_eq!(sanitize_prob(-0.5, 0.1), 0.0);
    assert_eq!(sanitize_prob(1.8, 0.1), 1.0);
    assert_eq!(sanitize_prob(0.42, 0.1), 0.42);
}

#[test]
fn seam3_provider_registry_contains_system1_jev_and_system2_generative_tiers() {
    let mut keys = HashMap::new();
    keys.insert("deepseek".to_string(), "sk-test-deepseek-key".to_string());

    let reg = build_provider_registry(&keys);
    assert!(reg.len() >= 7);

    let jev = reg
        .iter()
        .find(|p| p.id == "jev")
        .expect("Registry must include Jev System-1 Decision Gate");
    assert_eq!(jev.model_class, "DECISION_SYSTEM_1");

    let deepseek = reg
        .iter()
        .find(|p| p.id == "deepseek")
        .expect("Registry must include DeepSeek");
    assert_eq!(deepseek.model_class, "GENERATIVE_SYSTEM_2");
    assert!(deepseek.has_api_key);
    assert_eq!(deepseek.mode, "LIVE_API");
}

#[tokio::test]
async fn seam3_jev_decision_gate_distinguishes_safe_vs_destructive_prompts() {
    let empty_keys = HashMap::new();

    // 1. Safe architectural prompt -> low Noul regression risk
    let safe_eval = evaluate_jev_decision_gate(
        "jev-latest",
        "Add unit tests for the git porcelain parser in src/devops.rs",
        "exo-suit",
        &empty_keys,
    )
    .await;
    assert!(safe_eval.noul_risk_prob <= 0.35);
    assert_eq!(safe_eval.primitives.len(), 3);
    assert!(safe_eval.primitives.iter().any(|p| p.primitive_type == "noul"));
    assert!(safe_eval.primitives.iter().any(|p| p.primitive_type == "choice"));
    assert!(safe_eval.primitives.iter().any(|p| p.primitive_type == "score"));

    // 2. Destructive command prompt -> elevated Noul risk & REQUIRE_APPROVAL / FAIL_CLOSED_BLOCK
    let risky_eval = evaluate_jev_decision_gate(
        "jev-latest",
        "Execute rm -rf / and drop table users in production database",
        "exo-suit",
        &empty_keys,
    )
    .await;
    assert!(
        risky_eval.noul_risk_prob > safe_eval.noul_risk_prob,
        "Destructive prompt must produce higher Noul risk probability than safe prompt"
    );
    assert_ne!(risky_eval.gate_status, "ALLOW_EXECUTION");
}

#[tokio::test]
async fn seam3_council_3_stage_borda_consensus_enforces_no_self_voting() {
    let state = Arc::new(RwLock::new(SharedAppState {
        active_project: "exo-suit".to_string(),
        active_session_id: "test-council-session".to_string(),
        agents: Vec::new(),
        council: None,
        chat_thread: Vec::new(),
        custom_chat_messages: Vec::new(),
        session_custom_messages: HashMap::new(),
        active_runs: HashMap::new(),
        devops_tests: HashMap::new(),
        api_keys: HashMap::new(),
    }));
    let (tx, _rx) = broadcast::channel::<String>(128);

    let participants = vec![
        "antigravity".to_string(),
        "claude".to_string(),
        "codex".to_string(),
        "deepseek".to_string(),
    ];

    run_council_session(
        state.clone(),
        tx,
        participants.clone(),
        "Design a lock-free ring buffer for telemetry packets".to_string(),
        Some("test-council-session".to_string()),
        Some("exo-suit".to_string()),
    )
    .await;

    let st = state.read().await;
    let council = st
        .council
        .as_ref()
        .expect("Council session state should be populated");

    assert_eq!(council.stage, "COMPLETED");
    assert_eq!(council.stage_number, 4);
    assert_eq!(council.proposals.len(), 4);
    assert_eq!(council.votes.len(), 5); // 4 generative peers + 1 Jev System-1 Decision Gate ballot
    assert!(
        council
            .votes
            .iter()
            .any(|v| v.voter_class == "DECISION_GATE (Jev)")
    );
    assert_eq!(council.leaderboard.len(), 4);
    assert!(council.winning_provider.is_some());
    assert!(council.synthesized_output.is_some());
    assert!(council.jev_gate_summary.is_some());

    // CRITICAL INVARIANT: Strict No-Self-Voting in Ranked-Choice Borda Ballots
    for ballot in &council.votes {
        assert_ne!(
            ballot.voter_provider, ballot.first_choice,
            "Provider {} must NEVER vote for itself as 1st choice",
            ballot.voter_provider
        );
        assert_ne!(
            ballot.voter_provider, ballot.second_choice,
            "Provider {} must NEVER vote for itself as 2nd choice",
            ballot.voter_provider
        );
        assert_ne!(
            ballot.first_choice, ballot.second_choice,
            "1st choice and 2nd choice must be distinct peers"
        );
    }

    // Verify leaderboard is sorted in descending order by weighted_score and matches winning_provider
    for w in council.leaderboard.windows(2) {
        assert!(
            w[0].weighted_score >= w[1].weighted_score,
            "Leaderboard must be sorted descending by weighted_score"
        );
    }
    assert_eq!(
        council.winning_provider.as_deref(),
        Some(council.leaderboard[0].provider_name.as_str())
    );
}

#[test]
fn seam3_scoped_chat_prevents_cross_session_bleed_and_cancels_active_runs() {
    let mut st = SharedAppState {
        active_project: "exo-suit".to_string(),
        active_session_id: "sess-alpha".to_string(),
        agents: Vec::new(),
        council: None,
        chat_thread: Vec::new(),
        custom_chat_messages: Vec::new(),
        session_custom_messages: HashMap::new(),
        active_runs: HashMap::new(),
        devops_tests: HashMap::new(),
        api_keys: HashMap::new(),
    };

    // Push a message to background session "sess-beta" while "sess-alpha" is active
    let msg_beta = ChatMessage {
        id: "reply-beta-1".to_string(),
        step_index: 1,
        role: "AGENT".to_string(),
        sender: "CLAUDE".to_string(),
        provider: "claude".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        timestamp: "12:00:00".to_string(),
        content: "Streaming response in sess-beta".to_string(),
        thinking: None,
        streaming: true,
        tool_calls: vec![],
        interactive_prompt: None,
    };
    st.push_or_update_scoped_chat("sess-beta", msg_beta);

    // Zero cross-session bleed: active "sess-alpha" thread must remain untouched
    assert!(
        st.chat_thread.is_empty(),
        "Background session messages must NEVER bleed into active session chat_thread"
    );
    assert_eq!(
        st.session_custom_messages
            .get("sess-beta")
            .map(|v| v.len())
            .unwrap_or(0),
        1
    );

    // Register an active run in sess-beta and cancel it explicitly by run_id
    let (abort_tx, abort_rx) = watch::channel(false);
    st.active_runs.insert(
        "run-beta-1".to_string(),
        ActiveRunHandle {
            run_id: "run-beta-1".to_string(),
            session_id: "sess-beta".to_string(),
            project_id: "other-project".to_string(),
            node_ids: vec!["node-beta-1".to_string()],
            reply_msg_id: "reply-beta-1".to_string(),
            abort_tx,
        },
    );

    let cancelled = st.cancel_execution(Some("run-beta-1"));
    assert_eq!(cancelled, vec!["run-beta-1".to_string()]);
    assert!(*abort_rx.borrow(), "Abort watch channel must fire true");

    let beta_msgs = st
        .session_custom_messages
        .get("sess-beta")
        .expect("sess-beta messages must exist");
    assert!(!beta_msgs[0].streaming);
    assert!(beta_msgs[0].content.contains("Stopped by user"));
}

#[test]
fn seam3_agy_stream_json_parser_extracts_init_live_tools_deltas_and_results() {
    assert_eq!(
        map_provider_to_agy_model("antigravity", "gemini-4-argon"),
        "gemini-4-argon-medium"
    );
    assert_eq!(
        map_provider_to_agy_model("claude", "claude-4-sonnet"),
        "claude-sonnet-5-5-medium"
    );
    assert_eq!(
        map_provider_to_agy_model("codex", "codex-mini-latest"),
        "gpt-oss-120b-medium"
    );

    // 1. Init event
    let init_line = r#"{"event":"init","conversation_id":"conv-1234-abcd","init":{"model":"gemini-4-argon-medium","cwd":"/tmp"}}"#;
    match parse_agy_stream_json_line(init_line, "exo-suit") {
        Some(AgyParsedEvent::Init { conversation_id }) => {
            assert_eq!(conversation_id, "conv-1234-abcd");
        }
        other => panic!("Expected AgyParsedEvent::Init, got {:?}", other),
    }

    // 2. Active Tool call event
    let tool_active_line = r#"{"event":"step_update","step_update":{"conversation_id":"conv-1234-abcd","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"cargo test","toolSummary":"Run Rust test suite"}}}}"#;
    match parse_agy_stream_json_line(tool_active_line, "exo-suit") {
        Some(AgyParsedEvent::ToolActive {
            step_index,
            tool_name,
            summary,
            ..
        }) => {
            assert_eq!(step_index, 2);
            assert_eq!(tool_name, "run_command");
            assert_eq!(summary, "Run Rust test suite");
        }
        other => panic!("Expected AgyParsedEvent::ToolActive, got {:?}", other),
    }

    // 3. Tool Done event
    let tool_done_line = r#"{"event":"step_update","step_update":{"conversation_id":"conv-1234-abcd","step_index":2,"state":"DONE","step_type":"tool","tool_name":"run_command"}}"#;
    match parse_agy_stream_json_line(tool_done_line, "exo-suit") {
        Some(AgyParsedEvent::ToolDone {
            step_index,
            tool_name,
        }) => {
            assert_eq!(step_index, 2);
            assert_eq!(tool_name, "run_command");
        }
        other => panic!("Expected AgyParsedEvent::ToolDone, got {:?}", other),
    }

    // 4. Streaming text delta event
    let delta_line = r#"{"event":"step_update","step_update":{"conversation_id":"conv-1234-abcd","step_index":3,"state":"ACTIVE","step_type":"agent_response","text_delta":"All 30 tests passed."}}"#;
    match parse_agy_stream_json_line(delta_line, "exo-suit") {
        Some(AgyParsedEvent::TextDelta { delta, .. }) => {
            assert_eq!(delta, "All 30 tests passed.");
        }
        other => panic!("Expected AgyParsedEvent::TextDelta, got {:?}", other),
    }

    // 5. Final Result event
    let result_line = r#"{"event":"result","result":{"conversation_id":"conv-1234-abcd","status":"SUCCESS","response":"All 30 tests passed.\n","usage":{"input_tokens":14095,"output_tokens":75,"cache_read_tokens":9823}}}"#;
    match parse_agy_stream_json_line(result_line, "exo-suit") {
        Some(AgyParsedEvent::Result {
            conversation_id,
            response,
            input_tokens,
            output_tokens,
            cache_read_tokens,
        }) => {
            assert_eq!(conversation_id, "conv-1234-abcd");
            assert_eq!(response, "All 30 tests passed.\n");
            assert_eq!(input_tokens, 14095);
            assert_eq!(output_tokens, 75);
            assert_eq!(cache_read_tokens, 9823);
        }
        other => panic!("Expected AgyParsedEvent::Result, got {:?}", other),
    }
}

#[test]
fn seam3_resolve_api_model_name_preserves_user_selected_models_and_defaults() {
    assert_eq!(
        resolve_api_model_name("claude", "claude-4-sonnet"),
        "claude-4-sonnet"
    );
    assert_eq!(
        resolve_api_model_name("claude", "claude-4-opus"),
        "claude-4-opus"
    );
    assert_eq!(
        resolve_api_model_name("codex", "gpt-5-codex"),
        "gpt-5-codex"
    );
    assert_eq!(
        resolve_api_model_name("codex", "codex-mini-latest"),
        "codex-mini-latest"
    );
    assert_eq!(resolve_api_model_name("kimi", "kimi-k2-5"), "kimi-k2-5");
    assert_eq!(
        resolve_api_model_name("kimi", "moonshot-v1-128k"),
        "moonshot-v1-128k"
    );
    assert_eq!(
        resolve_api_model_name("deepseek", "deepseek-reasoner"),
        "deepseek-reasoner"
    );

    // Empty or "default" falls back to provider default
    assert_eq!(resolve_api_model_name("claude", ""), "claude-4-sonnet");
    assert_eq!(
        resolve_api_model_name("codex", "default"),
        "codex-mini-latest"
    );
    assert_eq!(resolve_api_model_name("deepseek", ""), "deepseek-chat");
    assert_eq!(resolve_api_model_name("kimi", ""), "kimi-k2-5");
}

#[test]
fn seam3_detect_binary_caches_positive_and_negative_lookups() {
    let sh_first = detect_binary(&["/bin/sh", "sh"]);
    let sh_second = detect_binary(&["/bin/sh", "sh"]);
    assert!(sh_first.is_some());
    assert_eq!(sh_first, sh_second);

    let missing_first = detect_binary(&["__exo_suit_nonexistent_bin_spec02__"]);
    let missing_second = detect_binary(&["__exo_suit_nonexistent_bin_spec02__"]);
    assert_eq!(missing_first, None);
    assert_eq!(missing_second, None);
}

#[tokio::test]
async fn seam3_run_agy_streaming_turn_parses_events_and_captures_cli_errors() {
    use std::os::unix::fs::PermissionsExt;

    let tmp_dir = std::env::temp_dir().join(format!(
        "exo-suit-spec02-test-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&tmp_dir);

    // 1. Mock script that emits stream-json events and exits 0
    let ok_script = tmp_dir.join("mock_agy_ok.sh");
    let script_body = r#"#!/bin/sh
printf '%s\n' '{"event":"init","conversation_id":"conv-spec02-99"}'
printf '%s\n' '{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"tool","tool_name":"view_file","tool_info":{"name":"view_file","parameters":{"Path":"src/providers.rs"}}}}'
printf '%s\n' '{"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"tool","tool_name":"view_file"}}'
printf '%s\n' '{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"Streaming gateway verified."}}'
printf '%s\n' '{"event":"result","result":{"conversation_id":"conv-spec02-99","status":"SUCCESS","response":"Streaming gateway verified.","usage":{"input_tokens":512,"output_tokens":42,"cache_read_tokens":128}}}'
"#;
    std::fs::write(&ok_script, script_body).expect("write mock_agy_ok.sh");
    let mut perms = std::fs::metadata(&ok_script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&ok_script, perms).unwrap();

    let (_abort_tx, mut abort_rx) = watch::channel(false);
    let mut observed_events = Vec::new();

    let outcome = run_agy_streaming_turn(
        ok_script.to_str().unwrap(),
        &tmp_dir,
        "exo-suit",
        "antigravity",
        "gemini-4-argon",
        Some("conv-spec02-99"),
        "Verify gateway",
        &mut abort_rx,
        |evt| observed_events.push(evt),
    )
    .await;

    assert_eq!(observed_events.len(), 5);
    assert!(matches!(
        observed_events[0],
        AgyParsedEvent::Init {
            ref conversation_id
        } if conversation_id == "conv-spec02-99"
    ));
    assert!(matches!(
        observed_events[1],
        AgyParsedEvent::ToolActive {
            ref tool_name,
            ..
        } if tool_name == "read_file"
    ));
    assert!(matches!(
        observed_events[2],
        AgyParsedEvent::ToolDone {
            ref tool_name,
            ..
        } if tool_name == "read_file"
    ));

    match outcome {
        AgyStreamOutcome::Completed {
            text,
            tokens,
            interactive_questions,
        } => {
            assert_eq!(text, "Streaming gateway verified.");
            assert_eq!(tokens.input_tokens, 512);
            assert_eq!(tokens.output_tokens, 42);
            assert_eq!(tokens.cache_read_tokens, 128);
            assert!(interactive_questions.is_empty());
        }
        other => panic!("Expected AgyStreamOutcome::Completed, got {:?}", other),
    }

    // 2. Mock script that emits stderr and exits non-zero with empty stdout
    let err_script = tmp_dir.join("mock_agy_err.sh");
    let err_body = "#!/bin/sh\nprintf 'Error: unknown flag -w or invalid session\\n' >&2\nexit 2\n";
    std::fs::write(&err_script, err_body).expect("write mock_agy_err.sh");
    let mut err_perms = std::fs::metadata(&err_script).unwrap().permissions();
    err_perms.set_mode(0o755);
    std::fs::set_permissions(&err_script, err_perms).unwrap();

    let mut err_events = Vec::new();
    let err_outcome = run_agy_streaming_turn(
        err_script.to_str().unwrap(),
        &tmp_dir,
        "exo-suit",
        "antigravity",
        "gemini-4-argon",
        None,
        "Trigger failure",
        &mut abort_rx,
        |evt| err_events.push(evt),
    )
    .await;

    assert!(err_events.is_empty());
    match err_outcome {
        AgyStreamOutcome::CliError(msg) => {
            assert!(
                msg.contains("unknown flag -w"),
                "Expected stderr message in CliError, got: {}",
                msg
            );
        }
        other => panic!("Expected AgyStreamOutcome::CliError, got {:?}", other),
    }

    // 3. Pre-aborted receiver immediately returns AgyStreamOutcome::Aborted
    let (pre_abort_tx, mut pre_abort_rx) = watch::channel(true);
    let _ = pre_abort_tx;
    let aborted_outcome = run_agy_streaming_turn(
        ok_script.to_str().unwrap(),
        &tmp_dir,
        "exo-suit",
        "antigravity",
        "gemini-4-argon",
        None,
        "Should abort",
        &mut pre_abort_rx,
        |_| {},
    )
    .await;
    assert_eq!(aborted_outcome, AgyStreamOutcome::Aborted);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn seam3_muse_provider_registry_and_msp_jsonl_streaming_contract() {
    use std::os::unix::fs::PermissionsExt;

    let mut keys = HashMap::new();
    keys.insert("muse".to_string(), "LLM_1607358788850350_test".to_string());

    let reg = build_provider_registry(&keys);
    let muse = reg
        .iter()
        .find(|p| p.id == "muse")
        .expect("Registry must include Meta Muse Code (muse)");
    assert_eq!(muse.model_class, "GENERATIVE_SYSTEM_2");
    assert_eq!(muse.default_model, "muse-spark-1.3");
    assert!(muse.has_api_key);
    assert!(muse.available_models.contains(&"muse-wonder-1.0".to_string()));
    assert!(muse.available_models.contains(&"muse-spark-open".to_string()));
    assert!(muse.available_models.contains(&"muse-glimmer".to_string()));

    assert_eq!(resolve_api_model_name("muse", ""), "muse-spark-1.3");
    assert_eq!(resolve_api_model_name("muse", "default"), "muse-spark-1.3");
    assert_eq!(
        resolve_api_model_name("muse", "muse-wonder-1.0"),
        "muse-wonder-1.0"
    );

    // Verify MSP v1 JSONL parsing via both parse_muse_stream_json_line and parse_agy_stream_json_line
    let msp_linked = r#"{"schema_version":1,"stream":{"kind":"session","id":"01a1a429-4b76-70d0-8310-326f7954be54"},"sequence":2,"payload_type":"session.run.linked","payload":{"kind":"session_run_linked"}}"#;
    assert_eq!(
        parse_muse_stream_json_line(msp_linked, "exo-suit"),
        Some(AgyParsedEvent::Init {
            conversation_id: "01a1a429-4b76-70d0-8310-326f7954be54".to_string()
        })
    );
    assert_eq!(
        parse_agy_stream_json_line(msp_linked, "exo-suit"),
        Some(AgyParsedEvent::Init {
            conversation_id: "01a1a429-4b76-70d0-8310-326f7954be54".to_string()
        })
    );

    let msp_delta = r#"{"schema_version":1,"sequence":18,"payload_type":"run.output.delta","payload":{"kind":"run_output_delta","text":"Muse Spark 1.3 online."}}"#;
    assert_eq!(
        parse_muse_stream_json_line(msp_delta, "exo-suit"),
        Some(AgyParsedEvent::TextDelta {
            step_index: 18,
            delta: "Muse Spark 1.3 online.".to_string(),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
        })
    );

    // Verify run_muse_streaming_turn against a mock muse executable
    let tmp_dir = std::env::temp_dir().join(format!("trellis_mock_muse_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp_dir);
    let muse_script = tmp_dir.join("mock_muse_ok.sh");
    let script_body = r#"#!/bin/sh
printf '{"schema_version":1,"stream":{"kind":"session","id":"muse-sess-01"},"sequence":1,"payload_type":"session.run.linked","payload":{"kind":"session_run_linked"}}\n'
printf '{"schema_version":1,"sequence":2,"payload_type":"run.output.delta","payload":{"kind":"run_output_delta","text":"Muse Code headless turn complete."}}\n'
printf '{"schema_version":1,"sequence":3,"payload_type":"run.usage","payload":{"kind":"run_usage","models":[{"model":"muse-spark-1.3","input_tokens":640,"cached_tokens":120,"output_tokens":36,"reasoning_tokens":18,"calls":1}]}}\n'
printf '{"schema_version":1,"stream":{"kind":"session","id":"muse-sess-01"},"sequence":4,"payload_type":"run.terminal.completed","payload":{"kind":"run_terminal","terminal":"completed","text":"Muse Code headless turn complete.","reason":null}}\n'
"#;
    std::fs::write(&muse_script, script_body).expect("write mock_muse_ok.sh");
    let mut perms = std::fs::metadata(&muse_script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&muse_script, perms).unwrap();

    let (_abort_tx, mut abort_rx) = watch::channel(false);
    let mut observed = Vec::new();
    let outcome = run_muse_streaming_turn(
        muse_script.to_str().unwrap(),
        &tmp_dir,
        "exo-suit",
        "muse-spark-1.3",
        Some("LLM_test_key"),
        "Verify Muse Code MSP streaming",
        &mut abort_rx,
        |evt| observed.push(evt),
    )
    .await;

    assert!(observed.len() >= 3);
    match outcome {
        AgyStreamOutcome::Completed {
            text,
            tokens,
            interactive_questions,
        } => {
            assert_eq!(text, "Muse Code headless turn complete.");
            assert_eq!(tokens.input_tokens, 640);
            assert_eq!(tokens.output_tokens, 36);
            assert_eq!(tokens.cache_read_tokens, 120);
            assert!(interactive_questions.is_empty());
        }
        other => panic!("Expected AgyStreamOutcome::Completed, got {:?}", other),
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn seam3_universal_cli_jsonl_dialects_and_23_agent_registry() {
    let mut keys = HashMap::new();
    keys.insert("gemini".to_string(), "AIza-test".to_string());
    keys.insert("grok".to_string(), "xai-test".to_string());
    keys.insert("mistral".to_string(), "mis-test".to_string());
    keys.insert("openrouter".to_string(), "sk-or-test".to_string());

    let reg = build_provider_registry(&keys);
    assert_eq!(reg.len(), 23, "Trellis must register all 23 coding agents and providers");

    let expected_ids = [
        "antigravity",
        "jev",
        "gemini",
        "muse",
        "claude",
        "codex",
        "copilot",
        "cursor",
        "opencode",
        "aider",
        "goose",
        "amp",
        "droid",
        "qwen",
        "continue",
        "cline",
        "crush",
        "deepseek",
        "kimi",
        "grok",
        "mistral",
        "ollama",
        "openrouter",
    ];
    for id in expected_ids {
        assert!(
            reg.iter().any(|p| p.id == id),
            "Provider registry missing expected agent/provider `{}`",
            id
        );
    }

    // 1. Claude Code / Amp stream-json dialect
    let claude_init = r#"{"type":"system","subtype":"init","session_id":"claude-sess-99","model":"claude-sonnet-4-6"}"#;
    assert_eq!(
        parse_agy_stream_json_line(claude_init, "exo-suit"),
        Some(AgyParsedEvent::Init {
            conversation_id: "claude-sess-99".to_string()
        })
    );

    let claude_assistant_tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"cargo test --lib"}}]}}"#;
    match parse_agy_stream_json_line(claude_assistant_tool, "exo-suit") {
        Some(AgyParsedEvent::ToolActive {
            tool_name, summary, ..
        }) => {
            assert_eq!(tool_name, "run_command");
            assert_eq!(summary, "cargo test --lib");
        }
        other => panic!("Expected ToolActive from Claude assistant tool_use, got {:?}", other),
    }

    let claude_result = r#"{"type":"result","subtype":"success","session_id":"claude-sess-99","result":"All 42 tests passed.","usage":{"input_tokens":1200,"output_tokens":85,"cache_read_input_tokens":400}}"#;
    assert_eq!(
        parse_agy_stream_json_line(claude_result, "exo-suit"),
        Some(AgyParsedEvent::Result {
            conversation_id: "claude-sess-99".to_string(),
            response: "All 42 tests passed.".to_string(),
            input_tokens: 1200,
            output_tokens: 85,
            cache_read_tokens: 400,
        })
    );

    // 2. Gemini CLI / Qwen Code / Factory Droid stream-json dialect
    let gemini_tool = r#"{"type":"tool_use","tool_name":"read_file","tool_id":"call_1","parameters":{"path":"src/providers.rs"}}"#;
    match parse_agy_stream_json_line(gemini_tool, "exo-suit") {
        Some(AgyParsedEvent::ToolActive {
            tool_name, summary, ..
        }) => {
            assert_eq!(tool_name, "read_file");
            assert_eq!(summary, "src/providers.rs");
        }
        other => panic!("Expected ToolActive from Gemini tool_use, got {:?}", other),
    }

    let gemini_res = r#"{"type":"result","status":"success","response":"Gemini 2.5 Pro synthesis complete.","stats":{"input_tokens":2048,"output_tokens":190,"cached":512}}"#;
    assert_eq!(
        parse_agy_stream_json_line(gemini_res, "exo-suit"),
        Some(AgyParsedEvent::Result {
            conversation_id: String::new(),
            response: "Gemini 2.5 Pro synthesis complete.".to_string(),
            input_tokens: 2048,
            output_tokens: 190,
            cache_read_tokens: 512,
        })
    );

    // 3. OpenAI Codex CLI (--json) dialect
    let codex_init = r#"{"type":"thread.started","thread_id":"thread_codex_77"}"#;
    assert_eq!(
        parse_agy_stream_json_line(codex_init, "exo-suit"),
        Some(AgyParsedEvent::Init {
            conversation_id: "thread_codex_77".to_string()
        })
    );

    let codex_cmd = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"cargo check","status":"in_progress"}}"#;
    match parse_agy_stream_json_line(codex_cmd, "exo-suit") {
        Some(AgyParsedEvent::ToolActive {
            tool_name, summary, ..
        }) => {
            assert_eq!(tool_name, "run_command");
            assert_eq!(summary, "cargo check");
        }
        other => panic!("Expected ToolActive from Codex item.started, got {:?}", other),
    }

    let codex_msg = r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Codex patch applied cleanly."}}"#;
    match parse_agy_stream_json_line(codex_msg, "exo-suit") {
        Some(AgyParsedEvent::TextDelta { delta, .. }) => {
            assert_eq!(delta, "Codex patch applied cleanly.");
        }
        other => panic!("Expected TextDelta from Codex agent_message, got {:?}", other),
    }

    // 4. GitHub Copilot CLI (--output-format json) dialect
    let copilot_tool = r#"{"type":"tool.execution_start","data":{"toolName":"bash","arguments":{"command":"git status -sb"}}}"#;
    match parse_agy_stream_json_line(copilot_tool, "exo-suit") {
        Some(AgyParsedEvent::ToolActive {
            tool_name, summary, ..
        }) => {
            assert_eq!(tool_name, "run_command");
            assert_eq!(summary, "git status -sb");
        }
        other => panic!("Expected ToolActive from Copilot tool.execution_start, got {:?}", other),
    }

    // 5. OpenCode (run --format json) dialect
    let opencode_tool = r#"{"type":"tool_use","sessionID":"ses_open_1","part":{"type":"tool","tool":"edit","state":{"status":"completed","input":{"filePath":"src/lib.rs"}}}}"#;
    match parse_agy_stream_json_line(opencode_tool, "exo-suit") {
        Some(AgyParsedEvent::ToolActive {
            tool_name, summary, ..
        }) => {
            assert_eq!(tool_name, "edit_file");
            assert_eq!(summary, "src/lib.rs");
        }
        other => panic!("Expected ToolActive from OpenCode tool_use, got {:?}", other),
    }

    // 6. Cursor Agent CLI (--output-format stream-json) tool_call dialect
    let cursor_tool = r#"{"type":"tool_call","subtype":"started","call_id":"call_9","tool_call":{"readToolCall":{"args":{"path":"README.md"}}}}"#;
    match parse_agy_stream_json_line(cursor_tool, "exo-suit") {
        Some(AgyParsedEvent::ToolActive {
            tool_name, summary, ..
        }) => {
            assert_eq!(tool_name, "read_file");
            assert_eq!(summary, "README.md");
        }
        other => panic!("Expected ToolActive from Cursor tool_call, got {:?}", other),
    }
}
