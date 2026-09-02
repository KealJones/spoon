//! Read-only JSON views over the Brain for the inspector page. Every listing
//! takes `?q=` (substring filter), `?limit=` and `?offset=`; store failures
//! come back as `500 {"error": ...}` and never panic the server.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Response},
};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use spoon_core::can::Can;
use spoon_core::store::Store;
use spoon_core::types::*;
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

// ---- paging -----------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub q: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

impl ListQuery {
    fn q(&self) -> Option<&str> {
        self.q.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
    fn limit(&self) -> usize {
        self.limit.unwrap_or(100).clamp(1, 1000)
    }
    fn offset(&self) -> usize {
        self.offset.unwrap_or(0)
    }
    /// Case-insensitive substring test for the in-memory (CAN) listings.
    fn matches(&self, haystack: &str) -> bool {
        match self.q() {
            None => true,
            Some(q) => haystack.to_lowercase().contains(&q.to_lowercase()),
        }
    }
}

fn respond(result: anyhow::Result<Vec<JsonValue>>) -> Response {
    match result {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => store_error(e),
    }
}

fn to_rows<T: serde::Serialize>(rows: Vec<T>) -> anyhow::Result<Vec<JsonValue>> {
    rows.iter().map(|r| Ok(serde_json::to_value(r)?)).collect()
}

fn store_error(e: anyhow::Error) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response()
}

// ---- handlers ---------------------------------------------------------------

pub async fn counts_handler(State(brain): State<Arc<Brain>>) -> Response {
    let counts = brain.store.lock().counts();
    match counts {
        Ok(c) => Json(c).into_response(),
        Err(e) => store_error(e),
    }
}

pub async fn facts_handler(State(brain): State<Arc<Brain>>, Query(p): Query<ListQuery>) -> Response {
    respond(facts(&brain, &p))
}

pub async fn actions_handler(State(brain): State<Arc<Brain>>, Query(p): Query<ListQuery>) -> Response {
    respond(actions(&brain, &p))
}

pub async fn concepts_handler(State(brain): State<Arc<Brain>>, Query(p): Query<ListQuery>) -> Response {
    respond(concepts(&brain, &p))
}

pub async fn pairs_handler(State(brain): State<Arc<Brain>>, Query(p): Query<ListQuery>) -> Response {
    let rows = brain.store.lock().list_pairs(p.q(), p.limit(), p.offset());
    respond(rows.and_then(to_rows))
}

pub async fn stances_handler(State(brain): State<Arc<Brain>>, Query(p): Query<ListQuery>) -> Response {
    let rows = brain.store.lock().list_stances(p.q(), p.limit(), p.offset());
    respond(rows.and_then(to_rows))
}

pub async fn episodes_handler(State(brain): State<Arc<Brain>>, Query(p): Query<ListQuery>) -> Response {
    let rows = brain.store.lock().list_episodes(p.q(), p.limit(), p.offset());
    respond(rows.and_then(|rows| rows.iter().map(episode_row).collect()))
}

pub async fn kv_handler(State(brain): State<Arc<Brain>>, Query(p): Query<ListQuery>) -> Response {
    let rows = brain.store.lock().list_kv(p.q(), p.limit(), p.offset());
    respond(rows.map(|rows| rows.into_iter().map(|(key, value)| json!({"key": key, "value": value})).collect()))
}

// ---- rows -------------------------------------------------------------------

/// Lock order is can -> store (see `Brain`); the realizer needs both.
fn facts(brain: &Brain, p: &ListQuery) -> anyhow::Result<Vec<JsonValue>> {
    let can = brain.can.lock();
    let store = brain.store.lock();
    let facts = store.list_facts(p.q(), p.limit(), p.offset())?;
    Ok(facts.iter().map(|f| fact_row(f, &can, &store)).collect())
}

fn fact_row(f: &Fact, can: &Can, store: &Store) -> JsonValue {
    let relation = f.pred.0.strip_prefix("rel.").unwrap_or(&f.pred.0);
    let args: Vec<String> = f.args.iter().map(Value::render).collect();
    let modal = f.modal.as_deref().map(|m| format!("{m} ")).unwrap_or_default();
    let not = if f.truth { "" } else { "not " };
    json!({
        "id": f.id,
        "relation": relation,
        "pred": f.pred.0,
        "args": args,
        "text": format!("{modal}{not}{relation}({})", args.join(", ")),
        "sce": spoon_mind::discourse::realize_fact(can, store, f),
        "truth": f.truth,
        "modal": f.modal,
        "source": f.source,
        "at": f.asserted_at,
        "superseded": f.invalidated_at.is_some(),
        "invalidated_at": f.invalidated_at,
        "episode_id": f.episode_id,
    })
}

/// Actions come from the live CAN so kernel primitives show up next to the
/// learned ones; `updated_at` is joined in from the store and is null for
/// kernel actions, which are never persisted. Learned first, newest first.
fn actions(brain: &Brain, p: &ListQuery) -> anyhow::Result<Vec<JsonValue>> {
    let can = brain.can.lock();
    let stamps = brain.store.lock().action_updated_at()?;
    let mut rows: Vec<&Action> = can
        .actions()
        .filter(|a| {
            p.matches(&format!(
                "{} {} {} {} {:?}",
                a.id.0,
                a.verbs.join(" "),
                a.phrasings.join(" "),
                a.description,
                a.tier
            ))
        })
        .collect();
    rows.sort_by_key(|a| (a.tier == Tier::Kernel, std::cmp::Reverse(stamps.get(&a.id.0).copied()), a.id.0.clone()));
    Ok(rows
        .into_iter()
        .skip(p.offset())
        .take(p.limit())
        .map(|a| action_row(a, stamps.get(&a.id.0).copied()))
        .collect())
}

fn action_row(a: &Action, updated_at: Option<i64>) -> JsonValue {
    let (program, ir) = match &a.imp {
        Impl::Program { program } => (Some(spoon_mind::grow::describe(program)), serde_json::to_value(program).ok()),
        Impl::Primitive => (None, None),
    };
    let inputs: Vec<JsonValue> = a
        .inputs
        .iter()
        .map(|i| json!({"name": i.name, "ty": i.ty.to_string(), "required": i.required}))
        .collect();
    let params: Vec<String> = a.inputs.iter().map(|i| format!("{}: {}", i.name, i.ty)).collect();
    json!({
        "id": a.id.0,
        "verbs": a.verbs,
        "tier": format!("{:?}", a.tier),
        "role": format!("{:?}", a.role),
        "effect": format!("{:?}", a.effect),
        "inputs": inputs,
        "output": a.output.to_string(),
        "signature": format!("({}) -> {}", params.join(", "), a.output),
        "program": program,
        "ir": ir,
        "uses": a.stats.uses,
        "successes": a.stats.successes,
        "failures": a.stats.failures,
        "last_used": a.stats.last_used,
        "phrasings": a.phrasings,
        "description": a.description,
        "provenance": a.provenance,
        "updated_at": updated_at,
    })
}

/// Concepts come from the live CAN like actions. `examples` are up to five
/// entities with an `is_a` fact for the concept.
fn concepts(brain: &Brain, p: &ListQuery) -> anyhow::Result<Vec<JsonValue>> {
    let can = brain.can.lock();
    let store = brain.store.lock();
    let stamps = store.concept_updated_at()?;
    let mut rows: Vec<&Concept> = can
        .concepts()
        .filter(|c| p.matches(&format!("{} {} {} {:?}", c.id.0, c.nouns.join(" "), c.description, c.tier)))
        .collect();
    rows.sort_by_key(|c| (c.tier == Tier::Kernel, std::cmp::Reverse(stamps.get(&c.id.0).copied()), c.id.0.clone()));
    let is_a = ActionId("rel.is_a".into());
    rows.into_iter()
        .skip(p.offset())
        .take(p.limit())
        .map(|c| -> anyhow::Result<JsonValue> {
            let examples: Vec<String> = match c.kind {
                ConceptKind::Entity => store
                    .query_facts(&is_a, &[None, Some(Value::Name(c.id.0.clone()))])?
                    .iter()
                    .take(5)
                    .filter_map(|f| f.args.first().map(Value::render))
                    .collect(),
                _ => vec![],
            };
            Ok(concept_row(c, examples, stamps.get(&c.id.0).copied()))
        })
        .collect()
}

fn concept_row(c: &Concept, examples: Vec<String>, updated_at: Option<i64>) -> JsonValue {
    let (kind, detail) = match &c.kind {
        ConceptKind::Primitive { ty } => ("primitive", json!(ty.to_string())),
        ConceptKind::Structure { properties } => (
            "structure",
            json!(properties.iter().map(|p| format!("{}: {}", p.name, p.ty)).collect::<Vec<_>>()),
        ),
        ConceptKind::Enum { symbols } => ("enum", json!(symbols)),
        ConceptKind::Entity => ("entity", JsonValue::Null),
    };
    json!({
        "id": c.id.0,
        "kind": kind,
        "detail": detail,
        "parents": c.extends.iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
        "role_of": c.role_of.as_ref().map(|r| r.0.clone()),
        "nouns": c.nouns,
        "examples": examples,
        "tier": format!("{:?}", c.tier),
        "description": c.description,
        "provenance": c.provenance,
        "updated_at": updated_at,
    })
}

/// The stored episode plus two derived fields: `ears_path` lifted out of the
/// metrics, and `mouth_path` read off the mouth LLM call count (0 template,
/// 1 llm, 2+ llm_retried), since the episode does not record it directly.
fn episode_row(ep: &Episode) -> anyhow::Result<JsonValue> {
    let mut row = serde_json::to_value(ep)?;
    if let Some(obj) = row.as_object_mut() {
        obj.insert("ears_path".into(), json!(ep.metrics.ears_path));
        let mouth_path = match ep.metrics.mouth_llm_calls {
            0 => "template",
            1 => "llm",
            _ => "llm_retried",
        };
        obj.insert("mouth_path".into(), json!(mouth_path));
    }
    Ok(row)
}
