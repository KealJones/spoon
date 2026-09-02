use std::sync::Arc;
use axum::{extract::State, response::{Html, IntoResponse, Json}};
use serde_json::json;
use spoon_mind::brain::Brain;

pub async fn metrics_handler(State(brain): State<Arc<Brain>>) -> impl IntoResponse {
    Json(brain.metrics())
}

pub async fn snapshot_handler(State(brain): State<Arc<Brain>>) -> impl IntoResponse {
    Json(brain.snapshot())
}

pub async fn health_handler() -> impl IntoResponse {
    Json(json!({"ok": true}))
}

pub async fn inspector_handler() -> impl IntoResponse {
    Html(include_str!("inspector.html"))
}
