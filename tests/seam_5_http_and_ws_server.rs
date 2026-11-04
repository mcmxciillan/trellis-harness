use exo_suit::{
    ServerContext, build_router,
    council::{ActiveRunHandle, SharedAppState},
    dispatch_client_command, generate_snapshot_json, resolve_runtime_port, set_runtime_port,
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, broadcast, mpsc, watch};

fn make_test_context() -> ServerContext {
    let state = Arc::new(RwLock::new(SharedAppState {
        active_project: "exo-suit".to_string(),
        active_session_id: "test-http-session".to_string(),
        agents: Vec::new(),
        council: None,
        chat_thread: Vec::new(),
        active_session_live_messages: Vec::new(),
        session_custom_messages: HashMap::new(),
        active_runs: HashMap::new(),
        devops_tests: HashMap::new(),
        api_keys: HashMap::new(),
    }));
    let (tx, _rx) = broadcast::channel::<String>(128);
    ServerContext { state, tx }
}

async fn spawn_ephemeral_server(ctx: ServerContext) -> String {
    let app = build_router(ctx);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{}", addr)
}

#[tokio::test]
async fn seam5_http_server_serves_static_assets_and_health_json() {
    let ctx = make_test_context();
    let base_url = spawn_ephemeral_server(ctx).await;
    let client = reqwest::Client::new();

    // 1. Index HTML
    let idx_res = client
        .get(format!("{}/", base_url))
        .send()
        .await
        .expect("GET /");
    assert_eq!(idx_res.status(), 200);
    let idx_body = idx_res.text().await.unwrap();
    assert!(idx_body.contains("TRELLIS"));
    assert!(idx_body.contains("new-session-btn"));

    // 2. Styles CSS & App JS
    let css_res = client
        .get(format!("{}/styles.css", base_url))
        .send()
        .await
        .expect("GET /styles.css");
    assert_eq!(css_res.status(), 200);

    let js_res = client
        .get(format!("{}/app.js", base_url))
        .send()
        .await
        .expect("GET /app.js");
    assert_eq!(js_res.status(), 200);

    // 3. Health JSON endpoint
    let health_res = client
        .get(format!("{}/api/health", base_url))
        .send()
        .await
        .expect("GET /api/health");
    assert_eq!(health_res.status(), 200);
    let health_json: Value = health_res.json().await.expect("parse health json");
    assert_eq!(health_json["status"], "ok");
    assert_eq!(health_json["engine"], "exo-suit-rust");
    assert_eq!(health_json["active_project"], "exo-suit");
    assert!(health_json["providers"].is_array());
}

#[tokio::test]
async fn seam5_raw_file_endpoint_serves_sandboxed_files_and_rejects_traversal() {
    let ctx = make_test_context();
    let base_url = spawn_ephemeral_server(ctx).await;
    let client = reqwest::Client::new();

    // 1. Valid workspace file
    let ok_res = client
        .get(format!("{}/raw-file?path=Cargo.toml", base_url))
        .send()
        .await
        .expect("GET /raw-file?path=Cargo.toml");
    assert_eq!(ok_res.status(), 200);
    let cargo_toml = ok_res.text().await.unwrap();
    assert!(cargo_toml.contains("name = \"exo-suit\""));

    // 2. Absolute system path outside sandbox -> 404
    let bad_abs = client
        .get(format!("{}/raw-file?path=/etc/passwd", base_url))
        .send()
        .await
        .expect("GET /raw-file?path=/etc/passwd");
    assert_eq!(bad_abs.status(), 404);

    // 3. Relative path traversal -> 404
    let bad_rel = client
        .get(format!("{}/raw-file?path=../../etc/passwd", base_url))
        .send()
        .await
        .expect("GET /raw-file?path=../../etc/passwd");
    assert_eq!(bad_rel.status(), 404);
}

#[tokio::test]
async fn seam5_telemetry_snapshot_contains_complete_cockpit_and_workspace_contract() {
    let ctx = make_test_context();
    let snap_raw = generate_snapshot_json(&ctx.state).await;
    let snap: Value = serde_json::from_str(&snap_raw).expect("valid TELEMETRY_SNAPSHOT JSON");

    assert_eq!(snap["type"], "TELEMETRY_SNAPSHOT");
    assert_eq!(snap["project_id"], "exo-suit");
    assert_eq!(snap["active_session_id"], "test-http-session");
    assert!(snap["available_projects"].is_array());
    assert!(
        snap["available_projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("exo-suit"))
    );
    assert!(snap["providers"].is_array());
    assert!(snap["devops"].is_object());
    assert_eq!(snap["devops"]["project_id"], "exo-suit");
    assert!(snap["skills"].is_array());
    assert!(snap["tools"].is_array());
}

#[tokio::test]
async fn seam5_ws_command_dispatcher_handles_open_file_git_diff_api_keys_and_cancellation() {
    let ctx = make_test_context();
    let mut bcast_rx = ctx.tx.subscribe();
    let (direct_tx, mut direct_rx) = mpsc::unbounded_channel::<String>();

    // 1. OPEN_FILE -> emits FILE_OPENED on direct_rx
    dispatch_client_command(
        &json!({
            "command": "OPEN_FILE",
            "path": "Cargo.toml"
        })
        .to_string(),
        &ctx,
        &direct_tx,
    )
    .await;

    let msg1: Value = serde_json::from_str(&direct_rx.recv().await.unwrap()).unwrap();
    assert_eq!(msg1["type"], "FILE_OPENED");
    assert_eq!(msg1["file"]["name"], "Cargo.toml");
    assert!(
        msg1["file"]["content"]
            .as_str()
            .unwrap()
            .contains("exo_suit")
    );

    // 2. OPEN_GIT_DIFF -> emits GIT_DIFF_OPENED on direct_rx
    dispatch_client_command(
        &json!({
            "command": "OPEN_GIT_DIFF",
            "project_id": "exo-suit",
            "path": "src/lib.rs"
        })
        .to_string(),
        &ctx,
        &direct_tx,
    )
    .await;

    let msg2: Value = serde_json::from_str(&direct_rx.recv().await.unwrap()).unwrap();
    assert_eq!(msg2["type"], "GIT_DIFF_OPENED");
    assert_eq!(msg2["git_diff"]["project_id"], "exo-suit");

    // 3. CREATE_SESSION_OR_PROJECT with invalid traversal slug -> emits SESSION_CREATE_ERROR on direct_rx
    dispatch_client_command(
        &json!({
            "command": "CREATE_SESSION_OR_PROJECT",
            "mode": "new_repo",
            "project_id": "../bad-slug"
        })
        .to_string(),
        &ctx,
        &direct_tx,
    )
    .await;

    let msg3: Value = serde_json::from_str(&direct_rx.recv().await.unwrap()).unwrap();
    assert_eq!(msg3["type"], "SESSION_CREATE_ERROR");

    // 4. SET_API_KEYS -> updates state and broadcasts PROVIDERS_UPDATED
    dispatch_client_command(
        &json!({
            "command": "SET_API_KEYS",
            "api_keys": {
                "claude": "sk-ant-test-key-123"
            }
        })
        .to_string(),
        &ctx,
        &direct_tx,
    )
    .await;

    let bcast1: Value = serde_json::from_str(&bcast_rx.recv().await.unwrap()).unwrap();
    assert_eq!(bcast1["type"], "PROVIDERS_UPDATED");
    let claude_entry = bcast1["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "claude")
        .expect("claude provider entry");
    assert_eq!(claude_entry["has_api_key"], true);

    // 5. CANCEL_EXECUTION -> cancels active run and broadcasts EXECUTION_CANCELLED
    let (abort_tx, abort_rx) = watch::channel(false);
    {
        let mut st = ctx.state.write().await;
        st.active_runs.insert(
            "run-ws-99".to_string(),
            ActiveRunHandle {
                run_id: "run-ws-99".to_string(),
                session_id: "test-http-session".to_string(),
                project_id: "exo-suit".to_string(),
                node_ids: vec![],
                reply_msg_id: String::new(),
                abort_tx,
            },
        );
    }

    dispatch_client_command(
        &json!({
            "command": "CANCEL_EXECUTION",
            "run_id": "run-ws-99"
        })
        .to_string(),
        &ctx,
        &direct_tx,
    )
    .await;

    let bcast2: Value = serde_json::from_str(&bcast_rx.recv().await.unwrap()).unwrap();
    assert_eq!(bcast2["type"], "EXECUTION_CANCELLED");
    assert_eq!(bcast2["cancelled_runs"][0], "run-ws-99");
    assert!(*abort_rx.borrow());
}

#[tokio::test]
async fn seam5_ws_command_dispatcher_emits_typed_error_envelopes_and_propagates_runtime_port() {
    let ctx = make_test_context();
    let (direct_tx, mut direct_rx) = mpsc::unbounded_channel::<String>();

    // 1. Malformed JSON -> COMMAND_ERROR with command == "PARSE_ERROR"
    dispatch_client_command("{not valid json", &ctx, &direct_tx).await;
    let err_parse: Value = serde_json::from_str(&direct_rx.recv().await.unwrap()).unwrap();
    assert_eq!(err_parse["type"], "COMMAND_ERROR");
    assert_eq!(err_parse["command"], "PARSE_ERROR");
    assert!(
        err_parse["error"]
            .as_str()
            .unwrap_or("")
            .contains("Invalid JSON command payload")
    );

    // 2. Unknown command -> COMMAND_ERROR with command == "NON_EXISTENT_CMD"
    dispatch_client_command(
        &json!({
            "command": "NON_EXISTENT_CMD"
        })
        .to_string(),
        &ctx,
        &direct_tx,
    )
    .await;
    let err_unknown: Value = serde_json::from_str(&direct_rx.recv().await.unwrap()).unwrap();
    assert_eq!(err_unknown["type"], "COMMAND_ERROR");
    assert_eq!(err_unknown["command"], "NON_EXISTENT_CMD");
    assert!(
        err_unknown["error"]
            .as_str()
            .unwrap_or("")
            .contains("NON_EXISTENT_CMD")
    );

    // 3. OPEN_FILE outside sandbox -> COMMAND_ERROR with command == "OPEN_FILE"
    dispatch_client_command(
        &json!({
            "command": "OPEN_FILE",
            "path": "/etc/passwd"
        })
        .to_string(),
        &ctx,
        &direct_tx,
    )
    .await;
    let err_file: Value = serde_json::from_str(&direct_rx.recv().await.unwrap()).unwrap();
    assert_eq!(err_file["type"], "COMMAND_ERROR");
    assert_eq!(err_file["command"], "OPEN_FILE");
    assert_eq!(err_file["path"], "/etc/passwd");
    assert_eq!(
        err_file["error"],
        "File not found or outside workspace sandbox"
    );

    // 4. Dynamic runtime port propagation
    set_runtime_port(9456);
    assert_eq!(resolve_runtime_port(), 9456);
    set_runtime_port(9000);
    assert_eq!(resolve_runtime_port(), 9000);
}

