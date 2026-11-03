use exo_suit::{
    brain::{
        brain_dir, build_live_session_graph, get_past_sessions, get_session_chat_thread,
        infer_session_project_id,
    },
    build_router,
    council::SharedAppState,
    spawn_brain_watcher, ServerContext,
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::sync::{broadcast, RwLock};

#[tokio::main]
async fn main() {
    let sessions = get_past_sessions(25);
    let active_session_id = sessions
        .first()
        .map(|s| s.session_id.clone())
        .unwrap_or_else(|| "eae8f37c-4de6-4200-b86b-66ca0bb8edc7".to_string());

    let active_project = infer_session_project_id(&active_session_id);
    let initial_agents = build_live_session_graph(&active_session_id, &active_project);
    let initial_chat = get_session_chat_thread(&active_session_id);

    let shared_state = Arc::new(RwLock::new(SharedAppState {
        active_project,
        active_session_id,
        agents: initial_agents,
        council: None,
        chat_thread: initial_chat,
        custom_chat_messages: Vec::new(),
        session_custom_messages: HashMap::new(),
        active_runs: HashMap::new(),
        devops_tests: HashMap::new(),
        api_keys: HashMap::new(),
    }));

    let (tx, _rx) = broadcast::channel::<String>(256);

    // Spawn real-time filesystem watcher on ~/.gemini/antigravity-cli/brain
    spawn_brain_watcher(shared_state.clone(), tx.clone());

    let ctx = ServerContext {
        state: shared_state,
        tx,
    };

    let app = build_router(ctx);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    println!("  ┌─ trellis :: ai-harness v1.0.0");
    println!("  │");
    println!("  ├── [lattice] loading scaffold topology... ok");
    println!(
        "  ├── [root] binding model context ({})... connected",
        brain_dir().display()
    );
    println!(
        "  └── [yield] pipeline ready on port {} (http://127.0.0.1:{})",
        port, port
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind port");
    axum::serve(listener, app).await.expect("Server error");
}
