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
    get_installed_skills, get_past_sessions, get_session_artifacts, get_tool_catalog,
    infer_session_project_id, list_directory_contents, list_workspace_projects,
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
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{broadcast, RwLock};

pub const INDEX_HTML: &str = include_str!("../web/index.html");
pub const STYLES_CSS: &str = include_str!("../web/styles.css");
pub const APP_JS: &str = include_str!("../web/app.js");
pub const MANIFEST_JSON: &str = include_str!("../web/manifest.json");
pub const LOGO_JPG: &[u8] = include_bytes!("../trellis-espalier.jpg");

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
    let st = ctx.state.read().await;
    let providers = build_provider_registry(&st.api_keys);
    Json(json!({
        "status": "ok",
        "engine": "exo-suit-rust",
        "active_project": st.active_project,
        "active_session_id": st.active_session_id,
        "agent_count": st.agents.len(),
        "providers": providers,
    }))
}

async fn ws_handler(ws: WebSocketUpgrade, State(ctx): State<ServerContext>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, ctx))
}

pub async fn generate_snapshot_json(state: &Arc<RwLock<SharedAppState>>) -> String {
    let st = state.read().await;
    let sessions = get_past_sessions(25);
    let artifacts = get_session_artifacts(&st.active_session_id);
    let file_tree = list_directory_contents(&st.active_project);
    let providers = build_provider_registry(&st.api_keys);
    let skills = get_installed_skills();
    let tools = get_tool_catalog();
    let cached_test = st.devops_tests.get(&st.active_project).cloned();
    let devops = build_workspace_devops_snapshot(&st.active_project, cached_test, 9000);
    let available_projects = list_workspace_projects();

    json!({
        "type": "TELEMETRY_SNAPSHOT",
        "timestamp": now_iso_short(),
        "project_id": st.active_project,
        "root_dir": code_root_dir().to_string_lossy(),
        "active_session_id": st.active_session_id,
        "active_run_count": st.active_runs.len(),
        "available_projects": available_projects,
        "providers": providers,
        "sessions": sessions,
        "skills": skills,
        "tools": tools,
        "artifacts": artifacts,
        "file_browser": file_tree,
        "devops": devops,
        "agents": st.agents,
        "council": st.council,
        "chat_thread": st.chat_thread,
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
    direct_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) {
    let Ok(cmd) = serde_json::from_str::<ClientCommand>(raw_txt) else {
        return;
    };

    match cmd.command.as_str() {
            "BROWSE_DIR" => {
                let path = cmd.path.unwrap_or_default();
                let tree = list_directory_contents(&path);
                let top_slug = path
                    .trim_matches('/')
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let mut project_switched: Option<(
                    String,
                    Vec<models::AgentNode>,
                    Option<models::TestRunSnapshot>,
                )> = None;
                if !top_slug.is_empty() && !top_slug.starts_with('.') {
                    let candidate_dir = code_root_dir().join(&top_slug);
                    if candidate_dir.is_dir() {
                        let mut st = ctx.state.write().await;
                        if st.active_project != top_slug {
                            st.active_project = top_slug.clone();
                            save_session_project_id(&st.active_session_id, &top_slug);
                            st.agents =
                                build_live_session_graph(&st.active_session_id, &top_slug);
                            let cached_test = st.devops_tests.get(&top_slug).cloned();
                            project_switched =
                                Some((top_slug, st.agents.clone(), cached_test));
                        }
                    }
                }
                let _ = direct_tx.send(
                    json!({
                        "type": "DIRECTORY_CONTENTS",
                        "file_browser": tree
                    })
                    .to_string(),
                );
                if let Some((new_proj, updated_agents, cached_test)) = project_switched {
                    let sessions = get_past_sessions(25);
                    let devops =
                        build_workspace_devops_snapshot(&new_proj, cached_test, 9000);
                    let _ = ctx.tx.send(
                        json!({
                            "type": "PROJECT_CONTEXT_UPDATED",
                            "project_id": new_proj,
                            "agents": updated_agents,
                            "sessions": sessions,
                            "devops": devops
                        })
                        .to_string(),
                    );
                }
            }
            "OPEN_FILE" => {
                let path = cmd.path.unwrap_or_default();
                let (s_id, proj) = {
                    let st = ctx.state.read().await;
                    (st.active_session_id.clone(), st.active_project.clone())
                };
                if let Some(f) = read_bounded_file(&path, &s_id, &proj) {
                    let _ = direct_tx.send(
                        json!({
                            "type": "FILE_OPENED",
                            "file": f
                        })
                        .to_string(),
                    );
                }
            }
            "REFRESH_DEVOPS" => {
                let (proj, cached_test) = {
                    let st = ctx.state.read().await;
                    let p = cmd
                        .project_id
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| st.active_project.clone());
                    let ct = st.devops_tests.get(&p).cloned();
                    (p, ct)
                };
                let devops = build_workspace_devops_snapshot(&proj, cached_test, 9000);
                let _ = direct_tx.send(
                    json!({
                        "type": "DEVOPS_UPDATED",
                        "devops": devops
                    })
                    .to_string(),
                );
            }
            "OPEN_GIT_DIFF" => {
                let proj = {
                    let st = ctx.state.read().await;
                    cmd.project_id
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| st.active_project.clone())
                };
                let diff_payload = compute_workspace_diff(&proj, cmd.path.as_deref());
                let _ = direct_tx.send(
                    json!({
                        "type": "GIT_DIFF_OPENED",
                        "git_diff": diff_payload
                    })
                    .to_string(),
                );
            }
            "RUN_WORKSPACE_TESTS" => {
                let proj = {
                    let st = ctx.state.read().await;
                    cmd.project_id
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| st.active_project.clone())
                };
                tokio::spawn(run_workspace_tests(
                    ctx.state.clone(),
                    ctx.tx.clone(),
                    proj,
                    9000,
                ));
            }
            "SET_PROJECT" => {
                let raw_proj = cmd.path.unwrap_or_else(|| "exo-suit".to_string());
                let proj = raw_proj
                    .trim_matches('/')
                    .split('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("exo-suit")
                    .to_string();
                {
                    let mut st = ctx.state.write().await;
                    st.active_project = proj.clone();
                    save_session_project_id(&st.active_session_id, &proj);
                    st.agents = build_live_session_graph(&st.active_session_id, &proj);
                }
                let snap = generate_snapshot_json(&ctx.state).await;
                let _ = ctx.tx.send(snap);
            }
            "SELECT_SESSION" => {
                let s_id = cmd.session_id.unwrap_or_default();
                let inferred_proj = infer_session_project_id(&s_id);
                let file_tree = list_directory_contents(&inferred_proj);
                let (artifacts, agents, chat_thread, cached_test) = {
                    let mut st = ctx.state.write().await;
                    st.active_session_id = s_id.clone();
                    st.active_project = inferred_proj.clone();
                    st.council = None;
                    let merged_chat = st.load_merged_session_chat(&s_id);
                    st.agents = build_live_session_graph(&s_id, &inferred_proj);
                    let ct = st.devops_tests.get(&inferred_proj).cloned();
                    (
                        get_session_artifacts(&s_id),
                        st.agents.clone(),
                        merged_chat,
                        ct,
                    )
                };
                let devops =
                    build_workspace_devops_snapshot(&inferred_proj, cached_test, 9000);
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
            "RENAME_SESSION" => {
                let s_id = cmd.session_id.unwrap_or_default();
                let new_title = cmd.title.unwrap_or_default();
                rename_session(&s_id, &new_title);
                let sessions = get_past_sessions(25);
                let _ = ctx.tx.send(
                    json!({
                        "type": "SESSIONS_UPDATED",
                        "sessions": sessions
                    })
                    .to_string(),
                );
            }
            "CREATE_SESSION_OR_PROJECT" => {
                let mode = cmd.mode.unwrap_or_else(|| "existing_repo".to_string());
                let raw_proj = cmd.project_id.unwrap_or_default();
                let init_git = cmd.init_git.unwrap_or(true);
                let auto_kickoff = cmd.auto_kickoff.unwrap_or(false);
                let desc_clone = cmd.description.clone();

                match provision_workspace_and_session(
                    &mode,
                    &raw_proj,
                    cmd.title.as_deref(),
                    cmd.description.as_deref(),
                    init_git,
                ) {
                    Ok(prov) => {
                        let new_sid = prov.session_id.clone();
                        let new_proj = prov.project_id.clone();
                        let file_tree = list_directory_contents(&new_proj);
                        let (artifacts, agents, chat_thread, cached_test) = {
                            let mut st = ctx.state.write().await;
                            st.active_session_id = new_sid.clone();
                            st.active_project = new_proj.clone();
                            st.council = None;
                            let merged_chat = st.load_merged_session_chat(&new_sid);
                            st.agents = build_live_session_graph(&new_sid, &new_proj);
                            let ct = st.devops_tests.get(&new_proj).cloned();
                            (
                                get_session_artifacts(&new_sid),
                                st.agents.clone(),
                                merged_chat,
                                ct,
                            )
                        };
                        let sessions = get_past_sessions(25);
                        let available_projects = list_workspace_projects();
                        let devops =
                            build_workspace_devops_snapshot(&new_proj, cached_test, 9000);
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

                        if auto_kickoff {
                            let provider = cmd
                                .provider
                                .unwrap_or_else(|| "antigravity".to_string());
                            let model = cmd
                                .model
                                .unwrap_or_else(|| "gemini-4-argon".to_string());
                            let desc_summary = desc_clone
                                .map(|d| d.trim().to_string())
                                .filter(|d| !d.is_empty());
                            let kickoff_prompt = if let Some(d) = desc_summary {
                                format!(
                                    "/greenfield-architecture Design and decompose the 8-Pillar architecture for ~/code/{}: {}",
                                    new_proj, d
                                )
                            } else {
                                format!(
                                    "/greenfield-architecture Audit ~/code/{} and initiate Day-1 8-Pillar architecture discovery.",
                                    new_proj
                                )
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
                    }
                    Err(err_msg) => {
                        let _ = direct_tx.send(
                            json!({
                                "type": "SESSION_CREATE_ERROR",
                                "error": err_msg
                            })
                            .to_string(),
                        );
                    }
                }
            }
            "APPROVE_ARTIFACT" => {
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
            "APPROVE_TOOL_EXECUTION" => {
                let agent_id = cmd.agent_id.unwrap_or_default();
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
                let _ = ctx.tx.send(
                    json!({
                        "type": "AGENTS_UPDATED",
                        "agents": st.agents
                    })
                    .to_string(),
                );
            }
            "SET_API_KEYS" => {
                if let Some(keys) = cmd.api_keys {
                    let mut st = ctx.state.write().await;
                    for (k, v) in keys {
                        if !v.is_empty() {
                            st.api_keys.insert(k, v);
                        }
                    }
                    let providers = build_provider_registry(&st.api_keys);
                    let _ = ctx.tx.send(
                        json!({
                            "type": "PROVIDERS_UPDATED",
                            "providers": providers
                        })
                        .to_string(),
                    );
                }
            }
            "SUBMIT_PROMPT" => {
                let prompt = cmd.prompt.unwrap_or_default();
                let provider = cmd.provider.unwrap_or_else(|| "antigravity".to_string());
                let model = cmd.model;
                let session_id = cmd.session_id;
                let project_id = cmd.project_id;
                if !prompt.is_empty() {
                    tokio::spawn(run_independent_prompt(
                        ctx.state.clone(),
                        ctx.tx.clone(),
                        provider,
                        model,
                        prompt,
                        session_id,
                        project_id,
                    ));
                }
            }
            "CANCEL_EXECUTION" => {
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
            "ANSWER_QUESTION" => {
                let s_id = cmd.session_id.unwrap_or_default();
                let prompt_id = cmd.prompt_id.unwrap_or_default();
                let answers = cmd.answers.unwrap_or_default();
                let provider = cmd.provider.unwrap_or_else(|| "antigravity".to_string());
                let model = cmd.model;
                if !prompt_id.is_empty() && !answers.is_empty() {
                    tokio::spawn(submit_interactive_answer(
                        ctx.state.clone(),
                        ctx.tx.clone(),
                        s_id,
                        prompt_id,
                        answers,
                        provider,
                        model,
                    ));
                }
            }
            "START_COUNCIL" => {
                let prompt = cmd.prompt.unwrap_or_default();
                let providers = cmd.providers.unwrap_or_default();
                let session_id = cmd.session_id;
                let project_id = cmd.project_id;
                if !prompt.is_empty() {
                    tokio::spawn(run_council_session(
                        ctx.state.clone(),
                        ctx.tx.clone(),
                        providers,
                        prompt,
                        session_id,
                        project_id,
                    ));
                }
            }
            _ => {}
        }
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
        let mut last_transcript_len: u64 = 0;

        loop {
            let notified =
                tokio::time::timeout(Duration::from_millis(300), watch_rx.recv())
                    .await
                    .is_ok();
            while watch_rx.try_recv().is_ok() {}

            let (s_id, proj, council_active) = {
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
                )
            };

            if council_active {
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

            if !notified && current_len == last_transcript_len {
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
            let devops = build_workspace_devops_snapshot(&proj, cached_test, 9000);
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
