use exo_suit::brain::{
    brain_dir, build_live_session_graph, classify_file_kind, clean_ansi_and_tags,
    get_session_artifacts, get_session_chat_thread, parse_ask_question_args, read_bounded_file,
    resolve_sandboxed_file_path, save_session_interactive_answer,
};
use serde_json::json;
use std::fs;

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
    // Allowed: file inside ~/code/exo-suit
    let allowed = resolve_sandboxed_file_path("Cargo.toml", "", "exo-suit");
    assert!(allowed.is_some(), "Cargo.toml inside exo-suit should resolve");

    let bounded = read_bounded_file("Cargo.toml", "", "exo-suit")
        .expect("read_bounded_file should read Cargo.toml");
    assert_eq!(bounded.name, "Cargo.toml");
    assert_eq!(bounded.file_kind, "code");
    assert!(bounded.content.contains("exo-suit"));

    // Blocked: system file outside ~/code and ~/.gemini
    assert!(resolve_sandboxed_file_path("/etc/passwd", "", "exo-suit").is_none());
    assert!(resolve_sandboxed_file_path("../../../../etc/hosts", "", "exo-suit").is_none());
}

#[test]
fn seam1_subagent_pruning_task_capsules_and_interactive_question_reconciliation() {
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

    // Cleanup test directories
    let _ = fs::remove_dir_all(&s_dir);
    let _ = fs::remove_dir_all(brain_dir().join(&sub_active_id));
    let _ = fs::remove_dir_all(brain_dir().join(&sub_done_id));
}
