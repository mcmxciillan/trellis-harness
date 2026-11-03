use exo_suit::{
    council::{ActiveRunHandle, SharedAppState, run_council_session},
    models::ChatMessage,
    providers::{build_provider_registry, evaluate_jev_decision_gate, sanitize_prob},
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
