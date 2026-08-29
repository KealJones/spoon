use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{Html, IntoResponse, Json};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use spoon_core::{InterpretationProposal, SessionId, SessionVisibility, Value as SpoonValue};
use spoon_engine::{
    CycleBudget, CycleInput, CycleProgress, Engine, EngineError, IntentProposalWire,
    IntentRequestWire, RecallMode, TeacherProposalWire,
};
use spoon_episode::EpisodeQuery;

use crate::RpcServer;

// ---------- Teacher / Interpreter LLM config ----------

#[derive(Clone)]
pub struct TeacherConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl TeacherConfig {
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("SPOON_TEACHER_URL")
            .or_else(|_| std::env::var("SPOON_OLLAMA_URL"))
            .unwrap_or_else(|_| "http://localhost:11434".into());
        let model = std::env::var("SPOON_TEACHER_MODEL").unwrap_or_else(|_| "qwen2.5:1.5b".into());
        let api_key = std::env::var("SPOON_TEACHER_API_KEY").ok();
        Some(Self {
            base_url,
            model,
            api_key,
        })
    }
}

/// Wraps engine access on a dedicated thread since rusqlite::Connection is !Send.
/// Requests are dispatched via a channel; the engine thread processes them
/// sequentially but returns quickly (engine ops are CPU-bound milliseconds).
/// Concurrent HTTP requests queue here and the slow parts (LLM calls) happen
/// outside this channel.
#[derive(Clone)]
pub struct EngineHandle {
    tx: tokio::sync::mpsc::Sender<EngineRequest>,
}

type EngineResponse = Result<Value, String>;
struct EngineRequest {
    op: EngineOp,
    reply: tokio::sync::oneshot::Sender<EngineResponse>,
}

enum EngineOp {
    Cycle(CycleInput),
    Teach {
        situation: String,
        proposal: TeacherProposalWire,
    },
    Assist {
        situation: String,
        hints: Value,
    },
    ListSessions,
    CreateSession {
        name: Option<String>,
        visibility: SessionVisibility,
    },
    EndSession(String),
    ListEpisodes {
        session_id: Option<String>,
        limit: u32,
    },
}

impl EngineHandle {
    fn spawn(mut server: RpcServer, teacher: Option<TeacherConfig>) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<EngineRequest>(256);
        // Build the blocking HTTP client outside the tokio runtime to avoid
        // drop-in-async-context panics (reqwest::blocking creates its own runtime).
        let http_client = std::thread::spawn(|| {
            reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("failed to build HTTP client")
        })
        .join()
        .expect("client builder thread panicked");
        std::thread::spawn(move || {
            while let Some(req) = rx.blocking_recv() {
                let result = match req.op {
                    EngineOp::Cycle(input) => {
                        run_engine_cycle(&mut server.engine, input, &teacher, &http_client)
                            .map_err(|e| e.to_string())
                    }
                    EngineOp::Teach {
                        situation,
                        proposal,
                    } => run_teach(&mut server.engine, situation, proposal)
                        .map_err(|e| e.to_string()),
                    EngineOp::Assist { situation, hints } => {
                        run_assist(&teacher, &http_client, &situation, &hints)
                    }
                    EngineOp::ListSessions => server
                        .engine
                        .list_sessions()
                        .map(|s| serde_json::to_value(s).unwrap_or_default())
                        .map_err(|e| e.to_string()),
                    EngineOp::CreateSession { name, visibility } => server
                        .engine
                        .create_session(name, visibility)
                        .map(|s| serde_json::to_value(s).unwrap_or_default())
                        .map_err(|e| e.to_string()),
                    EngineOp::EndSession(id) => server
                        .engine
                        .end_session(&id)
                        .map(|s| serde_json::to_value(s).unwrap_or_default())
                        .map_err(|e| e.to_string()),
                    EngineOp::ListEpisodes { session_id, limit } => {
                        let query = EpisodeQuery {
                            session_id: session_id
                                .and_then(|id| Uuid::parse_str(&id).ok().map(SessionId)),
                            limit,
                            ..EpisodeQuery::default()
                        };
                        server
                            .engine
                            .episodes()
                            .query(&query)
                            .map(|e| serde_json::to_value(e).unwrap_or_default())
                            .map_err(|e| e.to_string())
                    }
                };
                let _ = req.reply.send(result);
            }
        });
        Self { tx }
    }

    async fn send(&self, op: EngineOp) -> EngineResponse {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(EngineRequest {
                op,
                reply: reply_tx,
            })
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        reply_rx
            .await
            .map_err(|_| "engine reply dropped".to_string())?
    }
}

#[derive(Clone)]
pub struct HttpState {
    engine: EngineHandle,
    teacher: Option<TeacherConfig>,
}

// ---------- OpenAI-compatible types ----------

#[derive(Deserialize)]
struct ChatCompletionRequest {
    #[serde(default = "default_model")]
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    max_tokens: Option<u32>,
    /// Spoon extension: session ID for conversation continuity
    #[serde(default)]
    spoon_session_id: Option<String>,
    /// Spoon extension: recall mode
    #[serde(default)]
    spoon_recall_mode: Option<String>,
    /// Spoon extension: whether to include the decision path in streaming
    #[serde(default)]
    spoon_include_decision_path: bool,
}

fn default_model() -> String {
    "spoon".into()
}

#[derive(Deserialize, Serialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: &'static str,
    created: u64,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Serialize)]
struct Choice {
    index: u32,
    message: ChatMessage,
    finish_reason: String,
}

#[derive(Serialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Serialize)]
struct ChatCompletionChunk {
    id: String,
    object: &'static str,
    created: u64,
    model: String,
    choices: Vec<ChunkChoice>,
}

#[derive(Serialize)]
struct ChunkChoice {
    index: u32,
    delta: ChunkDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
}

#[derive(Serialize)]
struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Serialize)]
struct ModelInfo {
    id: String,
    object: &'static str,
    created: u64,
    owned_by: String,
}

#[derive(Serialize)]
struct ModelList {
    object: &'static str,
    data: Vec<ModelInfo>,
}

// ---------- Spoon-specific API types ----------

#[derive(Deserialize)]
struct SpoonCycleRequest {
    situation: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    teacher_allowed: Option<bool>,
    #[serde(default)]
    interpreter_allowed: Option<bool>,
    #[serde(default)]
    recall_mode: Option<String>,
    #[serde(default)]
    max_exec_steps: Option<u32>,
    #[serde(default)]
    max_teacher_turns: Option<u32>,
    #[serde(default)]
    environment: Option<BTreeMap<String, SpoonValue>>,
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Deserialize)]
struct ListEpisodesQuery {
    session_id: Option<String>,
    limit: Option<u32>,
}

// ---------- Router ----------

pub fn router(server: RpcServer) -> Router {
    let teacher = TeacherConfig::from_env();
    if let Some(ref t) = teacher {
        eprintln!("Teacher configured: {} (model: {})", t.base_url, t.model);
    } else {
        eprintln!("No teacher configured - cycles will abstain when knowledge is missing");
    }
    let state = HttpState {
        engine: EngineHandle::spawn(server, teacher.clone()),
        teacher,
    };
    Router::new()
        .route("/", get(chat_page))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/api/cycle", post(run_cycle))
        .route("/api/teach", post(teach_procedure))
        .route("/api/teach/assist", post(teach_assist))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}/end", post(end_session))
        .route("/api/episodes", get(list_episodes))
        .route("/api/runtime", get(runtime_status))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

pub async fn serve(server: RpcServer, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let app = router(server);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    eprintln!("Spoon HTTP server listening at http://127.0.0.1:{port}");
    eprintln!("Chat UI: http://127.0.0.1:{port}/");
    eprintln!("OpenAI-compatible API: http://127.0.0.1:{port}/v1/chat/completions");
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------- Handlers ----------

async fn list_models() -> Json<ModelList> {
    Json(ModelList {
        object: "list",
        data: vec![ModelInfo {
            id: "spoon".into(),
            object: "model",
            created: now_unix(),
            owned_by: "spoon".into(),
        }],
    })
}

async fn chat_completions(
    State(state): State<HttpState>,
    Json(req): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    let situation = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default();

    if situation.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "no user message found", "type": "invalid_request_error"}})),
        )
            .into_response();
    }

    let recall_mode = match req.spoon_recall_mode.as_deref() {
        Some("session") => RecallMode::Session,
        Some("none") => RecallMode::None,
        _ => RecallMode::Global,
    };

    let input = CycleInput {
        situation,
        working_directory: None,
        environment: BTreeMap::new(),
        assumptions: Vec::new(),
        budget: CycleBudget {
            max_exec_steps: req.max_tokens.unwrap_or(10_000),
            max_context_items: 64,
            max_teacher_turns: 2,
        },
        teacher_allowed: true,
        interpreter_allowed: true,
        session_id: req.spoon_session_id.clone(),
        recall_mode,
        permission_mode: None,
    };

    let completion_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());

    if req.stream {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(32);
        let engine = state.engine.clone();
        let comp_id = completion_id.clone();
        let model = req.model.clone();

        tokio::spawn(async move {
            let send_chunk = |id: String,
                              m: String,
                              content: Option<String>,
                              role: Option<String>,
                              finish: Option<String>| {
                Event::default().data(
                    serde_json::to_string(&ChatCompletionChunk {
                        id,
                        object: "chat.completion.chunk",
                        created: now_unix(),
                        model: m,
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: ChunkDelta { role, content },
                            finish_reason: finish,
                        }],
                    })
                    .unwrap(),
                )
            };

            let _ = tx
                .send(Ok(send_chunk(
                    comp_id.clone(),
                    model.clone(),
                    None,
                    Some("assistant".into()),
                    None,
                )))
                .await;

            let result = engine.send(EngineOp::Cycle(input)).await;
            match result {
                Ok(answer) => {
                    let _ = tx
                        .send(Ok(send_chunk(
                            comp_id.clone(),
                            model.clone(),
                            Some(format_answer(&answer)),
                            None,
                            None,
                        )))
                        .await;
                }
                Err(error) => {
                    let _ = tx
                        .send(Ok(send_chunk(
                            comp_id.clone(),
                            model.clone(),
                            Some(format!("[error: {error}]")),
                            None,
                            None,
                        )))
                        .await;
                }
            }
            let _ = tx
                .send(Ok(send_chunk(
                    comp_id,
                    model,
                    None,
                    None,
                    Some("stop".into()),
                )))
                .await;
            let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
        });

        Sse::new(ReceiverStream::new(rx)).into_response()
    } else {
        let result = state.engine.send(EngineOp::Cycle(input)).await;

        match result {
            Ok(answer) => {
                let content = format_answer(&answer);
                Json(ChatCompletionResponse {
                    id: completion_id,
                    object: "chat.completion",
                    created: now_unix(),
                    model: req.model,
                    choices: vec![Choice {
                        index: 0,
                        message: ChatMessage {
                            role: "assistant".into(),
                            content,
                        },
                        finish_reason: "stop".into(),
                    }],
                    usage: Usage {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        total_tokens: 0,
                    },
                })
                .into_response()
            }
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"message": error, "type": "server_error"}})),
            )
                .into_response(),
        }
    }
}

/// Native Spoon cycle endpoint - returns full episode detail
async fn run_cycle(
    State(state): State<HttpState>,
    Json(req): Json<SpoonCycleRequest>,
) -> impl IntoResponse {
    let recall_mode = match req.recall_mode.as_deref() {
        Some("session") => RecallMode::Session,
        Some("none") => RecallMode::None,
        _ => RecallMode::Global,
    };

    let input = CycleInput {
        situation: req.situation,
        working_directory: None,
        environment: req.environment.unwrap_or_default(),
        assumptions: Vec::new(),
        budget: CycleBudget {
            max_exec_steps: req.max_exec_steps.unwrap_or(10_000),
            max_context_items: 64,
            max_teacher_turns: req.max_teacher_turns.unwrap_or(2),
        },
        teacher_allowed: req.teacher_allowed.unwrap_or(true),
        interpreter_allowed: req.interpreter_allowed.unwrap_or(true),
        session_id: req.session_id,
        recall_mode,
        permission_mode: None,
    };

    match state.engine.send(EngineOp::Cycle(input)).await {
        Ok(answer) => Json(answer).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
            .into_response(),
    }
}

fn llm_role_status(config: &Option<TeacherConfig>) -> Value {
    match config {
        Some(config) => json!({
            "adapter": "ollama",
            "model": config.model,
            "baseUrl": config.base_url,
        }),
        None => json!({
            "adapter": "off",
            "model": Value::Null,
            "baseUrl": Value::Null,
        }),
    }
}

async fn runtime_status(State(state): State<HttpState>) -> Json<Value> {
    Json(json!({
        "teacher": llm_role_status(&state.teacher),
        "interpreter": llm_role_status(&state.teacher),
    }))
}

async fn list_sessions(State(state): State<HttpState>) -> impl IntoResponse {
    match state.engine.send(EngineOp::ListSessions).await {
        Ok(sessions) => Json(sessions).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
            .into_response(),
    }
}

async fn create_session(
    State(state): State<HttpState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let visibility = match req.visibility.as_deref() {
        Some("isolated") => SessionVisibility::Isolated,
        _ => SessionVisibility::Global,
    };
    match state
        .engine
        .send(EngineOp::CreateSession {
            name: req.name,
            visibility,
        })
        .await
    {
        Ok(session) => Json(session).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
            .into_response(),
    }
}

async fn end_session(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.engine.send(EngineOp::EndSession(id)).await {
        Ok(session) => Json(session).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
            .into_response(),
    }
}

async fn list_episodes(
    State(state): State<HttpState>,
    Query(query): Query<ListEpisodesQuery>,
) -> impl IntoResponse {
    if let Some(session_id) = query.session_id.as_deref() {
        if Uuid::parse_str(session_id).is_err() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "session_id is not a UUID"})),
            )
                .into_response();
        }
    }
    let limit = query.limit.unwrap_or(200).min(1000);
    match state
        .engine
        .send(EngineOp::ListEpisodes {
            session_id: query.session_id,
            limit,
        })
        .await
    {
        Ok(episodes) => Json(episodes).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
            .into_response(),
    }
}

// ---------- Manual teach endpoint ----------

#[derive(Deserialize)]
struct TeachRequest {
    situation: String,
    proposal: Value,
}

async fn teach_procedure(
    State(state): State<HttpState>,
    Json(req): Json<TeachRequest>,
) -> impl IntoResponse {
    let proposal = teacher_proposal(
        req.proposal,
        "human:chat-builder".into(),
        "human",
        None,
        &req.situation,
    );
    match state
        .engine
        .send(EngineOp::Teach {
            situation: req.situation,
            proposal,
        })
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct AssistRequest {
    situation: String,
    #[serde(default)]
    hints: Value,
}

async fn teach_assist(
    State(state): State<HttpState>,
    Json(req): Json<AssistRequest>,
) -> impl IntoResponse {
    match state
        .engine
        .send(EngineOp::Assist {
            situation: req.situation,
            hints: req.hints,
        })
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
            .into_response(),
    }
}

fn run_assist(
    teacher: &Option<TeacherConfig>,
    http_client: &reqwest::blocking::Client,
    situation: &str,
    hints: &Value,
) -> EngineResponse {
    let Some(config) = teacher else {
        return Err("no teacher configured - set SPOON_TEACHER_URL and SPOON_TEACHER_MODEL".into());
    };

    let mut context_parts = vec![
        "The user is building a procedure in the visual IR builder.".to_string(),
        format!("Situation/trigger: {situation}"),
    ];
    if let Some(params) = hints.get("parameters").and_then(|v| v.as_array()) {
        let param_desc: Vec<String> = params
            .iter()
            .filter_map(|p| {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let vtype = p.get("valueType").and_then(|v| v.as_str()).unwrap_or("any");
                let desc = p.get("description").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    return None;
                }
                Some(format!("{name}: {vtype} - {desc}"))
            })
            .collect();
        if !param_desc.is_empty() {
            context_parts.push(format!(
                "Parameters already defined:\n{}",
                param_desc.join("\n")
            ));
        }
    }
    if let Some(concepts) = hints.get("concepts").and_then(|v| v.as_array()) {
        let concept_desc: Vec<String> = concepts
            .iter()
            .filter_map(|c| {
                let key = c.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if key.is_empty() && name.is_empty() {
                    return None;
                }
                Some(format!("{key}: {name}"))
            })
            .collect();
        if !concept_desc.is_empty() {
            context_parts.push(format!(
                "Concepts already defined: {}",
                concept_desc.join(", ")
            ));
        }
    }
    if let Some(notes) = hints.get("notes").and_then(|v| v.as_str()) {
        if !notes.is_empty() {
            context_parts.push(format!("Additional notes from the user: {notes}"));
        }
    }

    let context = json!({ "builderHints": context_parts.join("\n\n") });
    let desired_output = spoon_engine::proposal_schema();

    match call_teacher(
        config,
        http_client,
        situation,
        &context,
        Some(
            "Fill in the complete proposal based on the hints provided. The user will review and edit your suggestions in a visual builder.",
        ),
        &desired_output,
    ) {
        Ok(proposal) => Ok(proposal.content),
        Err(error) => Err(format!("teacher assist failed: {error}")),
    }
}

fn run_teach(
    engine: &mut Engine,
    situation: String,
    proposal: TeacherProposalWire,
) -> Result<Value, EngineError> {
    let input = CycleInput {
        situation,
        working_directory: None,
        environment: BTreeMap::new(),
        assumptions: Vec::new(),
        budget: CycleBudget {
            max_exec_steps: 10_000,
            max_context_items: 64,
            max_teacher_turns: 2,
        },
        teacher_allowed: true,
        interpreter_allowed: false,
        session_id: None,
        recall_mode: RecallMode::Global,
        permission_mode: None,
    };

    let mut progress = engine.begin_cycle(input)?;
    loop {
        match progress {
            CycleProgress::Completed(outcome) => {
                return Ok(json!({
                    "status": "completed",
                    "answer": outcome.answer,
                    "disposition": format!("{:?}", outcome.disposition),
                    "episode": outcome.episode,
                }));
            }
            CycleProgress::NeedIntent { cycle_id, .. } => {
                progress = engine.skip_intent_with_diagnostic(
                    cycle_id,
                    "manual teach - skipping interpreter",
                    None,
                )?;
            }
            CycleProgress::NeedTeacher { cycle_id, .. } => {
                progress = engine.resume_cycle(cycle_id, proposal)?;
                // After feeding the proposal, the cycle should complete.
                // If it asks for teacher again, something's wrong - abort.
                loop {
                    match progress {
                        CycleProgress::Completed(outcome) => {
                            return Ok(json!({
                                "status": "completed",
                                "answer": outcome.answer,
                                "disposition": format!("{:?}", outcome.disposition),
                                "episode": outcome.episode,
                            }));
                        }
                        CycleProgress::NeedTeacher { cycle_id, .. } => {
                            engine.abort_cycle(cycle_id, "manual teach exhausted")?;
                            return Ok(json!({
                                "status": "error",
                                "reason": "engine requested teacher again after manual proposal",
                            }));
                        }
                        CycleProgress::NeedIntent { cycle_id, .. } => {
                            progress = engine.skip_intent_with_diagnostic(
                                cycle_id,
                                "manual teach - skipping interpreter",
                                None,
                            )?;
                        }
                    }
                }
            }
        }
    }
}

// ---------- Engine cycle execution ----------

fn run_engine_cycle(
    engine: &mut Engine,
    input: CycleInput,
    teacher: &Option<TeacherConfig>,
    http_client: &reqwest::blocking::Client,
) -> Result<Value, EngineError> {
    let max_teacher_turns = input.budget.max_teacher_turns;
    let mut progress = engine.begin_cycle(input)?;
    let mut teacher_turns: u32 = 0;

    loop {
        match progress {
            CycleProgress::Completed(outcome) => {
                return Ok(json!({
                    "status": "completed",
                    "disposition": outcome.disposition,
                    "answer": outcome.answer,
                    "episode": {
                        "id": outcome.episode.id,
                        "situation": outcome.episode.situation,
                        "action": outcome.episode.action,
                        "cost": outcome.episode.cost,
                        "evaluation": outcome.episode.evaluation,
                        "reasoning_trace": outcome.episode.reasoning_trace,
                    },
                }));
            }
            CycleProgress::NeedIntent { cycle_id, request } => {
                let Some(config) = teacher else {
                    progress = engine.skip_intent_with_diagnostic(
                        cycle_id,
                        "no interpreter LLM configured - set SPOON_TEACHER_URL",
                        None,
                    )?;
                    continue;
                };
                match call_interpreter(config, http_client, &request) {
                    Ok(proposal) => {
                        progress = engine.resume_intent(cycle_id, proposal)?;
                    }
                    Err(error) => {
                        progress = engine.skip_intent_with_diagnostic(
                            cycle_id,
                            &format!("interpreter call failed: {error}"),
                            None,
                        )?;
                    }
                }
            }
            CycleProgress::NeedTeacher { cycle_id, request } => {
                let Some(config) = teacher else {
                    engine.abort_cycle(cycle_id, "no teacher configured")?;
                    return Ok(json!({
                        "status": "abstained",
                        "reason": "no teacher configured - set SPOON_TEACHER_URL and SPOON_TEACHER_MODEL",
                    }));
                };

                if teacher_turns >= max_teacher_turns.max(2) {
                    engine.abort_cycle(cycle_id, "teacher turn budget exhausted")?;
                    return Ok(json!({
                        "status": "abstained",
                        "reason": "teacher turn budget exhausted",
                    }));
                }
                teacher_turns += 1;

                match call_teacher(
                    config,
                    http_client,
                    &request.situation,
                    &request.context,
                    request.specific_question.as_deref(),
                    &request.desired_output,
                ) {
                    Ok(proposal) => {
                        progress = engine.resume_cycle(cycle_id, proposal)?;
                    }
                    Err(error) => {
                        engine.abort_cycle(cycle_id, &format!("teacher call failed: {error}"))?;
                        return Ok(json!({
                            "status": "error",
                            "reason": format!("teacher call failed: {error}"),
                        }));
                    }
                }
            }
        }
    }
}

// ---------- Teacher LLM call ----------

const TEACHER_SYSTEM_PROMPT: &str = concat!(
    "You are a Spoon teacher. Return JSON with source (spoonlang text) and interpretations ([] unless a known graph concept applies). ",
    "Author THIS situation. Do not copy prompt examples. Do not put tagged IR JSON in source. ",
    "kind reusable_lesson teaches an executable procedure. kind answer_only is a one-shot fact or phrase. ",
    "Stable general facts with no user-supplied input to transform (how many eyes, spelling a word) use answer_only. ",
    "Never invent ids. The engine compiles spoonlang. ",
    "Format example for a procedure, only if the user asked about doubling:\n",
    "kind reusable_lesson\n",
    "concept double: procedural\n",
    "  \"Twice a number\"\n",
    "proc double(x: number)\n",
    "  x * 2\n",
    "example double(7) => 14\n",
    "Format example for a fact:\n",
    "kind answer_only\n",
    "answer 2\n",
);

fn call_teacher(
    config: &TeacherConfig,
    client: &reqwest::blocking::Client,
    situation: &str,
    context: &Value,
    specific_question: Option<&str>,
    desired_output: &Value,
) -> Result<TeacherProposalWire, String> {
    let prompt = build_teacher_prompt(situation, context, specific_question, desired_output);
    let url = format!("{}/api/generate", config.base_url.trim_end_matches('/'));

    let mut request_body = json!({
        "model": config.model,
        "prompt": prompt,
        "system": TEACHER_SYSTEM_PROMPT,
        "stream": true,
        "keep_alive": "30m",
        "format": desired_output,
        "think": false,
    });

    if let Some(ref key) = config.api_key {
        request_body["options"] = json!({"api_key": key});
    }

    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .body(serde_json::to_string(&request_body).unwrap())
        .send()
        .map_err(|e| format!("teacher HTTP request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("teacher returned {status}: {body}"));
    }

    let body = response
        .text()
        .map_err(|e| format!("failed to read teacher response: {e}"))?;
    let content = parse_ollama_streaming_response(&body)?;

    let parsed: Value = serde_json::from_str(&content)
        .map_err(|e| format!("teacher response was not valid JSON: {e}\nraw: {content}"))?;

    Ok(teacher_proposal(
        parsed,
        format!("ollama:{}", config.model),
        "ollama",
        Some(&config.model),
        situation,
    ))
}

fn teacher_proposal(
    content: Value,
    source: String,
    provider: &str,
    model: Option<&str>,
    situation: &str,
) -> TeacherProposalWire {
    let mut provenance = json!({
        "provider": provider,
        "teacher": source,
        "requestId": Uuid::new_v4().to_string(),
        "generatedAt": chrono_now_iso(),
        "situation": situation,
    });
    if let Some(model) = model {
        provenance["model"] = json!(model);
    }
    TeacherProposalWire {
        content,
        source,
        status: "unverified".into(),
        provenance,
        validation: None,
    }
}

fn build_teacher_prompt(
    situation: &str,
    context: &Value,
    specific_question: Option<&str>,
    desired_output: &Value,
) -> String {
    let mut parts = vec![
        format!("Situation:\n{situation}"),
        format!(
            "Relevant Spoon knowledge context:\n{}",
            serde_json::to_string_pretty(context).unwrap_or_default()
        ),
    ];

    if let Some(question) = specific_question {
        parts.push(format!("Specific question:\n{question}"));
    }

    parts.push("Teaching checklist (encode every supported facet in the supplied schema):".into());
    parts.push("1. Language: identify the introduced terms, names, aliases, and wording.".into());
    parts.push(
        "2. Meaning: state the definition, semantic role, units or domain, and scope.".into(),
    );
    parts.push("3. Intent: identify what the user wants to accomplish.".into());
    parts.push("4. Structure: record stable relationships, dependencies, inputs, outputs.".into());
    parts.push("5. Procedure: when deterministic and safe, author focused procedures with contracts and a worked invocation.".into());
    parts.push("6. Limits: preserve uncertainty and use answer-only or abstain when reusable structure is not justified.".into());
    parts.push(format!(
        "Desired proposal JSON Schema:\n{}",
        serde_json::to_string_pretty(desired_output).unwrap_or_default()
    ));
    parts.push(spoon_core::spoonlang::SPOONLANG_GRAMMAR.into());
    parts.push(
        "Produce the most complete safe structured lesson the evidence and schema permit.".into(),
    );

    parts.join("\n\n")
}

// ---------- Interpreter LLM call ----------

const INTERPRETER_SYSTEM_PROMPT: &str = concat!(
    "You are a Spoon language interpreter. Your job is to route a user's natural language ",
    "situation to the single best matching procedure from a candidate list, and fill its ",
    "parameter slots by grounding values from the user's input tokens.\n\n",
    "You receive:\n",
    "- A situation (the user's input text)\n",
    "- A context object with a 'candidates' array listing available procedures with their ",
    "  names, descriptions, parameters, and aliases\n",
    "- A 'literalCandidates' array listing token ranges from the input that can be used ",
    "  as slot values (each has startToken/endToken indexes and the extracted text/value)\n",
    "- A desired output JSON Schema you must conform to\n\n",
    "Rules:\n",
    "1. If exactly one candidate procedure clearly matches the user's intent, set ",
    "   disposition='execute', selected=0, and populate candidates with one frame.\n",
    "2. Fill each slot by selecting the best matching literal token range from ",
    "   literalCandidates using 'sourceTokens'. If no literal matches, use 'inferredValue'.\n",
    "3. The frame 'name' must be the procedure's alias from the candidates list.\n",
    "4. If multiple procedures could match and you cannot disambiguate, set ",
    "   disposition='clarify' with ambiguities listing the competing aliases.\n",
    "5. If no candidate procedure matches the user's intent at all, set ",
    "   disposition='abstain' with candidates=[] and selected=null.\n",
    "6. Confidence values are 0.0-1.0 floats.\n",
    "7. sourceTokens arrays must contain objects with startToken and endToken fields ",
    "   matching values from the literalCandidates.\n",
    "Return only valid JSON matching the supplied schema.",
);

fn call_interpreter(
    config: &TeacherConfig,
    client: &reqwest::blocking::Client,
    request: &IntentRequestWire,
) -> Result<IntentProposalWire, String> {
    let prompt = build_interpreter_prompt(request);
    let url = format!("{}/api/generate", config.base_url.trim_end_matches('/'));

    let request_body = json!({
        "model": config.model,
        "prompt": prompt,
        "system": INTERPRETER_SYSTEM_PROMPT,
        "stream": true,
        "keep_alive": "30m",
        "format": request.desired_output,
        "think": false,
    });

    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .body(serde_json::to_string(&request_body).unwrap())
        .send()
        .map_err(|e| format!("interpreter HTTP request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("interpreter returned {status}: {body}"));
    }

    let body = response
        .text()
        .map_err(|e| format!("failed to read interpreter response: {e}"))?;
    let content = parse_ollama_streaming_response(&body)?;

    let parsed: InterpretationProposal = serde_json::from_str(&content).map_err(|e| {
        format!("interpreter response was not valid InterpretationProposal: {e}\nraw: {content}")
    })?;

    Ok(IntentProposalWire {
        content: parsed,
        source: format!("ollama:{}", config.model),
        status: "unverified".into(),
        provenance: json!({
            "provider": "ollama",
            "model": config.model,
            "role": "interpreter",
            "generatedAt": chrono_now_iso(),
        }),
        raw_content: Some(serde_json::to_value(&content).unwrap_or_default()),
    })
}

fn build_interpreter_prompt(request: &IntentRequestWire) -> String {
    let mut parts = vec![
        format!("User situation:\n{}", request.situation),
        format!(
            "Available procedures and context:\n{}",
            serde_json::to_string_pretty(&request.context).unwrap_or_default()
        ),
        format!(
            "Required output JSON Schema:\n{}",
            serde_json::to_string_pretty(&request.desired_output).unwrap_or_default()
        ),
    ];
    parts.push(
        "Select the best matching procedure for this situation, fill its parameter slots \
         using sourceTokens from the literalCandidates, and return the result as JSON."
            .into(),
    );
    parts.join("\n\n")
}

fn parse_ollama_streaming_response(body: &str) -> Result<String, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err("teacher returned empty response".into());
    }

    if let Ok(obj) = serde_json::from_str::<Value>(trimmed) {
        if let Some(error) = obj.get("error").and_then(Value::as_str) {
            return Err(format!("teacher error: {error}"));
        }
        return ollama_structured_content(
            obj.get("response").and_then(Value::as_str).unwrap_or(""),
            obj.get("thinking").and_then(Value::as_str).unwrap_or(""),
        );
    }

    let mut response = String::new();
    let mut thinking = String::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(obj) = serde_json::from_str::<Value>(line) {
            if let Some(error) = obj.get("error").and_then(Value::as_str) {
                return Err(format!("teacher error: {error}"));
            }
            if let Some(chunk) = obj.get("response").and_then(Value::as_str) {
                response.push_str(chunk);
            }
            if let Some(chunk) = obj.get("thinking").and_then(Value::as_str) {
                thinking.push_str(chunk);
            }
        }
    }

    ollama_structured_content(&response, &thinking)
}

fn ollama_structured_content(response: &str, thinking: &str) -> Result<String, String> {
    if !response.trim().is_empty() {
        return Ok(response.to_string());
    }
    if !thinking.trim().is_empty() {
        return Ok(thinking.to_string());
    }
    Err("teacher response contained no content".into())
}

fn chrono_now_iso() -> String {
    unix_secs_to_rfc3339(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

fn unix_secs_to_rfc3339(secs: u64) -> String {
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn format_answer(result: &Value) -> String {
    if let Some(answer) = result.get("answer") {
        if answer.is_null() {
            if let Some(reason) = result.get("reason").and_then(Value::as_str) {
                return reason.to_string();
            }
            return "I don't have enough knowledge to answer that yet.".into();
        }
        match answer {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        }
    } else if let Some(reason) = result.get("reason").and_then(Value::as_str) {
        reason.to_string()
    } else {
        serde_json::to_string_pretty(result).unwrap_or_default()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------- Chat UI ----------

async fn chat_page() -> Html<&'static str> {
    Html(CHAT_HTML)
}

const CHAT_HTML: &str = include_str!("chat.html");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_teacher_provenance_binds_source_request_and_situation() {
        let proposal = teacher_proposal(
            json!({"proposalKind": "answer_only"}),
            "ollama:qwen2.5:1.5b".into(),
            "ollama",
            Some("qwen2.5:1.5b"),
            "how many eyes to humans have?",
        );
        let provenance = proposal.provenance.as_object().expect("object");
        assert_eq!(proposal.status, "unverified");
        assert_eq!(proposal.source, "ollama:qwen2.5:1.5b");
        assert_eq!(provenance["provider"], "ollama");
        assert_eq!(provenance["teacher"], "ollama:qwen2.5:1.5b");
        assert_eq!(provenance["situation"], "how many eyes to humans have?");
        assert_eq!(provenance["model"], "qwen2.5:1.5b");
        assert!(
            provenance["requestId"]
                .as_str()
                .is_some_and(|id| !id.trim().is_empty())
        );
        assert!(
            provenance["generatedAt"]
                .as_str()
                .is_some_and(|stamp| stamp.contains('T') && stamp.ends_with('Z'))
        );
    }

    #[test]
    fn unix_epoch_formats_as_rfc3339() {
        assert_eq!(unix_secs_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_secs_to_rfc3339(1_788_000_000), "2026-08-29T10:40:00Z");
    }

    #[test]
    fn llm_role_status_omits_secrets_and_labels_ollama() {
        let on = llm_role_status(&Some(TeacherConfig {
            base_url: "http://localhost:11434".into(),
            model: "qwen3.8:27b".into(),
            api_key: Some("secret".into()),
        }));
        assert_eq!(on["adapter"], "ollama");
        assert_eq!(on["model"], "qwen3.8:27b");
        assert_eq!(on["baseUrl"], "http://localhost:11434");
        assert!(on.get("apiKey").is_none());
        assert!(on.get("api_key").is_none());

        let off = llm_role_status(&None);
        assert_eq!(off["adapter"], "off");
        assert!(off["model"].is_null());
    }
}
