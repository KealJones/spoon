pub mod debug;
pub mod openai;

use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use spoon_mind::brain::Brain;

/// Build the application router. Exposed for tests (bind on a random port).
pub fn router(brain: Arc<Brain>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(openai::chat_completions))
        .route("/v1/models", get(openai::list_models))
        .route("/debug/metrics", get(debug::metrics_handler))
        .route("/debug/snapshot", get(debug::snapshot_handler))
        .route("/debug/counts", get(debug::counts_handler))
        .route("/debug/facts", get(debug::facts_handler))
        .route("/debug/actions", get(debug::actions_handler))
        .route("/debug/concepts", get(debug::concepts_handler))
        .route("/debug/pairs", get(debug::pairs_handler))
        .route("/debug/stances", get(debug::stances_handler))
        .route("/debug/episodes", get(debug::episodes_handler))
        .route("/debug/kv", get(debug::kv_handler))
        .route("/health", get(debug::health_handler))
        .route("/", get(debug::inspector_handler))
        .with_state(brain)
        .layer(CorsLayer::permissive())
}

/// Bind and serve forever (used by the `serve` subcommand).
pub async fn serve(brain: Arc<Brain>, host: &str, port: u16) -> anyhow::Result<()> {
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, router(brain)).await?;
    Ok(())
}
