pub mod brain;
pub mod council;
pub mod devops;
pub mod models;
pub mod providers;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use brain::{
    approve_artifact_metadata, brain_dir, build_live_session_graph, code_root_dir,
    get_installed_skills, get_past_sessions, get_session_artifacts,
    get_tool_catalog, infer_session_project_id, list_directory_contents, list_workspace_projects,
    provision_workspace_and_session, read_bounded_file, rename_session,
    resolve_sandboxed_file_path, save_session_project_id,
};
use council::{
    now_iso_short, run_council_session, run_independent_prompt, submit_interactive_answer,
    SharedAppState,
};
use devops::{build_workspace_devops_snapshot, compute_workspace_diff, run_workspace_tests};
use futures_util::{SinkExt, StreamExt};
use models::ClientCommand;
use notify::{Event, RecursiveMode, Result as NotifyResult, Watcher};
use providers::build_provider_registry;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU16, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{broadcast, mpsc::UnboundedSender, RwLock};

pub const INDEX_HTML: &str = include_str!("../web/index.html");
pub const STYLES_CSS: &str = include_str!("../web/styles.css");
pub const APP_JS: &str = include_str!("../web/app.js");
pub const MANIFEST_JSON: &str = include_str!("../web/manifest.json");
pub const LOGO_JPG: &[u8] = include_bytes!("../trellis-espalier.jpg");

static RUNTIME_PORT: AtomicU16 = AtomicU16::new(0);

pub fn set_runtime_port(port: u16) {
    RUNTIME_PORT.store(port, Ordering::Relaxed);
    council::set_active_server_port(port);
}

pub fn resolve_runtime_port() -> u16 {
    let configured = RUNTIME_PORT.load(Ordering::Relaxed);
    if configured != 0 {
        return configured;
    }
    council::active_server_port()
}

pub fn bootstrap_shared_state(port: u16) -> Arc<RwLock<SharedAppState>> {
    set_runtime_port(port);
    Arc::new(RwLock::new(SharedAppState::bootstrap_from_disk(port)))
}

#[derive(Clone)]
pub struct ServerContext {
    pub state: Arc<RwLock<SharedAppState>>,
    pub tx: broadcast::Sender<String>,
}

pub fn build_router(ctx: ServerContext) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/index.html", get(index_handler))
        .route("/styles.css", get(styles_handler))
        .route("/app.js", get(js_handler))
        .route("/manifest.json", get(manifest_handler))
        .route("/trellis-espalier.jpg", get(logo_handler))
        .route("/raw-file", get(raw_file_handler))
        .route("/ws", get(ws_handler))
        .route("/api/health", get(health_handler))
        .with_state(ctx)
}

async fn index_handler() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
        ],
        INDEX_HTML,
    )
        .into_response()
}

async fn styles_handler() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
        ],
        STYLES_CSS,
    )
        .into_response()
}

async fn js_handler() -> Response {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
        ],
        APP_JS,
    )
        .into_response()
}

async fn manifest_handler() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/manifest+json"),
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
        ],
        MANIFEST_JSON,
    )
        .into_response()
}

async fn logo_handler() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/jpeg")],
        LOGO_JPG,
    )
        .into_response()
}

async fn raw_file_handler(
    Query(params): Query<HashMap<String, String>>,
    State(ctx): State<ServerContext>,
) -> Response {
    let raw_path = params.get("path").cloned().unwrap_or_default();
    let (s_id, proj) = {
        let st = ctx.state.read().await;
        (st.active_session_id.clone(), st.active_project.clone())
    };
    let Some(safe_path) = resolve_sandboxed_file_path(&raw_path, &s_id, &proj) else {
        return (StatusCode::NOT_FOUND, "File not found in sandbox").into_response();
    };
    let Ok(bytes) = std::fs::read(&safe_path) else {
        return (StatusCode::NOT_FOUND, "Unable to read file").into_response();
    };
    let ext = safe_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let content_type = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type)],
        bytes,
    )
        .into_response()
}

async fn health_handler(State(ctx): State<ServerContext>) -> Json<serde_json::Value> {
    let (active_project, active_session_id, agent_count, api_keys) = {
        let st = ctx.state.read().await;
        (
            st.active_project.clone(),
            st.active_session_id.clone(),
            st.agents.len(),
            st.api_keys.clone(),
        )
    };
    let providers = build_provider_registry(&api_keys);
    Json(json!({
        "status": "ok",
        "engine": "exo-suit-rust",
        "active_project": active_project,
        "active_session_id": active_session_id,
        "agent_count": agent_count,
        "providers": providers,
    }))
}

async fn ws_handler(ws: WebSocketUpgrade, State(ctx): State<ServerContext>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, ctx))
}

pub async fn generate_snapshot_json(state: &Arc<RwLock<SharedAppState>>) -> String {
    let (
        active_project,
        active_session_id,
        active_run_count,
        agents,
        council,
        chat_thread,
        api_keys,
        cached_test,
    ) = {
        let st = state.read().await;
        let ct = st.devops_tests.get(&st.active_project).cloned();
        (
            st.active_project.clone(),
            st.active_session_id.clone(),
            st.active_runs.len(),
            st.agents.clone(),
            st.council.clone(),
            st.chat_thread.clone(),
            st.api_keys.clone(),
            ct,
        )
    };

    let sessions = get_past_sessions(25);
    let artifacts = get_session_artifacts(&active_session_id);
    let file_tree = list_directory_contents(&active_project);
    let providers = build_provider_registry(&api_keys);
    let skills = get_installed_skills();
    let tools = get_tool_catalog();
    let devops =
        build_workspace_devops_snapshot(&active_project, cached_test, resolve_runtime_port());
    let available_projects = list_workspace_projects();

    json!({
        "type": "TELEMETRY_SNAPSHOT",
        "timestamp": now_iso_short(),
        "project_id": active_project,
        "root_dir": code_root_dir().to_string_lossy(),
        "active_session_id": active_session_id,
        "active_run_count": active_run_count,
        "available_projects": available_projects,
        "providers": providers,
        "sessions": sessions,
        "skills": skills,
        "tools": tools,
        "artifacts": artifacts,
        "file_browser": file_tree,
        "devops": devops,
        "agents": agents,
        "council": council,
        "chat_thread": chat_thread,
    })
    .to_string()
}

async fn handle_socket(socket: WebSocket, ctx: ServerContext) {
    let (mut sender, mut receiver) = socket.split();
    let mut bcast_rx = ctx.tx.subscribe();

    // Send initial full telemetry snapshot immediately upon connection
    let initial_snapshot = generate_snapshot_json(&ctx.state).await;
    if sender.send(Message::Text(initial_snapshot)).await.is_err() {
        return;
    }

    let (direct_tx, mut direct_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Forward both broadcast and direct responses to client WebSocket
    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Ok(msg) = bcast_rx.recv() => {
                    if sender.send(Message::Text(msg)).await.is_err() {
                        break;
                    }
                }
                Some(direct_msg) = direct_rx.recv() => {
                    if sender.send(Message::Text(direct_msg)).await.is_err() {
                        break;
                    }
                }
                else => break,
            }
        }
    });

    while let Some(Ok(Message::Text(raw_txt))) = receiver.next().await {
        dispatch_client_command(&raw_txt, &ctx, &direct_tx).await;
    }

    send_task.abort();
}

pub async fn dispatch_client_command(
    raw_txt: &str,
    ctx: &ServerContext,
    direct_tx: &UnboundedSender<String>,
) {
    let cmd = match serde_json::from_str::<ClientCommand>(raw_txt) {
        Ok(c) => c,
        Err(e) => {
            let _ = direct_tx.send(
                json!({
                    "type": "COMMAND_ERROR",
                    "command": "PARSE_ERROR",
                    "error": format!("Invalid JSON command payload: {}", e)
                })
                .to_string(),
            );
            return;
        }
    };

    match cmd.command.as_str() {
        "BROWSE_DIR" => handle_browse_dir(cmd, ctx, direct_tx).await,
        "OPEN_FILE" => handle_open_file(cmd, ctx, direct_tx).await,
        "REFRESH_DEVOPS" => handle_refresh_devops(cmd, ctx, direct_tx).await,
        "OPEN_GIT_DIFF" => handle_open_git_diff(cmd, ctx, direct_tx).await,
        "RUN_WORKSPACE_TESTS" => handle_run_workspace_tests(cmd, ctx).await,
        "SET_PROJECT" => handle_set_project(cmd, ctx).await,
        "SELECT_SESSION" => handle_select_session(cmd, ctx).await,
        "RENAME_SESSION" => handle_rename_session(cmd, ctx).await,
        "CREATE_SESSION_OR_PROJECT" => handle_create_session(cmd, ctx, direct_tx).await,
        "APPROVE_ARTIFACT" => handle_approve_artifact(cmd, ctx).await,
        "APPROVE_TOOL_EXECUTION" => handle_approve_tool(cmd, ctx).await,
        "SET_API_KEYS" => handle_set_api_keys(cmd, ctx).await,
        "SUBMIT_PROMPT" => handle_submit_prompt(cmd, ctx),
        "CANCEL_EXECUTION" => handle_cancel_execution(cmd, ctx).await,
        "ANSWER_QUESTION" => handle_answer_question(cmd, ctx),
        "START_COUNCIL" => handle_start_council(cmd, ctx),
        _ => {
            let _ = direct_tx.send(
                json!({
                    "type": "COMMAND_ERROR",
                    "command": cmd.command,
                    "error": format!("Unknown WebSocket command: {}", cmd.command)
                })
                .to_string(),
            );
        }
    }
}

async fn handle_browse_dir(
    cmd: ClientCommand,
    ctx: &ServerContext,
    direct_tx: &UnboundedSender<String>,
) {
    let path = cmd.path.unwrap_or_default();
    let tree = list_directory_contents(&path);
    let top_slug = path
        .trim_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let _ = direct_tx.send(json!({ "type": "DIRECTORY_CONTENTS", "file_browser": tree }).to_string());
    if !top_slug.is_empty()
        && !top_slug.starts_with('.')
        && code_root_dir().join(&top_slug).is_dir()
    {
        switch_project_on_browse(ctx, &top_slug).await;
    }
}

async fn switch_project_on_browse(ctx: &ServerContext, top_slug: &str) {
    let (current_proj, s_id) = {
        let st = ctx.state.read().await;
        (st.active_project.clone(), st.active_session_id.clone())
    };
    if current_proj == top_slug {
        return;
    }
    save_session_project_id(&s_id, top_slug);
    let updated_agents = build_live_session_graph(&s_id, top_slug);
    let cached_test = {
        let mut st = ctx.state.write().await;
        st.active_project = top_slug.to_string();
        st.agents = updated_agents.clone();
        st.devops_tests.get(top_slug).cloned()
    };
    let sessions = get_past_sessions(25);
    let devops = build_workspace_devops_snapshot(top_slug, cached_test, resolve_runtime_port());
    let _ = ctx.tx.send(
        json!({
            "type": "PROJECT_CONTEXT_UPDATED",
            "project_id": top_slug,
            "agents": updated_agents,
            "sessions": sessions,
            "devops": devops
        })
        .to_string(),
    );
}

async fn handle_open_file(
    cmd: ClientCommand,
    ctx: &ServerContext,
    direct_tx: &UnboundedSender<String>,
) {
    let target = cmd.path.unwrap_or_default();
    let (s_id, proj) = {
        let st = ctx.state.read().await;
        (st.active_session_id.clone(), st.active_project.clone())
    };
    let opened = resolve_sandboxed_file_path(&target, &s_id, &proj)
        .and_then(|_| read_bounded_file(&target, &s_id, &proj));
    match opened {
        Some(f) => {
            let _ = direct_tx.send(json!({ "type": "FILE_OPENED", "file": f }).to_string());
        }
        None => {
            let _ = direct_tx.send(
                json!({
                    "type": "COMMAND_ERROR",
                    "command": "OPEN_FILE",
                    "path": target,
                    "error": "File not found or outside workspace sandbox"
                })
                .to_string(),
            );
        }
    }
}

async fn handle_refresh_devops(
    cmd: ClientCommand,
    ctx: &ServerContext,
    direct_tx: &UnboundedSender<String>,
) {
    let (proj, cached_test) = {
        let st = ctx.state.read().await;
        let p = cmd
            .project_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| st.active_project.clone());
        let ct = st.devops_tests.get(&p).cloned();
        (p, ct)
    };
    let devops = build_workspace_devops_snapshot(&proj, cached_test, resolve_runtime_port());
    let _ = direct_tx.send(json!({ "type": "DEVOPS_UPDATED", "devops": devops }).to_string());
}

async fn handle_open_git_diff(
    cmd: ClientCommand,
    ctx: &ServerContext,
    direct_tx: &UnboundedSender<String>,
) {
    let proj = {
        let st = ctx.state.read().await;
        cmd.project_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| st.active_project.clone())
    };
    let diff_payload = compute_workspace_diff(&proj, cmd.path.as_deref());
    let _ = direct_tx
        .send(json!({ "type": "GIT_DIFF_OPENED", "git_diff": diff_payload }).to_string());
}

async fn handle_run_workspace_tests(cmd: ClientCommand, ctx: &ServerContext) {
    let proj = {
        let st = ctx.state.read().await;
        cmd.project_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| st.active_project.clone())
    };
    tokio::spawn(run_workspace_tests(
        ctx.state.clone(),
        ctx.tx.clone(),
        proj,
        resolve_runtime_port(),
    ));
}

async fn handle_set_project(cmd: ClientCommand, ctx: &ServerContext) {
    let raw_proj = cmd.path.unwrap_or_else(|| "exo-suit".to_string());
    let proj = raw_proj
        .trim_matches('/')
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("exo-suit")
        .to_string();
    let s_id = {
        let st = ctx.state.read().await;
        st.active_session_id.clone()
    };
    save_session_project_id(&s_id, &proj);
    let updated_agents = build_live_session_graph(&s_id, &proj);
    {
        let mut st = ctx.state.write().await;
        st.active_project = proj;
        st.agents = updated_agents;
    }
    let snap = generate_snapshot_json(&ctx.state).await;
    let _ = ctx.tx.send(snap);
}

async fn handle_select_session(cmd: ClientCommand, ctx: &ServerContext) {
    let s_id = cmd.session_id.unwrap_or_default();
    let inferred_proj = infer_session_project_id(&s_id);
    let file_tree = list_directory_contents(&inferred_proj);
    let agents = build_live_session_graph(&s_id, &inferred_proj);
    let artifacts = get_session_artifacts(&s_id);
    let (chat_thread, cached_test) = {
        let mut st = ctx.state.write().await;
        st.active_session_id = s_id.clone();
        st.active_project = inferred_proj.clone();
        st.council = None;
        st.agents = agents.clone();
        let merged_chat = st.load_merged_session_chat(&s_id);
        let ct = st.devops_tests.get(&inferred_proj).cloned();
        (merged_chat, ct)
    };
    let devops =
        build_workspace_devops_snapshot(&inferred_proj, cached_test, resolve_runtime_port());
    let _ = ctx.tx.send(
        json!({
            "type": "SESSION_LOADED",
            "active_session_id": s_id,
            "project_id": inferred_proj,
            "file_browser": file_tree,
            "devops": devops,
            "artifacts": artifacts,
            "agents": agents,
            "chat_thread": chat_thread,
            "council": serde_json::Value::Null
        })
        .to_string(),
    );
}

async fn handle_rename_session(cmd: ClientCommand, ctx: &ServerContext) {
    let s_id = cmd.session_id.unwrap_or_default();
    let new_title = cmd.title.unwrap_or_default();
    rename_session(&s_id, &new_title);
    let sessions = get_past_sessions(25);
    let _ = ctx
        .tx
        .send(json!({ "type": "SESSIONS_UPDATED", "sessions": sessions }).to_string());
}

async fn handle_create_session(
    cmd: ClientCommand,
    ctx: &ServerContext,
    direct_tx: &UnboundedSender<String>,
) {
    let mode = cmd.mode.clone().unwrap_or_else(|| "existing_repo".to_string());
    let raw_proj = cmd.project_id.clone().unwrap_or_default();
    let init_git = cmd.init_git.unwrap_or(true);

    match provision_workspace_and_session(
        &mode,
        &raw_proj,
        cmd.title.as_deref(),
        cmd.description.as_deref(),
        init_git,
    ) {
        Ok(prov) => finalize_created_session(cmd, ctx, prov).await,
        Err(err_msg) => {
            let _ = direct_tx.send(
                json!({ "type": "SESSION_CREATE_ERROR", "error": err_msg }).to_string(),
            );
        }
    }
}

async fn finalize_created_session(
    cmd: ClientCommand,
    ctx: &ServerContext,
    prov: models::SessionProvisionResult,
) {
    let new_sid = prov.session_id.clone();
    let new_proj = prov.project_id.clone();
    let file_tree = list_directory_contents(&new_proj);
    let agents = build_live_session_graph(&new_sid, &new_proj);
    let artifacts = get_session_artifacts(&new_sid);
    let (chat_thread, cached_test) = {
        let mut st = ctx.state.write().await;
        st.active_session_id = new_sid.clone();
        st.active_project = new_proj.clone();
        st.council = None;
        st.agents = agents.clone();
        let merged_chat = st.load_merged_session_chat(&new_sid);
        let ct = st.devops_tests.get(&new_proj).cloned();
        (merged_chat, ct)
    };
    let sessions = get_past_sessions(25);
    let available_projects = list_workspace_projects();
    let devops = build_workspace_devops_snapshot(&new_proj, cached_test, resolve_runtime_port());
    let _ = ctx.tx.send(
        json!({
            "type": "SESSION_CREATED",
            "active_session_id": new_sid,
            "project_id": new_proj,
            "sessions": sessions,
            "available_projects": available_projects,
            "file_browser": file_tree,
            "devops": devops,
            "artifacts": artifacts,
            "agents": agents,
            "chat_thread": chat_thread,
            "council": serde_json::Value::Null,
            "provision": prov
        })
        .to_string(),
    );
    if cmd.auto_kickoff.unwrap_or(false) {
        spawn_greenfield_kickoff(ctx, cmd, new_sid, new_proj);
    }
}

fn spawn_greenfield_kickoff(
    ctx: &ServerContext,
    cmd: ClientCommand,
    new_sid: String,
    new_proj: String,
) {
    let provider = cmd.provider.unwrap_or_else(|| "antigravity".to_string());
    let model = cmd.model.unwrap_or_else(|| "gemini-4-argon".to_string());
    let desc_summary = cmd
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    let kickoff_prompt = match desc_summary {
        Some(d) => format!(
            "/greenfield-architecture Design and decompose the 8-Pillar architecture for ~/code/{}: {}",
            new_proj, d
        ),
        None => format!(
            "/greenfield-architecture Audit ~/code/{} and initiate Day-1 8-Pillar architecture discovery.",
            new_proj
        ),
    };
    tokio::spawn(run_independent_prompt(
        ctx.state.clone(),
        ctx.tx.clone(),
        provider,
        Some(model),
        kickoff_prompt,
        Some(new_sid),
        Some(new_proj),
    ));
}

async fn handle_approve_artifact(cmd: ClientCommand, ctx: &ServerContext) {
    let art_name = cmd.artifact_name.unwrap_or_default();
    let s_id = cmd.session_id.unwrap_or_default();
    approve_artifact_metadata(&s_id, &art_name);
    let artifacts = get_session_artifacts(&s_id);
    let _ = ctx.tx.send(
        json!({
            "type": "ARTIFACT_UPDATED",
            "active_session_id": s_id,
            "artifacts": artifacts
        })
        .to_string(),
    );
}

async fn handle_approve_tool(cmd: ClientCommand, ctx: &ServerContext) {
    let agent_id = cmd.agent_id.unwrap_or_default();
    let agents = {
        let mut st = ctx.state.write().await;
        if let Some(agent) = st.agents.iter_mut().find(|a| a.agent_id == agent_id) {
            agent.status = "Running".to_string();
            agent.recent_log = "Tool execution approved by operator.".to_string();
            for tc in &mut agent.tool_calls {
                if tc.status == "Pending" {
                    tc.status = "Completed".to_string();
                }
            }
        }
        st.agents.clone()
    };
    let _ = ctx
        .tx
        .send(json!({ "type": "AGENTS_UPDATED", "agents": agents }).to_string());
}

async fn handle_set_api_keys(cmd: ClientCommand, ctx: &ServerContext) {
    let Some(keys) = cmd.api_keys else {
        return;
    };
    let updated_keys = {
        let mut st = ctx.state.write().await;
        for (k, v) in keys {
            if !v.is_empty() {
                st.api_keys.insert(k, v);
            }
        }
        st.api_keys.clone()
    };
    let providers = build_provider_registry(&updated_keys);
    let _ = ctx
        .tx
        .send(json!({ "type": "PROVIDERS_UPDATED", "providers": providers }).to_string());
}

fn handle_submit_prompt(cmd: ClientCommand, ctx: &ServerContext) {
    let prompt = cmd.prompt.unwrap_or_default();
    if prompt.is_empty() {
        return;
    }
    let provider = cmd.provider.unwrap_or_else(|| "antigravity".to_string());
    tokio::spawn(run_independent_prompt(
        ctx.state.clone(),
        ctx.tx.clone(),
        provider,
        cmd.model,
        prompt,
        cmd.session_id,
        cmd.project_id,
    ));
}

async fn handle_cancel_execution(cmd: ClientCommand, ctx: &ServerContext) {
    let target_ref = cmd
        .run_id
        .as_deref()
        .or(cmd.session_id.as_deref())
        .map(|s| s.to_string());
    let (cancelled_runs, agents, chat_thread, council) = {
        let mut st = ctx.state.write().await;
        let cancelled = st.cancel_execution(target_ref.as_deref());
        (
            cancelled,
            st.agents.clone(),
            st.chat_thread.clone(),
            st.council.clone(),
        )
    };
    let _ = ctx.tx.send(
        json!({
            "type": "EXECUTION_CANCELLED",
            "cancelled_runs": cancelled_runs,
            "agents": agents,
            "chat_thread": chat_thread,
            "council": council
        })
        .to_string(),
    );
}

fn handle_answer_question(cmd: ClientCommand, ctx: &ServerContext) {
    let s_id = cmd.session_id.unwrap_or_default();
    let prompt_id = cmd.prompt_id.unwrap_or_default();
    let answers = cmd.answers.unwrap_or_default();
    if prompt_id.is_empty() || answers.is_empty() {
        return;
    }
    let provider = cmd.provider.unwrap_or_else(|| "antigravity".to_string());
    tokio::spawn(submit_interactive_answer(
        ctx.state.clone(),
        ctx.tx.clone(),
        s_id,
        prompt_id,
        answers,
        provider,
        cmd.model,
    ));
}

fn handle_start_council(cmd: ClientCommand, ctx: &ServerContext) {
    let prompt = cmd.prompt.unwrap_or_default();
    if prompt.is_empty() {
        return;
    }
    let providers = cmd.providers.unwrap_or_default();
    tokio::spawn(run_council_session(
        ctx.state.clone(),
        ctx.tx.clone(),
        providers,
        prompt,
        cmd.session_id,
        cmd.project_id,
    ));
}

pub fn spawn_brain_watcher(state: Arc<RwLock<SharedAppState>>, tx: broadcast::Sender<String>) {
    let bdir = brain_dir();
    if !bdir.exists() {
        return;
    }

    let (watch_tx, mut watch_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    std::thread::spawn(move || {
        let mut watcher = notify::recommended_watcher(move |res: NotifyResult<Event>| {
            if let Ok(ev) = res {
                let relevant = ev.paths.iter().any(|p| {
                    let s = p.to_string_lossy();
                    s.ends_with("transcript.jsonl") || s.ends_with(".metadata.json")
                });
                if relevant {
                    let _ = watch_tx.send(());
                }
            }
        })
        .ok();

        if let Some(ref mut w) = watcher {
            let _ = w.watch(&bdir, RecursiveMode::Recursive);
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
    });

    tokio::spawn(async move {
        let mut last_session_id = String::new();
        let mut last_transcript_len: u64 = 0;

        loop {
            let notified =
                tokio::time::timeout(Duration::from_millis(300), watch_rx.recv())
                    .await
                    .is_ok();
            while watch_rx.try_recv().is_ok() {}

            let (s_id, proj, council_active, has_active_run) = {
                let st = state.read().await;
                let c_active = st
                    .council
                    .as_ref()
                    .map(|c| c.stage != "COMPLETED" && c.stage != "CANCELLED")
                    .unwrap_or(false);
                (
                    st.active_session_id.clone(),
                    st.active_project.clone(),
                    c_active,
                    !st.active_runs.is_empty(),
                )
            };

            if council_active || has_active_run {
                continue;
            }

            let transcript_p = brain_dir()
                .join(&s_id)
                .join(".system_generated")
                .join("logs")
                .join("transcript.jsonl");
            let current_len = std::fs::metadata(&transcript_p)
                .map(|m| m.len())
                .unwrap_or(0);

            if s_id != last_session_id {
                last_session_id = s_id.clone();
                last_transcript_len = current_len;
                if !notified {
                    continue;
                }
            } else if !notified && current_len == last_transcript_len {
                continue;
            }
            last_transcript_len = current_len;

            let updated_nodes = build_live_session_graph(&s_id, &proj);
            let artifacts = get_session_artifacts(&s_id);
            let (updated_chat, cached_test) = {
                let mut st = state.write().await;
                st.agents = updated_nodes.clone();
                let ct = st.devops_tests.get(&proj).cloned();
                (st.load_merged_session_chat(&s_id), ct)
            };
            let devops =
                build_workspace_devops_snapshot(&proj, cached_test, resolve_runtime_port());
            let _ = tx.send(
                json!({
                    "type": "LIVE_BRAIN_UPDATE",
                    "agents": updated_nodes,
                    "chat_thread": updated_chat,
                    "artifacts": artifacts,
                    "devops": devops
                })
                .to_string(),
            );
        }
    });
}
