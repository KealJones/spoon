use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use ekg_core::{
    Concept, ConceptId, Contract, EpisodeId, EscalationRung, Expr, MutabilityClass, Param,
    Procedure, ProcedureId, Relationship, RelationshipId, Value as EkgValue,
};
use ekg_engine::{Engine, EngineError};
use ekg_episode::EpisodeQuery;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub struct RpcServer {
    engine: Engine,
}

impl RpcServer {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        Ok(Self {
            engine: Engine::open(path)?,
        })
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Ok(Self {
            engine: Engine::in_memory()?,
        })
    }

    pub fn handle_line(&self, line: &str) -> String {
        let request: RpcRequest = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(error) => {
                return serialize_response(RpcResponse::error(
                    Value::Null,
                    RpcFault::new(-32700, "parse error").with_data(error.to_string()),
                ));
            }
        };

        if request.jsonrpc != "2.0" || request.method.trim().is_empty() {
            return serialize_response(RpcResponse::error(
                request.id,
                RpcFault::new(-32600, "invalid request"),
            ));
        }

        let response = match self.dispatch(&request.method, request.params) {
            Ok(result) => RpcResponse::success(request.id, result),
            Err(error) => RpcResponse::error(request.id, error),
        };
        serialize_response(response)
    }

    fn dispatch(&self, method: &str, params: Value) -> Result<Value, RpcFault> {
        match method {
            "concept.create" => {
                let input: CreateConcept = decode(params)?;
                let mut concept = Concept::new(input.name, input.mutability);
                concept.description = input.description;
                self.engine
                    .graph()
                    .insert_concept(&concept)
                    .map_err(app_error)?;
                encode(concept)
            }
            "concept.get" => {
                let input: ConceptIdParam = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .get_concept(input.concept_id()?)
                        .map_err(app_error)?,
                )
            }
            "concept.findByName" => {
                let input: NameParam = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .get_concept_by_name(&input.name)
                        .map_err(app_error)?,
                )
            }
            "concept.list" => encode(self.engine.graph().list_concepts().map_err(app_error)?),
            "concept.update" => {
                let concept: Concept = decode(params)?;
                self.engine
                    .graph()
                    .update_concept(&concept)
                    .map_err(app_error)?;
                encode(concept)
            }
            "concept.delete" => {
                let input: ConceptIdParam = decode(params)?;
                self.engine
                    .graph()
                    .delete_concept(input.concept_id()?)
                    .map_err(app_error)?;
                Ok(json!({ "deleted": true }))
            }
            "relationship.create" => {
                let input: CreateRelationship = decode(params)?;
                let mut relationship =
                    Relationship::new(input.source()?, input.target()?, input.kind);
                relationship.strength = input.strength;
                self.engine
                    .graph()
                    .insert_relationship(&relationship)
                    .map_err(app_error)?;
                encode(relationship)
            }
            "relationship.get" => {
                let input: RelationshipIdParam = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .get_relationship(input.relationship_id()?)
                        .map_err(app_error)?,
                )
            }
            "relationship.update" => {
                let relationship: Relationship = decode(params)?;
                self.engine
                    .graph()
                    .update_relationship(&relationship)
                    .map_err(app_error)?;
                encode(relationship)
            }
            "relationship.delete" => {
                let input: RelationshipIdParam = decode(params)?;
                self.engine
                    .graph()
                    .delete_relationship(input.relationship_id()?)
                    .map_err(app_error)?;
                Ok(json!({ "deleted": true }))
            }
            "graph.traverse" => {
                let input: TraverseParams = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .traverse(input.concept_id()?, &input.kind, input.max_hops)
                        .map_err(app_error)?,
                )
            }
            "procedure.create" => {
                let input: CreateProcedure = decode(params)?;
                let mut procedure = Procedure::new(input.name, input.params, input.body);
                procedure.contract = input.contract;
                procedure.concept = input
                    .concept_id
                    .map(|id| parse_uuid(&id).map(ConceptId))
                    .transpose()?;
                self.engine
                    .graph()
                    .insert_procedure(&procedure)
                    .map_err(app_error)?;
                encode(procedure)
            }
            "procedure.get" => {
                let input: ProcedureIdParam = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .get_procedure(input.procedure_id()?)
                        .map_err(app_error)?,
                )
            }
            "procedure.findByName" => {
                let input: NameParam = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .get_procedure_by_name(&input.name)
                        .map_err(app_error)?,
                )
            }
            "procedure.list" => encode(self.engine.graph().list_procedures().map_err(app_error)?),
            "procedure.update" => {
                let procedure: Procedure = decode(params)?;
                self.engine
                    .graph()
                    .update_procedure(&procedure)
                    .map_err(app_error)?;
                encode(procedure)
            }
            "procedure.delete" => {
                let input: ProcedureIdParam = decode(params)?;
                self.engine
                    .graph()
                    .delete_procedure(input.procedure_id()?)
                    .map_err(app_error)?;
                Ok(json!({ "deleted": true }))
            }
            "procedure.execute" => {
                let input: ExecuteParams = decode(params)?;
                encode(
                    self.engine
                        .execute_procedure(input.procedure_id()?, input.inputs, input.prediction)
                        .map_err(app_error)?,
                )
            }
            "episode.get" => {
                let input: EpisodeIdParam = decode(params)?;
                encode(
                    self.engine
                        .episodes()
                        .get(input.episode_id()?)
                        .map_err(app_error)?,
                )
            }
            "episode.list" => {
                let input: EpisodeListParams = decode(params)?;
                encode(
                    self.engine
                        .episodes()
                        .query(&input.into_query()?)
                        .map_err(app_error)?,
                )
            }
            "episode.replay" => {
                let input: ReplayParams = decode(params)?;
                encode(
                    self.engine
                        .replay_episode(input.episode_id()?, input.substitutions)
                        .map_err(app_error)?,
                )
            }
            _ => Err(RpcFault::new(-32601, "method not found")),
        }
    }
}

pub fn run_stdio<R: BufRead, W: Write>(
    server: &RpcServer,
    reader: R,
    mut writer: W,
) -> std::io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        writeln!(writer, "{}", server.handle_line(&line))?;
        writer.flush()?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcFault>,
}

impl RpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, error: RpcFault) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Serialize)]
struct RpcFault {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl RpcFault {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn with_data(mut self, data: impl Serialize) -> Self {
        self.data = serde_json::to_value(data).ok();
        self
    }
}

fn decode<T: DeserializeOwned>(params: Value) -> Result<T, RpcFault> {
    serde_json::from_value(params)
        .map_err(|error| RpcFault::new(-32602, "invalid params").with_data(error.to_string()))
}

fn encode<T: Serialize>(value: T) -> Result<Value, RpcFault> {
    serde_json::to_value(value)
        .map_err(|error| RpcFault::new(-32603, "serialization failed").with_data(error.to_string()))
}

fn app_error(error: impl std::fmt::Display) -> RpcFault {
    RpcFault::new(-32000, error.to_string())
}

fn serialize_response(response: RpcResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal error"}}"#.into()
    })
}

fn parse_uuid(value: &str) -> Result<Uuid, RpcFault> {
    Uuid::parse_str(value)
        .map_err(|error| RpcFault::new(-32602, "invalid id").with_data(error.to_string()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConceptIdParam {
    concept_id: String,
}
impl ConceptIdParam {
    fn concept_id(&self) -> Result<ConceptId, RpcFault> {
        Ok(ConceptId(parse_uuid(&self.concept_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelationshipIdParam {
    relationship_id: String,
}
impl RelationshipIdParam {
    fn relationship_id(&self) -> Result<RelationshipId, RpcFault> {
        Ok(RelationshipId(parse_uuid(&self.relationship_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcedureIdParam {
    procedure_id: String,
}
impl ProcedureIdParam {
    fn procedure_id(&self) -> Result<ProcedureId, RpcFault> {
        Ok(ProcedureId(parse_uuid(&self.procedure_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeIdParam {
    episode_id: String,
}
impl EpisodeIdParam {
    fn episode_id(&self) -> Result<EpisodeId, RpcFault> {
        Ok(EpisodeId(parse_uuid(&self.episode_id)?))
    }
}

#[derive(Deserialize)]
struct NameParam {
    name: String,
}

fn default_mutability() -> MutabilityClass {
    MutabilityClass::DefeasibleGeneral
}
fn default_strength() -> f64 {
    1.0
}
fn default_hops() -> u32 {
    1
}
fn default_limit() -> u32 {
    100
}

#[derive(Deserialize)]
struct CreateConcept {
    name: String,
    description: Option<String>,
    #[serde(default = "default_mutability")]
    mutability: MutabilityClass,
}

#[derive(Deserialize)]
struct CreateRelationship {
    source: String,
    target: String,
    kind: String,
    #[serde(default = "default_strength")]
    strength: f64,
}
impl CreateRelationship {
    fn source(&self) -> Result<ConceptId, RpcFault> {
        Ok(ConceptId(parse_uuid(&self.source)?))
    }
    fn target(&self) -> Result<ConceptId, RpcFault> {
        Ok(ConceptId(parse_uuid(&self.target)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TraverseParams {
    concept_id: String,
    kind: String,
    #[serde(default = "default_hops")]
    max_hops: u32,
}
impl TraverseParams {
    fn concept_id(&self) -> Result<ConceptId, RpcFault> {
        Ok(ConceptId(parse_uuid(&self.concept_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProcedure {
    name: String,
    #[serde(default)]
    params: Vec<Param>,
    body: Expr,
    #[serde(default)]
    contract: Contract,
    concept_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteParams {
    procedure_id: String,
    #[serde(default)]
    inputs: BTreeMap<String, EkgValue>,
    prediction: Option<EkgValue>,
}
impl ExecuteParams {
    fn procedure_id(&self) -> Result<ProcedureId, RpcFault> {
        Ok(ProcedureId(parse_uuid(&self.procedure_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplayParams {
    episode_id: String,
    #[serde(default)]
    substitutions: BTreeMap<String, EkgValue>,
}
impl ReplayParams {
    fn episode_id(&self) -> Result<EpisodeId, RpcFault> {
        Ok(EpisodeId(parse_uuid(&self.episode_id)?))
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeListParams {
    since: Option<i64>,
    until: Option<i64>,
    outcome: Option<String>,
    rung: Option<EscalationRung>,
    concept_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}
impl EpisodeListParams {
    fn into_query(self) -> Result<EpisodeQuery, RpcFault> {
        let outcome = match self.outcome.as_deref() {
            None | Some("any") => None,
            Some("success") => Some(true),
            Some("failure") => Some(false),
            Some(other) => return Err(RpcFault::new(-32602, format!("invalid outcome '{other}'"))),
        };
        Ok(EpisodeQuery {
            since: self.since,
            until: self.until,
            outcome,
            rung: self.rung,
            concept: self
                .concept_id
                .map(|id| parse_uuid(&id).map(ConceptId))
                .transpose()?,
            limit: self.limit,
        })
    }
}
