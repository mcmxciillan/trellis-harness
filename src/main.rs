use exo_suit::{
    bootstrap_shared_state, brain::brain_dir, build_router, set_runtime_port,
    spawn_brain_watcher, ServerContext,
};
use std::net::SocketAddr;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9000);
    set_runtime_port(port);

    let shared_state = bootstrap_shared_state(port);
    let (tx, _rx) = broadcast::channel::<String>(256);

    // Spawn real-time filesystem watcher on ~/.gemini/antigravity-cli/brain
    spawn_brain_watcher(shared_state.clone(), tx.clone());

    let ctx = ServerContext {
        state: shared_state,
        tx,
    };

    let app = build_router(ctx);
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
