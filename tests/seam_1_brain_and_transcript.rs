use exo_suit::brain::{
     append_turn_to_session_transcript, atomic_write_string, brain_dir, build_live_session_graph,
    classify_file_kind, clean_ansi_and_tags, get_session_artifacts, get_session_chat_thread,
    parse_ask_question_args, read_bounded_file, reconcile_interactive_prompt,
    resolve_sandboxed_file_path, save_session_agy_conversation_id,
    save_session_interactive_answer,
};
use exo_suit::models::{InteractiveQuestionItem, TokenMetrics};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TempBrainGuard {
    _lock: MutexGuard<'static, ()>,
    root: PathBuf,
}

impl TempBrainGuard {
    fn new(label: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "exo_suit_seam1_{}_{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        std::env::set_var("EXO_BRAIN_DIR", &root);
        Self { _lock: lock, root }
    }
}

impl Drop for TempBrainGuard {
    fn drop(&mut self) {
        std::env::remove_var("EXO_BRAIN_DIR");
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn seam1_clean_ansi_and_extract_user_request_tags() {
    let raw = "\x1b[32mHeader\x1b[0m\n<USER_REQUEST>\nFix the subagent graph nodes\n</USER_REQUEST>\nFooter";
    assert_eq!(clean_ansi_and_tags(raw), "Fix the subagent graph nodes");

    let plain_ansi = "\x1b[1;31mERROR:\x1b[0m compilation failed";
    assert_eq!(clean_ansi_and_tags(plain_ansi), "ERROR: compilation failed");
}

#[test]
fn seam1_classify_file_kind_recognizes_markdown_images_json_and_code() {
    assert_eq!(classify_file_kind("ARCHITECTURE-SPEC.md"), "markdown");
    assert_eq!(classify_file_kind("trellis-espalier.jpg"), "image");
    assert_eq!(classify_file_kind("diagram.svg"), "image");
    assert_eq!(classify_file_kind("transcript.jsonl"), "json");
    assert_eq!(classify_file_kind("devops.rs"), "code");
    assert_eq!(classify_file_kind("app.js"), "code");
    assert_eq!(classify_file_kind("LICENSE"), "text");
}

#[test]
fn seam1_parse_ask_question_args_handles_structured_and_stringified_arrays() {
    let direct_args = json!({
        "questions": [
            {
                "question": "Which architecture should we use?",
                "options": ["(Recommended) Deep Modules", "Shallow Adapters"],
                "is_multi_select": false
            },
            {
                "question": "Select target platforms",
                "options": ["Desktop", "Mobile PWA"],
                "is_multi_select": true
            }
        ]
    });
    let parsed = parse_ask_question_args(Some(&direct_args));
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].index, 1);
    assert_eq!(parsed[0].question, "Which architecture should we use?");
    assert!(!parsed[0].is_multi_select);
    assert_eq!(parsed[1].index, 2);
    assert!(parsed[1].is_multi_select);

    // Stringified JSON array inside `questions`
    let stringified_args = json!({
        "questions": "[{\"question\":\"Confirm cutover?\",\"options\":[\"Yes\",\"No\"],\"is_multi_select\":false}]"
    });
    let parsed_str = parse_ask_question_args(Some(&stringified_args));
    assert_eq!(parsed_str.len(), 1);
    assert_eq!(parsed_str[0].question, "Confirm cutover?");
}

#[test]
fn seam1_sandboxed_file_path_allows_workspace_and_blocks_outside_files() {
    let guard = TempBrainGuard::new("sandbox");

    // Allowed: file inside ~/code/exo-suit
    let allowed = resolve_sandboxed_file_path("Cargo.toml", "", "exo-suit");
    assert!(allowed.is_some(), "Cargo.toml inside exo-suit should resolve");

    let bounded = read_bounded_file("Cargo.toml", "", "exo-suit")
        .expect("read_bounded_file should read Cargo.toml");
    assert_eq!(bounded.name, "Cargo.toml");
    assert_eq!(bounded.file_kind, "code");
    assert!(bounded.content.contains("exo-suit"));

    // Blocked: system file outside ~/code and brain_dir
    assert!(resolve_sandboxed_file_path("/etc/passwd", "", "exo-suit").is_none());
    assert!(resolve_sandboxed_file_path("../../../../etc/hosts", "", "exo-suit").is_none());
    assert!(exo_suit::brain::get_session_artifacts("../escape").is_empty());
    assert!(!exo_suit::brain::approve_artifact_metadata("../escape", "plan.md"));
    assert!(!exo_suit::brain::approve_artifact_metadata("valid-session", "../escape"));

    // Blocked: symlink inside session brain directory escaping to /etc/hosts
    let sess_dir = guard.root.join("symlink-test-session");
    fs::create_dir_all(&sess_dir).unwrap();
    #[cfg(unix)]
    {
        let link_path = sess_dir.join("escape.md");
        std::os::unix::fs::symlink("/etc/hosts", &link_path).unwrap();
        assert!(
            resolve_sandboxed_file_path("escape.md", "symlink-test-session", "exo-suit").is_none(),
            "Symlink escaping brain_dir must be rejected"
        );
    }
}

#[test]
fn seam1_subagent_pruning_task_capsules_and_interactive_question_reconciliation() {
    let _guard = TempBrainGuard::new("graph");
    let test_sid = format!("test-seam1-session-{}", std::process::id());
    let sub_active_id = format!("aaaa1111-2222-3333-4444-{:012x}", std::process::id());
    let sub_done_id = format!("bbbb1111-2222-3333-4444-{:012x}", std::process::id());

    let s_dir = brain_dir().join(&test_sid);
    let s_logs = s_dir.join(".system_generated").join("logs");
    let sub_active_logs = brain_dir()
        .join(&sub_active_id)
        .join(".system_generated")
        .join("logs");
    let sub_done_logs = brain_dir()
        .join(&sub_done_id)
        .join(".system_generated")
        .join("logs");

    fs::create_dir_all(&s_logs).unwrap();
    fs::create_dir_all(&sub_active_logs).unwrap();
    fs::create_dir_all(&sub_done_logs).unwrap();

    // Active subagent has not called send_message yet
    fs::write(
        sub_active_logs.join("transcript.jsonl"),
        format!(
            "{}\n",
            json!({
                "step_index": 1,
                "type": "USER_INPUT",
                "status": "DONE",
                "content": "Audit UI layout"
            })
        ),
    )
    .unwrap();

    // Completed subagent has finished
    fs::write(
        sub_done_logs.join("transcript.jsonl"),
        format!(
            "{}\n",
            json!({
                "step_index": 1,
                "type": "USER_INPUT",
                "status": "DONE",
                "content": "Completed research task"
            })
        ),
    )
    .unwrap();

    // Write an artifact with metadata in the session directory
    fs::write(s_dir.join("design_plan.md"), "# Design Plan\nDetails").unwrap();
    fs::write(
        s_dir.join("design_plan.md.metadata.json"),
        json!({
            "summary": "Architecture Design Plan",
            "requestFeedback": true,
            "userFacing": true,
            "approved": false
        })
        .to_string(),
    )
    .unwrap();

    // Write parent session transcript with:
    // - An earlier answered `ask_question` (step 2)
    // - Two spawned subagents (`sub_active_id` and `sub_done_id`), where `sub_done_id` later sends a completion SYSTEM_MESSAGE
    // - Two background tasks (`task-10` which finishes, and `task-20` which remains running)
    let lines = vec![
        json!({
            "step_index": 1,
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-11-02T15:00:00Z",
            "content": "<USER_REQUEST>Build the lifecycle cockpit</USER_REQUEST>"
        }),
        json!({
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-11-02T15:00:05Z",
            "content": "Let me clarify your preferences.",
            "tool_calls": [
                {
                    "name": "ask_question",
                    "args": {
                        "questions": [
                            {
                                "question": "Which tab should open first?",
                                "options": ["Diff", "Tests"],
                                "is_multi_select": false
                            }
                        ]
                    }
                }
            ]
        }),
        json!({
            "step_index": 3,
            "type": "GENERIC",
            "status": "DONE",
            "content": "A1: Diff"
        }),
        json!({
            "step_index": 4,
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [
                {
                    "name": "invoke_subagent",
                    "args": { "Role": "Active UI Researcher", "Prompt": "Inspect CSS" }
                },
                {
                    "name": "invoke_subagent",
                    "args": { "Role": "Finished Auditor", "Prompt": "Audit rust" }
                },
                {
                    "name": "run_command",
                    "args": { "toolSummary": "Finished Build", "CommandLine": "cargo check", "IsDaemon": false }
                },
                {
                    "name": "run_command",
                    "args": { "toolSummary": "Live Dev Server", "CommandLine": "./target/release/exo-suit", "IsDaemon": true }
                }
            ]
        }),
        json!({
            "step_index": 5,
            "type": "GENERIC",
            "status": "DONE",
            "content": format!(
                "Created the following subagent:\n{{\n  \"conversationId\": \"{}\"\n}}\nCreated the following subagent:\n{{\n  \"conversationId\": \"{}\"\n}}\nTool is running as a background task with task id: {}/task-10\nTool is running as a background task with task id: {}/task-20",
                sub_active_id, sub_done_id, test_sid, test_sid
            )
        }),
        json!({
            "step_index": 6,
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": format!(
                "[Message] sender={} content=Done\n[Message] sender={}/task-10 content=Task finished",
                sub_done_id, test_sid
            )
        }),
    ];

    let serialized = lines
        .into_iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(s_logs.join("transcript.jsonl"), serialized).unwrap();

    // 1. Verify live session graph nodes
    let nodes = build_live_session_graph(&test_sid, "exo-suit");
    // Should contain:
    // - Root lead agent (`node_kind == "agent"`)
    // - Active subagent (`sub_active_id`, `node_kind == "subagent"`), NOT `sub_done_id`
    // - Running task (`task-20`, `node_kind == "task"`), NOT `task-10`
    assert!(nodes.iter().any(|n| n.node_kind == "agent"));
    assert!(
        nodes
            .iter()
            .any(|n| n.node_kind == "subagent" && n.role == "Active UI Researcher"),
        "Active subagent must be present on graph"
    );
    assert!(
        !nodes
            .iter()
            .any(|n| n.node_kind == "subagent" && n.role == "Finished Auditor"),
        "Completed subagent must be pruned from graph"
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.node_kind == "task" && n.agent_id.contains("task-20")),
        "Running background task-20 must be present as a task capsule node"
    );
    assert!(
        !nodes
            .iter()
            .any(|n| n.node_kind == "task" && n.agent_id.contains("task-10")),
        "Completed background task-10 must be pruned from graph"
    );

    // 2. Verify historical `ask_question` is reconciled as "Answered", not stuck in "Pending"
    let chat = get_session_chat_thread(&test_sid);
    let ask_msg = chat
        .iter()
        .find(|m| m.interactive_prompt.is_some())
        .expect("Chat thread should include the interactive question message");
    let ip = ask_msg.interactive_prompt.as_ref().unwrap();
    assert_eq!(ip.status, "Answered");
    assert_eq!(ip.answers, vec!["A1: Diff".to_string()]);

    // 3. Verify artifact loading and interactive answer persistence
    let artifacts = get_session_artifacts(&test_sid);
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].name, "design_plan.md");
    assert!(save_session_interactive_answer(
        &test_sid,
        "ask-step-99",
        &["Option A".to_string()]
    ));
}

#[test]
fn seam1_reconcile_interactive_prompt_avoids_false_positive_and_detects_real_answers() {
    let questions = vec![InteractiveQuestionItem {
        index: 1,
        question: "Deploy target?".to_string(),
        options: vec!["Staging".to_string(), "Production".to_string()],
        is_multi_select: false,
    }];
    let empty_saved = HashMap::new();

    // 1. Followed only by non-USER_INPUT steps (e.g. tool output or system message) -> must remain "Pending"
    let non_answer_steps = vec![
        json!({
            "step_index": 11,
            "type": "GENERIC",
            "status": "DONE",
            "content": "Command exited with code 0"
        }),
        json!({
            "step_index": 12,
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "[Message] sender=task-1 content=finished"
        }),
    ];
    let pending = reconcile_interactive_prompt(10, questions.clone(), &non_answer_steps, &empty_saved);
    assert_eq!(pending.status, "Pending");
    assert!(pending.answers.is_empty());

    // 2. Followed by a real USER_INPUT step -> marked "Answered"
    let user_input_steps = vec![json!({
        "step_index": 11,
        "type": "USER_INPUT",
        "status": "DONE",
        "content": "<USER_REQUEST>Let's deploy to Staging first</USER_REQUEST>"
    })];
    let answered_by_user =
        reconcile_interactive_prompt(10, questions.clone(), &user_input_steps, &empty_saved);
    assert_eq!(answered_by_user.status, "Answered");

    // 3. Saved answers in `.interactive_answers.json` -> marked "Answered" with saved values
    let mut saved = HashMap::new();
    saved.insert("ask-step-10".to_string(), vec!["Staging".to_string()]);
    let answered_by_saved = reconcile_interactive_prompt(10, questions, &non_answer_steps, &saved);
    assert_eq!(answered_by_saved.status, "Answered");
    assert_eq!(answered_by_saved.answers, vec!["Staging".to_string()]);
}

#[test]
fn seam1_atomic_write_and_transcript_append_deduplication() {
    let guard = TempBrainGuard::new("atomic_dedup");

    // 1. Verify atomic_write_string creates parent dirs, writes content, and leaves no .tmp file
    let target_file = guard.root.join("nested").join("config.txt");
    atomic_write_string(&target_file, "initial-value").expect("atomic write should succeed");
    assert_eq!(fs::read_to_string(&target_file).unwrap(), "initial-value");
    atomic_write_string(&target_file, "updated-value").expect("atomic overwrite should succeed");
    assert_eq!(fs::read_to_string(&target_file).unwrap(), "updated-value");
    let tmp_leftovers: Vec<_> = fs::read_dir(target_file.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(tmp_leftovers.is_empty(), "No .tmp.<pid> files should remain");

    // 2. Verify append_turn_to_session_transcript deduplicates identical consecutive replies
    let sid = "dedup-session-1";
    let tokens = TokenMetrics {
        input_tokens: 100,
        cache_read_tokens: 50,
        output_tokens: 25,
    };
    append_turn_to_session_transcript(sid, "Hello agent", "First reply", &[], None, &tokens);
    append_turn_to_session_transcript(sid, "Hello agent", "  First reply  ", &[], None, &tokens);

    let thread = get_session_chat_thread(sid);
    assert_eq!(thread.len(), 2, "Duplicate turn should not be appended twice");

    // 3. Verify append_turn_to_session_transcript skips when agy_conversation_id == session_id
    save_session_agy_conversation_id(sid, sid);
    append_turn_to_session_transcript(sid, "Second prompt", "Second reply", &[], None, &tokens);
    let thread_after = get_session_chat_thread(sid);
    assert_eq!(
        thread_after.len(),
        2,
        "Should skip appending when agy_conversation_id equals session_id"
    );
}
