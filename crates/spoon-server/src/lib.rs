pub mod config;
pub mod http;

use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spoon_adapt::{
    Claim, Contradiction, ContradictionId, DemonstratedFeature, Implication, Refinement,
    ScopeAssignment, Uncertainty,
};
use spoon_capability::{
    AdapterExecution, AuthorizedPrimitiveInvocation, CapabilityError, CapabilityInvocationAdapter,
    NativePrimitive, PrimitivePolicy, ResourceBounds, ScopedFileAdapter,
};
use spoon_core::SessionVisibility;
use spoon_core::{
    Assumption, Concept, ConceptId, Contract, DialogueMove, EpisodeId, EscalationRung, Evaluation,
    Expr, LanguageError, MutabilityClass, Param, Procedure, ProcedureId, Relationship,
    RelationshipId, RenderVariant, RenderedResponse, ResponsePlan, ResponseRenderer, ResponseTone,
    Value as SpoonValue,
};
use spoon_engine::{
    AdaptationPlanId, AdaptationPlanRequest, ApplyAdaptationRequest, CapabilityBundle,
    CapabilityExecutionOutcome, CuriosityGap, CycleBudget, CycleId, CycleInput, CycleOutcome,
    CycleProgress, DiscoveredOperation, Engine, EngineError, FailureAnalysisRequest,
    FalsificationMeasurementInput, FalsificationRunInput, GoalKind, IntentProposalWire,
    InterfaceDescription, LocalValidation, Permission, PromotionReplay, RecallMode, SkillCandidate,
    TeacherProposalWire,
};
use spoon_episode::{EpisodeFeedback, EpisodeQuery, FeedbackSource};
use spoon_graph::GraphError;
use uuid::Uuid;

const MAX_PUBLIC_CAPABILITY_INPUT_BYTES: usize = 1024 * 1024;
const MAX_PUBLIC_PROCEDURE_ID_BYTES: usize = 512;
/// The core renderer separately limits claim and plan text. This public
/// envelope ceiling also bounds evidence/provenance metadata supplied by a
/// caller before it can reach the renderer.
const MAX_PUBLIC_LANGUAGE_RENDER_INPUT_BYTES: usize = 128 * 1024;

/// Server-owned registry of concrete host effect adapters. It is configured
/// out of band by the embedding host; JSON-RPC callers cannot register an
/// adapter, choose a root, or supply a permission policy.
#[derive(Debug, Clone, Default)]
pub struct CapabilityHostAdapters {
    scoped_files: Option<ScopedFileAdapter>,
    web_fetch: Option<WebFetchAdapter>,
}

/// A host-owned HTTP(S) adapter for the `NetworkRequest` primitive.
///
/// The capability contract still supplies the authorized host and method. The
/// typed input supplies a URL, and this adapter requires that URL's authority
/// match the authorized host exactly. This keeps URL paths/query strings
/// useful without allowing a procedure to turn a host grant into open egress.
#[derive(Debug, Clone)]
pub struct WebFetchAdapter {
    client: reqwest::blocking::Client,
    authorization_policy: PrimitivePolicy,
    allow_runtime_hosts: bool,
}

impl WebFetchAdapter {
    pub fn new(
        hosts: impl IntoIterator<Item = String>,
        bounds: ResourceBounds,
    ) -> Result<Self, CapabilityError> {
        Self::build(hosts, bounds, false)
    }

    /// A native Spoon capability may resolve its exact URL only at runtime.
    /// This adapter supports that path without pre-registering each host;
    /// the caller must still make the runtime permission decision before the
    /// adapter is invoked. Imported capability bundles remain host-scoped.
    pub fn new_runtime_approved(bounds: ResourceBounds) -> Result<Self, CapabilityError> {
        Self::build(Vec::new(), bounds, true)
    }

    fn build(
        hosts: impl IntoIterator<Item = String>,
        bounds: ResourceBounds,
        allow_runtime_hosts: bool,
    ) -> Result<Self, CapabilityError> {
        if bounds.max_bytes == 0 || bounds.max_steps == 0 || bounds.max_millis == 0 {
            return Err(CapabilityError::Invalid(
                "web fetch bounds must all be positive".into(),
            ));
        }
        let network_hosts = hosts
            .into_iter()
            .map(|host| {
                let host = host.trim().to_owned();
                if !valid_fetch_host(&host) {
                    return Err(CapabilityError::Invalid(format!(
                        "web fetch host is not a portable host authority: {host}"
                    )));
                }
                Ok(host)
            })
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        if network_hosts.is_empty() && !allow_runtime_hosts {
            return Err(CapabilityError::Invalid(
                "web fetch requires at least one allowed host".into(),
            ));
        }
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_millis(bounds.max_millis))
            .build()
            .map_err(|error| {
                CapabilityError::Invalid(format!("web fetch client failed: {error}"))
            })?;
        Ok(Self {
            client,
            authorization_policy: PrimitivePolicy {
                network_hosts,
                bounds,
                ..PrimitivePolicy::default()
            },
            allow_runtime_hosts,
        })
    }

    pub fn policy(&self) -> &PrimitivePolicy {
        &self.authorization_policy
    }

    fn execute_network(
        &self,
        invocation: &AuthorizedPrimitiveInvocation,
    ) -> Result<AdapterExecution, CapabilityError> {
        let started = Instant::now();
        let spoon_capability::PrimitiveRequest::Network {
            host,
            method,
            body_bytes,
        } = &invocation.request
        else {
            return Err(CapabilityError::AdapterViolation(
                "web fetch adapter received a non-network request".into(),
            ));
        };
        let url_text = invocation
            .input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CapabilityError::Invalid("web.fetch input requires a string url".into())
            })?;
        let url = reqwest::Url::parse(url_text).map_err(|error| {
            CapabilityError::Invalid(format!("web.fetch url is invalid: {error}"))
        })?;
        if !matches!(url.scheme(), "http" | "https")
            || url.username() != ""
            || url.password().is_some()
        {
            return Err(CapabilityError::Invalid(
                "web.fetch only accepts http(s) URLs without embedded credentials".into(),
            ));
        }
        let host_name = url
            .host_str()
            .ok_or_else(|| CapabilityError::Invalid("web.fetch url has no host".into()))?
            .to_owned();
        let authority_host = if host_name.contains(':') {
            format!("[{host_name}]")
        } else {
            host_name
        };
        let authority = match url.port() {
            Some(port) => format!("{authority_host}:{port}"),
            None => authority_host,
        };
        if authority != *host
            || (!self.allow_runtime_hosts
                && !self.authorization_policy.network_hosts.contains(host))
        {
            return Err(CapabilityError::PermissionRequired(format!(
                "network host {authority} is outside the authorized web.fetch host"
            )));
        }
        if *method != method.to_ascii_uppercase() {
            return Err(CapabilityError::Invalid(
                "web.fetch method must be uppercase".into(),
            ));
        }
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|error| {
            CapabilityError::Invalid(format!("web.fetch method is invalid: {error}"))
        })?;
        let mut request = self.client.request(method, url);
        if let Some(headers) = invocation.input.get("headers") {
            let headers = headers.as_object().ok_or_else(|| {
                CapabilityError::Invalid("web.fetch headers must be an object".into())
            })?;
            for (name, value) in headers {
                if forbidden_fetch_header(name) {
                    return Err(CapabilityError::PermissionRequired(format!(
                        "web.fetch header {name} is not host-authorized"
                    )));
                }
                let value = value.as_str().ok_or_else(|| {
                    CapabilityError::Invalid(format!("web.fetch header {name} must be a string"))
                })?;
                request = request.header(name, value);
            }
        }
        if let Some(body) = invocation.input.get("body") {
            let bytes = match body {
                Value::String(text) => text.as_bytes().to_vec(),
                _ => serde_json::to_vec(body).map_err(|error| {
                    CapabilityError::Invalid(format!("web.fetch body is not serializable: {error}"))
                })?,
            };
            if bytes.len() as u64 > *body_bytes
                || bytes.len() as u64 > self.authorization_policy.bounds.max_bytes
            {
                return Err(CapabilityError::Invalid(
                    "web.fetch request body exceeds its resource byte bound".into(),
                ));
            }
            request = request.body(bytes);
        }
        let response = request.send().map_err(|error| {
            CapabilityError::Invalid(format!("web.fetch request failed: {error}"))
        })?;
        let status = response.status().as_u16();
        let mut response_headers = BTreeMap::new();
        for (name, value) in response.headers() {
            if let Ok(value) = value.to_str() {
                response_headers.insert(name.to_string(), Value::String(value.to_owned()));
            }
        }
        let max_bytes =
            usize::try_from(self.authorization_policy.bounds.max_bytes).unwrap_or(usize::MAX);
        let mut bytes = Vec::new();
        response
            .take((max_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| {
                CapabilityError::Invalid(format!("web.fetch response read failed: {error}"))
            })?;
        if bytes.len() > max_bytes {
            return Err(CapabilityError::Invalid(
                "web.fetch response exceeds its resource byte bound".into(),
            ));
        }
        let body = match String::from_utf8(bytes.clone()) {
            Ok(text) => Value::String(text),
            Err(_) => Value::Array(bytes.into_iter().map(|byte| Value::from(byte)).collect()),
        };
        let output = json!({
            "status": status,
            "headers": response_headers,
            "body": body,
        });
        let usage_bytes = serde_json::to_vec(&invocation.input)?
            .len()
            .saturating_add(serde_json::to_vec(&output)?.len());
        Ok(AdapterExecution {
            effect: invocation.effect.clone(),
            output,
            usage: spoon_capability::ResourceUsage {
                bytes: u64::try_from(usage_bytes).unwrap_or(u64::MAX),
                steps: 1,
                millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            },
        })
    }
}

fn valid_fetch_host(host: &str) -> bool {
    !host.is_empty()
        && host != "*"
        && !host
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'\\' | b'@' | b'?' | b'#'))
        && host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
}

fn forbidden_fetch_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "proxy-authorization" | "host"
    )
}

impl CapabilityHostAdapters {
    pub fn with_runtime_approved_web_fetch(
        bounds: ResourceBounds,
    ) -> Result<Self, CapabilityError> {
        Ok(Self {
            scoped_files: None,
            web_fetch: Some(WebFetchAdapter::new_runtime_approved(bounds)?),
        })
    }

    pub fn with_scoped_files(
        binding: impl Into<String>,
        root: impl AsRef<std::path::Path>,
        bounds: ResourceBounds,
    ) -> Result<Self, CapabilityError> {
        Ok(Self {
            scoped_files: Some(ScopedFileAdapter::new(binding, root, bounds)?),
            web_fetch: None,
        })
    }

    pub fn with_web_fetch(
        hosts: impl IntoIterator<Item = String>,
        bounds: ResourceBounds,
    ) -> Result<Self, CapabilityError> {
        Ok(Self {
            scoped_files: None,
            web_fetch: Some(WebFetchAdapter::new(hosts, bounds)?),
        })
    }

    pub fn with_scoped_files_and_web_fetch(
        binding: impl Into<String>,
        root: impl AsRef<std::path::Path>,
        file_bounds: ResourceBounds,
        hosts: impl IntoIterator<Item = String>,
        web_bounds: ResourceBounds,
    ) -> Result<Self, CapabilityError> {
        Ok(Self {
            scoped_files: Some(ScopedFileAdapter::new(binding, root, file_bounds)?),
            web_fetch: Some(WebFetchAdapter::new(hosts, web_bounds)?),
        })
    }

    fn supports(&self, primitive: &NativePrimitive) -> bool {
        (matches!(
            primitive,
            NativePrimitive::FileRead | NativePrimitive::FileWrite
        ) && self.scoped_files.is_some())
            || (matches!(primitive, NativePrimitive::NetworkRequest) && self.web_fetch.is_some())
    }

    fn policy(&self, primitive: &NativePrimitive) -> Option<PrimitivePolicy> {
        if !self.supports(primitive) {
            return None;
        }
        match primitive {
            NativePrimitive::FileRead | NativePrimitive::FileWrite => self
                .scoped_files
                .as_ref()
                .map(|adapter| adapter.policy().clone()),
            NativePrimitive::NetworkRequest => self
                .web_fetch
                .as_ref()
                .map(|adapter| adapter.policy().clone()),
            _ => None,
        }
    }
}

impl CapabilityInvocationAdapter for CapabilityHostAdapters {
    fn policy(&self, primitive: &NativePrimitive) -> Option<PrimitivePolicy> {
        CapabilityHostAdapters::policy(self, primitive)
    }

    fn execute(
        &mut self,
        invocation: &AuthorizedPrimitiveInvocation,
    ) -> Result<AdapterExecution, CapabilityError> {
        match invocation.primitive {
            NativePrimitive::FileRead | NativePrimitive::FileWrite => self
                .scoped_files
                .as_mut()
                .ok_or_else(|| {
                    CapabilityError::AdapterViolation(
                        "no scoped file host adapter is configured".into(),
                    )
                })?
                .execute(invocation),
            NativePrimitive::NetworkRequest => self
                .web_fetch
                .as_ref()
                .ok_or_else(|| {
                    CapabilityError::AdapterViolation(
                        "no web fetch host adapter is configured".into(),
                    )
                })?
                .execute_network(invocation),
            _ => Err(CapabilityError::AdapterViolation(
                "no host adapter is configured for the requested primitive".into(),
            )),
        }
    }
}

pub struct RpcServer {
    pub(crate) engine: Engine,
    admin_token: Option<String>,
    feedback_source_identity: String,
    capability_adapters: CapabilityHostAdapters,
}

impl RpcServer {
    pub fn from_engine(mut engine: Engine) -> Self {
        let capability_adapters =
            CapabilityHostAdapters::with_runtime_approved_web_fetch(ResourceBounds {
                max_bytes: 1_048_576,
                max_steps: 1,
                max_millis: 10_000,
            })
            .expect("fixed native web fetch bounds are valid");
        engine.set_capability_adapter(Box::new(capability_adapters.clone()));
        Self {
            engine,
            admin_token: None,
            feedback_source_identity: "spoon-server".into(),
            capability_adapters,
        }
    }

    /// Install an explicit server-local host adapter registry. This is the
    /// only supported way to make `capability.invoke` effectful.
    pub fn with_capability_host_adapters(mut self, adapters: CapabilityHostAdapters) -> Self {
        self.engine
            .set_capability_adapter(Box::new(adapters.clone()));
        self.capability_adapters = adapters;
        self
    }

    pub fn with_admin_token(mut self, token: impl Into<String>) -> Result<Self, EngineError> {
        let token = token.into();
        if !token.trim().is_empty() {
            self.engine.enable_admin(&token)?;
            self.admin_token = Some(token);
        }
        Ok(self)
    }

    pub fn with_feedback_source_identity(mut self, identity: impl Into<String>) -> Self {
        let identity = identity.into();
        if !identity.trim().is_empty() {
            self.feedback_source_identity = identity;
        }
        self
    }

    pub fn open(path: &str) -> Result<Self, EngineError> {
        Ok(Self::from_engine(Engine::open(path)?))
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Ok(Self::from_engine(Engine::in_memory()?))
    }

    pub fn handle_line(&mut self, line: &str) -> String {
        let request: RpcRequest = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(error) => {
                return serialize_response(RpcResponse::error(
                    Value::Null,
                    RpcFault::new(-32700, "parse error").with_data(json!({
                        "kind": "parse_error",
                        "cause": error.to_string(),
                    })),
                ));
            }
        };

        if request.jsonrpc != "2.0" || request.method.trim().is_empty() {
            return serialize_response(RpcResponse::error(
                request.id,
                RpcFault::new(-32600, "invalid request")
                    .with_data(json!({ "kind": "invalid_request" })),
            ));
        }

        let response = match self.dispatch(&request.method, request.params) {
            Ok(result) => RpcResponse::success(request.id, result),
            Err(error) => RpcResponse::error(request.id, error),
        };
        serialize_response(response)
    }

    fn dispatch(&mut self, method: &str, mut params: Value) -> Result<Value, RpcFault> {
        if requires_admin(method) {
            self.authorize_admin(&mut params)?;
        }
        match method {
            "concept.create" => {
                let input: CreateConcept = decode(params)?;
                let mut concept = Concept::new(input.name, input.mutability);
                concept.description = input.description;
                self.engine
                    .admin_insert_concept(&concept)
                    .map_err(engine_error)?;
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
                let input: ReviseConceptParams = decode(params)?;
                let version = self
                    .engine
                    .admin_revise_concept(&input.concept, input.expected_version)
                    .map_err(engine_error)?;
                encode(VersionedConceptWire {
                    version,
                    concept: input.concept,
                })
            }
            "concept.getVersion" => {
                let input: ConceptVersionParam = decode(params)?;
                let concept = self
                    .engine
                    .graph()
                    .get_concept_version(input.concept_id()?, input.version)
                    .map_err(graph_error)?;
                encode(concept.map(|concept| VersionedConceptWire {
                    version: input.version,
                    concept,
                }))
            }
            "concept.listVersions" => {
                let input: ConceptIdParam = decode(params)?;
                let concepts = self
                    .engine
                    .graph()
                    .list_concept_versions(input.concept_id()?)
                    .map_err(graph_error)?;
                encode(
                    concepts
                        .into_iter()
                        .enumerate()
                        .map(|(index, concept)| VersionedConceptWire {
                            version: index as u32 + 1,
                            concept,
                        })
                        .collect::<Vec<_>>(),
                )
            }
            "concept.delete" => {
                let input: ConceptIdParam = decode(params)?;
                self.engine
                    .admin_delete_concept(input.concept_id()?)
                    .map_err(engine_error)?;
                Ok(json!({ "deleted": true }))
            }
            "relationship.create" => {
                let input: CreateRelationship = decode(params)?;
                let mut relationship =
                    Relationship::new(input.source()?, input.target()?, input.kind);
                relationship.strength = input.strength;
                self.engine
                    .admin_insert_relationship(&relationship)
                    .map_err(engine_error)?;
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
            "relationship.list" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .list_relationships(input.limit.unwrap_or(default_limit()))
                        .map_err(app_error)?,
                )
            }
            "relationship.update" => {
                let input: ReviseRelationshipParams = decode(params)?;
                let version = self
                    .engine
                    .admin_revise_relationship(&input.relationship, input.expected_version)
                    .map_err(engine_error)?;
                encode(VersionedRelationshipWire {
                    version,
                    relationship: input.relationship,
                })
            }
            "relationship.getVersion" => {
                let input: RelationshipVersionParam = decode(params)?;
                let relationship = self
                    .engine
                    .graph()
                    .get_relationship_version(input.relationship_id()?, input.version)
                    .map_err(graph_error)?;
                encode(relationship.map(|relationship| VersionedRelationshipWire {
                    version: input.version,
                    relationship,
                }))
            }
            "relationship.listVersions" => {
                let input: RelationshipIdParam = decode(params)?;
                let relationships = self
                    .engine
                    .graph()
                    .list_relationship_versions(input.relationship_id()?)
                    .map_err(graph_error)?;
                encode(
                    relationships
                        .into_iter()
                        .enumerate()
                        .map(|(index, relationship)| VersionedRelationshipWire {
                            version: index as u32 + 1,
                            relationship,
                        })
                        .collect::<Vec<_>>(),
                )
            }
            "relationship.delete" => {
                let input: RelationshipIdParam = decode(params)?;
                self.engine
                    .admin_delete_relationship(input.relationship_id()?)
                    .map_err(engine_error)?;
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
            "capability.discover" => {
                let input: InterfaceDescription = decode(params)?;
                encode(
                    self.engine
                        .discover_capability(&input)
                        .map_err(engine_error)?,
                )
            }
            "capability.list" => encode(json!({
                "imported": self.engine.list_imported_capabilities().map_err(engine_error)?,
                "nativeBoundaries": self.native_capability_boundaries(),
            })),
            "capability.provisionWebFetch" => {
                let input: ProvisionWebFetchParam = decode(params)?;
                self.provision_web_fetch(&input.host)
            }
            "capability.import" => {
                let input: CapabilityBundleParam = decode(params)?;
                let bytes = serde_json::to_vec(&input.bundle).map_err(|error| {
                    RpcFault::new(-32602, "invalid capability bundle")
                        .with_data(json!({"cause": error.to_string()}))
                })?;
                encode(
                    self.engine
                        .import_capability_bundle(&bytes)
                        .map_err(engine_error)?,
                )
            }
            "capability.importAndRevalidate" => {
                let input: CapabilityImportRevalidateParam = decode(params)?;
                let bytes = serde_json::to_vec(&input.bundle).map_err(|error| {
                    RpcFault::new(-32602, "invalid capability bundle")
                        .with_data(json!({"cause": error.to_string()}))
                })?;
                encode(
                    self.engine
                        .import_and_revalidate_capability_bundle(&bytes, &input.validation)
                        .map_err(engine_error)?,
                )
            }
            "capability.export" => {
                let input: CapabilityIdParam = decode(params)?;
                let bytes = self
                    .engine
                    .export_capability_bundle(&input.content_id)
                    .map_err(engine_error)?;
                let bundle: Value = serde_json::from_slice(&bytes).map_err(|error| {
                    RpcFault::new(-32603, "capability serialization failed")
                        .with_data(json!({"cause": error.to_string()}))
                })?;
                encode(json!({"bundle": bundle}))
            }
            "capability.revalidate" => {
                let input: CapabilityRevalidateParam = decode(params)?;
                encode(
                    self.engine
                        .revalidate_capability(&input.content_id, &input.validation)
                        .map_err(engine_error)?,
                )
            }
            "capability.reconstruct" => {
                let input: CapabilityIdParam = decode(params)?;
                encode(
                    self.engine
                        .reconstruct_capability(&input.content_id)
                        .map_err(engine_error)?,
                )
            }
            "capability.grant" => {
                let input: CapabilityPermissionParam = decode(params)?;
                self.engine
                    .grant_capability_permission(&input.content_id, &input.permission)
                    .map_err(engine_error)?;
                Ok(json!({"granted": true}))
            }
            "capability.revoke" => {
                let input: CapabilityPermissionParam = decode(params)?;
                self.engine
                    .revoke_capability_permission(&input.content_id, &input.permission)
                    .map_err(engine_error)?;
                Ok(json!({"revoked": true}))
            }
            "capability.authorizeProcedure" => {
                let input: CapabilityProcedureParam = decode(params)?;
                encode(
                    self.engine
                        .require_capability_procedure(&input.content_id, &input.procedure_id)
                        .map_err(engine_error)?,
                )
            }
            "capability.invoke" => {
                let input: CapabilityInvokeParam = decode(params)?;
                validate_capability_invocation_request(&input)?;
                let procedure = self
                    .engine
                    .require_capability_procedure(&input.content_id, &input.procedure_id)
                    .map_err(capability_authorization_error)?;
                let policy = self
                    .capability_adapters
                    .policy(&procedure.primitive)
                    .ok_or_else(capability_adapter_unavailable)?;
                let outcome = self
                    .engine
                    .invoke_capability(
                        &input.content_id,
                        &input.procedure_id,
                        &input.input,
                        None,
                        &policy,
                        &mut self.capability_adapters,
                    )
                    .map_err(capability_invocation_error)?;
                Ok(public_capability_invocation(outcome))
            }
            "language.render" => {
                let input: LanguageRenderParam = decode(params)?;
                validate_language_render_request(&input)?;
                let mut plan = input.plan;
                if let Some(options) = input.options {
                    if let Some(tone) = options.tone {
                        plan.tone = tone;
                    }
                    if let Some(variant) = options.variant {
                        plan.variant = variant;
                    }
                }
                let rendered = ResponseRenderer
                    .render(&plan)
                    .map_err(language_render_error)?;
                Ok(public_language_render_response(
                    plan.dialogue_move,
                    rendered,
                    plan.claims.len(),
                ))
            }
            "metrics.snapshot" => encode(self.engine.metrics_snapshot().map_err(engine_error)?),
            "telemetry.createRun" => {
                let input: FalsificationRunInput = decode(params)?;
                encode(
                    self.engine
                        .create_falsification_run(input)
                        .map_err(engine_error)?,
                )
            }
            "telemetry.recordMeasurement" => {
                let input: FalsificationMeasurementParam = decode(params)?;
                encode(
                    self.engine
                        .record_falsification_measurement(&input.run_id, input.measurement)
                        .map_err(engine_error)?,
                )
            }
            "primitive.observe" => {
                let input: PrimitiveObserveParam = decode(params)?;
                encode(
                    self.engine
                        .observe_native_primitive(&input.target)
                        .map_err(engine_error)?,
                )
            }
            "goal.create" => {
                let input: GoalCreateParam = decode(params)?;
                encode(
                    self.engine
                        .create_goal(input.kind, &input.statement, input.parent_id.as_deref())
                        .map_err(engine_error)?,
                )
            }
            "goal.createLearning" => {
                let input: LearningGoalCreateParam = decode(params)?;
                encode(
                    self.engine
                        .create_learning_goal(
                            &input.statement,
                            &input.standing_goal_id,
                            &input.source_gap_id,
                            &input.derivation_reason,
                        )
                        .map_err(engine_error)?,
                )
            }
            "goal.createInstrumental" => {
                let input: InstrumentalGoalCreateParam = decode(params)?;
                encode(
                    self.engine
                        .create_instrumental_goal(
                            &input.statement,
                            &input.parent_goal_id,
                            &input.derivation_reason,
                        )
                        .map_err(engine_error)?,
                )
            }
            "goal.list" => encode(self.engine.list_goals().map_err(engine_error)?),
            "goal.learningRecords" => encode(
                self.engine
                    .list_learning_goal_records()
                    .map_err(engine_error)?,
            ),
            "goal.derivationRecords" => encode(
                self.engine
                    .list_goal_derivation_records()
                    .map_err(engine_error)?,
            ),
            "curiosity.record" => {
                let input: CuriosityGap = decode(params)?;
                self.engine
                    .record_curiosity_gap(&input)
                    .map_err(engine_error)?;
                Ok(json!({"recorded": true}))
            }
            "curiosity.rank" => {
                let input: CuriosityRankParam = decode(params)?;
                encode(
                    self.engine
                        .rank_curiosity_gaps(input.limit.unwrap_or(32))
                        .map_err(engine_error)?,
                )
            }
            "intuition.evaluateRanking" => {
                let input: RankingEvaluationParam = decode(params)?;
                encode(
                    self.engine
                        .evaluate_recall_ranking(
                            &input.query,
                            input.candidate_limit,
                            input.holdout_examples,
                        )
                        .map_err(engine_error)?,
                )
            }
            "intuition.trainRepresentation" => {
                let input: RepresentationTrainingParam = decode(params)?;
                encode(
                    self.engine
                        .train_representation_model(input.holdout_tasks)
                        .map_err(engine_error)?,
                )
            }
            "intuition.latestRepresentation" => encode(
                self.engine
                    .latest_representation_model()
                    .map_err(engine_error)?,
            ),
            "intuition.evaluateRepresentation" => {
                let input: RepresentationRegressionParam = decode(params)?;
                encode(
                    self.engine
                        .evaluate_representation_model(input.model_id, input.holdout_queries)
                        .map_err(engine_error)?,
                )
            }
            "intuition.evaluateSemanticRecall" => {
                let input: SemanticRecallParam = decode(params)?;
                encode(
                    self.engine
                        .evaluate_semantic_recall(input.candidate_limit, input.holdout_queries)
                        .map_err(engine_error)?,
                )
            }
            "intuition.activateRepresentation" => {
                let input: RepresentationModelIdParam = decode(params)?;
                encode(
                    self.engine
                        .activate_representation_model(input.model_id)
                        .map_err(engine_error)?,
                )
            }
            "consolidation.discover" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .discover_skill_candidates(input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.compressionPlan" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .plan_episode_compression(input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.compress" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .compress_episode_history(input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.compressedList" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .list_episode_compression_records(input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "regression.list" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .list_verified_answers(input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.register" => {
                let input: SkillCandidate = decode(params)?;
                encode(
                    self.engine
                        .register_skill_candidate(&input)
                        .map_err(engine_error)?,
                )
            }
            "consolidation.list" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .list_managed_skills(input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.listActive" => {
                let input: LimitParam = decode(params)?;
                encode(
                    self.engine
                        .list_active_managed_skills(input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.rankActive" => {
                let input: SkillRankParam = decode(params)?;
                encode(
                    self.engine
                        .rank_active_managed_skills(&input.query, input.limit.unwrap_or(128))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.executeBest" => {
                let input: SkillExecuteBestParam = decode(params)?;
                encode(
                    self.engine
                        .execute_best_managed_skill(&input.query, input.inputs, input.prediction)
                        .map_err(engine_error)?,
                )
            }
            "consolidation.registerSingle" => {
                let input: EpisodeIdParam = decode(params)?;
                encode(
                    self.engine
                        .register_single_success_skill(EpisodeId(parse_uuid(&input.episode_id)?))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.registerFailureCritic" => {
                let input: EpisodeIdParam = decode(params)?;
                encode(
                    self.engine
                        .register_failure_critic_skill(EpisodeId(parse_uuid(&input.episode_id)?))
                        .map_err(engine_error)?,
                )
            }
            "consolidation.execute" => {
                let input: SkillExecuteParam = decode(params)?;
                encode(
                    self.engine
                        .execute_managed_skill(&input.skill_id, input.inputs, input.prediction)
                        .map_err(engine_error)?,
                )
            }
            "consolidation.evaluateShadow" => {
                let input: SkillShadowReplayParam = decode(params)?;
                encode(
                    self.engine
                        .evaluate_skill_for_shadow(&input.skill_id, input.replays)
                        .map_err(engine_error)?,
                )
            }
            "consolidation.promoteLive" => {
                let input: SkillShadowWinParam = decode(params)?;
                encode(
                    self.engine
                        .record_skill_shadow_live_win(
                            &input.skill_id,
                            input.observed_result,
                            input.scope,
                            input.evaluation,
                            &input.verifier_identity,
                        )
                        .map_err(engine_error)?,
                )
            }
            "consolidation.retire" => {
                let input: SkillRetireParam = decode(params)?;
                encode(
                    self.engine
                        .retire_managed_skill(
                            &input.skill_id,
                            &input.successor_skill,
                            &input.reason,
                        )
                        .map_err(engine_error)?,
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
                    .admin_insert_procedure(&procedure)
                    .map_err(engine_error)?;
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
                let input: ReviseProcedureParams = decode(params)?;
                self.engine
                    .admin_revise_procedure(&input.procedure, input.expected_version)
                    .map_err(engine_error)?;
                encode(VersionedProcedureWire {
                    version: input.procedure.version,
                    procedure: input.procedure,
                })
            }
            "procedure.getVersion" => {
                let input: ProcedureVersionParam = decode(params)?;
                let procedure = self
                    .engine
                    .graph()
                    .get_procedure_version(input.procedure_id()?, input.version)
                    .map_err(graph_error)?;
                encode(procedure.map(|procedure| VersionedProcedureWire {
                    version: input.version,
                    procedure,
                }))
            }
            "procedure.listVersions" => {
                let input: ProcedureIdParam = decode(params)?;
                encode(
                    self.engine
                        .graph()
                        .list_procedure_versions(input.procedure_id()?)
                        .map_err(graph_error)?
                        .into_iter()
                        .map(|procedure| VersionedProcedureWire {
                            version: procedure.version,
                            procedure,
                        })
                        .collect::<Vec<_>>(),
                )
            }
            "procedure.delete" => {
                let input: ProcedureIdParam = decode(params)?;
                self.engine
                    .admin_delete_procedure(input.procedure_id()?)
                    .map_err(engine_error)?;
                Ok(json!({ "deleted": true }))
            }
            "procedure.execute" => {
                let input: ExecuteParams = decode(params)?;
                encode(
                    self.engine
                        .execute_procedure_with_capability_runtime(
                            input.procedure_id()?,
                            input.inputs,
                            input.prediction,
                            input.permission_mode,
                        )
                        .map_err(engine_error)?,
                )
            }
            "observation.recordAuthenticated" => {
                let input: AuthenticatedObservationParams = decode(params)?;
                encode(
                    self.engine
                        .record_authenticated_observation(
                            input.predicate,
                            input.value,
                            input.scope,
                            input.evaluation,
                            &input.verifier_identity,
                        )
                        .map_err(engine_error)?,
                )
            }
            "feedback.record" => {
                let input: RecordFeedbackParams = decode(params)?;
                let episode = self
                    .engine
                    .episodes()
                    .get(input.episode_id()?)
                    .map_err(app_error)?;
                let success = episode.prediction.as_ref() == Some(&input.observed_result);
                let evaluation = Evaluation {
                    tier: spoon_core::VerifiabilityTier::Deferred,
                    success,
                    details: "raw external observation; trust assigned by server".into(),
                    surprise: episode
                        .prediction
                        .as_ref()
                        .map(|_| if success { 0.0 } else { 1.0 }),
                };
                let feedback = EpisodeFeedback::new(
                    episode.id,
                    input.observed_result,
                    evaluation,
                    FeedbackSource::new(
                        "rpc_observation",
                        Some(self.feedback_source_identity.clone()),
                    ),
                    input.idempotency_key,
                );
                let stored = self
                    .engine
                    .record_external_feedback(&feedback)
                    .map_err(app_error)?;
                encode(EpisodeFeedbackWire::from(stored))
            }
            "credit.analyze" => {
                let idempotency_key = take_optional_idempotency_key(&mut params)?;
                let input: FailureAnalysisRequest = decode(params)?;
                let analysis = match idempotency_key {
                    Some(key) => self.engine.analyze_failure_idempotent(&key, input),
                    None => self.engine.analyze_failure(input),
                }
                .map_err(credit_error)?;
                encode(analysis)
            }
            "credit.get" => {
                let input: CreditAnalysisIdParam = decode(params)?;
                encode(
                    self.engine
                        .get_failure_analysis(&input.analysis_id)
                        .map_err(credit_error)?,
                )
            }
            "credit.getByKey" => {
                let input: CreditAnalysisKeyParam = decode(params)?;
                encode(
                    self.engine
                        .get_failure_analysis_by_key(&input.idempotency_key)
                        .map_err(credit_error)?,
                )
            }
            "adaptation.plan" => {
                let input: AdaptationPlanRequest = decode(params)?;
                encode(
                    self.engine
                        .plan_adaptation(input)
                        .map_err(adaptation_error)?,
                )
            }
            "adaptation.get" => {
                let input: AdaptationPlanIdParam = decode(params)?;
                encode(
                    self.engine
                        .get_adaptation(input.plan_id()?)
                        .map_err(adaptation_error)?,
                )
            }
            "adaptation.apply" => {
                let input: ApplyAdaptationParams = decode(params)?;
                encode(
                    self.engine
                        .apply_adaptation(input.into_request()?)
                        .map_err(adaptation_error)?,
                )
            }
            "adaptation.applyOffline" => {
                let input: ApplyAdaptationParams = decode(params)?;
                let request = input.into_request()?;
                let capability = self
                    .engine
                    .issue_offline_capability(&request)
                    .map_err(adaptation_error)?;
                encode(
                    self.engine
                        .apply_adaptation_offline(request, &capability)
                        .map_err(adaptation_error)?,
                )
            }
            "contradiction.list" => {
                let _: EmptyParams = decode(params)?;
                let contradictions = self
                    .engine
                    .list_held_contradictions()
                    .map_err(contradiction_error)?;
                encode(
                    contradictions
                        .into_iter()
                        .map(ContradictionWire::from)
                        .collect::<Vec<_>>(),
                )
            }
            "contradiction.get" => {
                let input: ContradictionIdParam = decode(params)?;
                let contradiction = self
                    .engine
                    .get_contradiction(input.contradiction_id()?)
                    .map_err(contradiction_error)?;
                encode(contradiction.map(ContradictionWire::from))
            }
            "contradiction.record" => {
                let input: RecordContradictionParams = decode(params)?;
                encode(ContradictionWire::from(
                    self.engine
                        .admin_record_contradiction(
                            input.left.into(),
                            input.right.into(),
                            input.created_at,
                        )
                        .map_err(contradiction_error)?,
                ))
            }
            "contradiction.refine" => {
                let input: RefineContradictionParams = decode(params)?;
                encode(RefinementWire::from(
                    self.engine
                        .admin_refine_contradiction(
                            input.contradiction_id()?,
                            input.discriminator.into_feature()?,
                            input.updated_at,
                        )
                        .map_err(contradiction_error)?,
                ))
            }
            "contradiction.uncertainty" => {
                let input: ClaimIdParam = decode(params)?;
                encode(UncertaintyWire::from(
                    self.engine
                        .uncertainty_for_claim(&input.claim_id)
                        .map_err(contradiction_error)?,
                ))
            }
            "session.create" => {
                let input: CreateSessionParams = decode(params)?;
                encode(
                    self.engine
                        .create_session(input.name, input.visibility)
                        .map_err(app_error)?,
                )
            }
            "session.list" => encode(self.engine.list_sessions().map_err(app_error)?),
            "session.get" => {
                let input: SessionLookupParams = decode(params)?;
                encode(
                    self.engine
                        .get_session(&input.id_or_name)
                        .map_err(app_error)?,
                )
            }
            "session.end" => {
                let input: SessionLookupParams = decode(params)?;
                encode(
                    self.engine
                        .end_session(&input.id_or_name)
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
            "cycle.begin" => {
                let input: BeginCycleParams = decode(params)?;
                let progress = self
                    .engine
                    .begin_cycle(input.into_cycle_input())
                    .map_err(app_error)?;
                encode_cycle_progress(progress)
            }
            "cycle.resume" => {
                let input: ResumeCycleParams = decode(params)?;
                let progress = self
                    .engine
                    .resume_cycle(input.cycle_id()?, input.proposal)
                    .map_err(app_error)?;
                encode_cycle_progress(progress)
            }
            "cycle.resumeIntent" => {
                let input: ResumeIntentParams = decode(params)?;
                let progress = self
                    .engine
                    .resume_intent(input.cycle_id()?, input.proposal)
                    .map_err(app_error)?;
                encode_cycle_progress(progress)
            }
            "cycle.skipIntent" => {
                let input: SkipIntentParams = decode(params)?;
                let progress = self
                    .engine
                    .skip_intent_with_diagnostic(input.cycle_id()?, input.reason, input.diagnostic)
                    .map_err(app_error)?;
                encode_cycle_progress(progress)
            }
            "cycle.abort" => {
                let input: AbortCycleParams = decode(params)?;
                let progress = self
                    .engine
                    .abort_cycle(input.cycle_id()?, input.reason)
                    .map_err(app_error)?;
                encode_cycle_progress(progress)
            }
            _ => Err(RpcFault::new(-32601, "method not found").with_data(json!({
                "kind": "method_not_found",
                "method": method,
            }))),
        }
    }

    fn authorize_admin(&self, params: &mut Value) -> Result<(), RpcFault> {
        let supplied = params
            .as_object_mut()
            .and_then(|object| object.remove("adminToken"))
            .and_then(|value| value.as_str().map(str::to_owned));
        if self.admin_token.as_deref().is_some()
            && self.admin_token.as_deref() == supplied.as_deref()
        {
            return Ok(());
        }
        Err(
            RpcFault::new(-32001, "admin authorization required").with_data(json!({
                "kind": "admin_authorization_required"
            })),
        )
    }

    /// Register one concrete host-scoped `web.fetch` procedure for teaching.
    /// This is an administrative mutation, but deliberately does not grant
    /// network permission: the resulting procedure remains subject to the
    /// normal ask/workspace/full-access decision at execution time.
    fn provision_web_fetch(&self, host: &str) -> Result<Value, RpcFault> {
        let host = host.trim();
        if !valid_fetch_host(host) {
            return Err(
                RpcFault::new(-32602, "invalid web fetch host").with_data(json!({
                    "kind": "invalid_web_fetch_host"
                })),
            );
        }
        let policy = self
            .capability_adapters
            .policy(&NativePrimitive::NetworkRequest)
            .ok_or_else(capability_adapter_unavailable)?;
        if !policy.network_hosts.contains(host) {
            return Err(RpcFault::new(
                -32602,
                "web fetch host is not configured for this Spoon server",
            )
            .with_data(json!({
                "kind": "web_fetch_host_not_configured",
                "host": host,
            })));
        }
        let description = InterfaceDescription {
            source: "spoon-server:web-fetch-provisioner".into(),
            fingerprint: format!("web-fetch:{host}:v1"),
            operations: vec![DiscoveredOperation {
                name: "web.fetch".into(),
                input_schema: json!({"type": "object"}),
                output_schema: json!({"type": "object"}),
                host: host.into(),
                method: "GET".into(),
                response_fixture: json!({"status": 200, "headers": {}, "body": ""}),
            }],
        };
        let bundle = self
            .engine
            .discover_capability(&description)
            .map_err(engine_error)?;
        let imported = self
            .engine
            .import_capability_bundle(&spoon_engine::export_bundle(&bundle).map_err(app_error)?)
            .map_err(engine_error)?;
        let validation_episode = self
            .engine
            .record_authenticated_observation(
                "capability.fixture",
                SpoonValue::Bool(true),
                BTreeMap::from([("host".into(), SpoonValue::Text(host.into()))]),
                Evaluation {
                    tier: spoon_core::VerifiabilityTier::Hard,
                    success: true,
                    details: "host-scoped web.fetch fixture validated".into(),
                    surprise: Some(0.0),
                },
                "spoon-server:web-fetch-provisioner",
            )
            .map_err(engine_error)?;
        let revalidated = self
            .engine
            .revalidate_capability(
                &imported.content_id,
                &LocalValidation {
                    passed: true,
                    validation_episodes: vec![validation_episode.id.to_string()],
                    environment_digest: format!("web-fetch:{host}:v1"),
                },
            )
            .map_err(engine_error)?;
        let procedure = bundle
            .procedures
            .first()
            .ok_or_else(|| RpcFault::new(-32603, "web fetch provisioning produced no procedure"))?;
        encode(json!({
            "capability": revalidated,
            "procedureId": procedure.id,
            "permissionGranted": false,
            "executionRequirement": "pass --permission-mode full-access or grant the declared network host"
        }))
    }

    fn native_capability_boundaries(&self) -> Vec<Value> {
        [
            ("network_request", NativePrimitive::NetworkRequest),
            ("file_read", NativePrimitive::FileRead),
            ("file_write", NativePrimitive::FileWrite),
            ("observe", NativePrimitive::Observe),
            ("sandbox_execute", NativePrimitive::SandboxExecute),
        ]
        .into_iter()
        .map(|(kind, primitive)| {
            json!({
                "kind": kind,
                "hostAdapterConfigured": self.capability_adapters.policy(&primitive).is_some(),
            })
        })
        .collect()
    }
}

fn requires_admin(method: &str) -> bool {
    matches!(
        method,
        "concept.create"
            | "concept.update"
            | "concept.delete"
            | "relationship.create"
            | "relationship.update"
            | "relationship.delete"
            | "procedure.create"
            | "procedure.update"
            | "procedure.delete"
            | "capability.grant"
            | "capability.revoke"
            | "capability.provisionWebFetch"
            | "observation.recordAuthenticated"
            | "intuition.activateRepresentation"
            | "consolidation.register"
            | "consolidation.registerSingle"
            | "consolidation.registerFailureCritic"
            | "consolidation.evaluateShadow"
            | "consolidation.promoteLive"
            | "consolidation.retire"
            | "consolidation.compress"
            | "adaptation.applyOffline"
            | "contradiction.record"
            | "contradiction.refine"
    )
}

#[derive(Debug, Deserialize)]
struct CapabilityBundleParam {
    bundle: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProvisionWebFetchParam {
    host: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityImportRevalidateParam {
    bundle: CapabilityBundle,
    validation: LocalValidation,
}

#[derive(Debug, Deserialize)]
struct CapabilityIdParam {
    #[serde(rename = "contentId")]
    content_id: String,
}

#[derive(Debug, Deserialize)]
struct CapabilityRevalidateParam {
    #[serde(rename = "contentId")]
    content_id: String,
    validation: LocalValidation,
}

#[derive(Debug, Deserialize)]
struct CapabilityPermissionParam {
    #[serde(rename = "contentId")]
    content_id: String,
    permission: Permission,
}

#[derive(Debug, Deserialize)]
struct CapabilityProcedureParam {
    #[serde(rename = "contentId")]
    content_id: String,
    #[serde(rename = "procedureId")]
    procedure_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityInvokeParam {
    content_id: String,
    procedure_id: String,
    input: Value,
}

/// Public wrapper for a deterministic rendering request. The response plan is
/// data, not an authority grant: evidence and provenance references are
/// required for plan validation but are never treated as server-verified.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LanguageRenderParam {
    plan: ResponsePlan,
    #[serde(default)]
    options: Option<LanguageRenderOptions>,
}

/// Content-free overrides. Tone remains metadata in the current deterministic
/// renderer; `variant` selects plain or bullet formatting only.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LanguageRenderOptions {
    #[serde(default)]
    tone: Option<ResponseTone>,
    #[serde(default)]
    variant: Option<RenderVariant>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillExecuteParam {
    skill_id: String,
    #[serde(default)]
    inputs: BTreeMap<String, SpoonValue>,
    prediction: Option<SpoonValue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillRankParam {
    query: String,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillExecuteBestParam {
    query: String,
    #[serde(default)]
    inputs: BTreeMap<String, SpoonValue>,
    prediction: Option<SpoonValue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalCreateParam {
    kind: GoalKind,
    statement: String,
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LearningGoalCreateParam {
    statement: String,
    standing_goal_id: String,
    source_gap_id: String,
    derivation_reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstrumentalGoalCreateParam {
    statement: String,
    parent_goal_id: String,
    derivation_reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CuriosityRankParam {
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillShadowReplayParam {
    skill_id: String,
    replays: Vec<PromotionReplay>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillRetireParam {
    skill_id: String,
    successor_skill: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillShadowWinParam {
    skill_id: String,
    observed_result: SpoonValue,
    #[serde(default)]
    scope: BTreeMap<String, SpoonValue>,
    evaluation: Evaluation,
    verifier_identity: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RankingEvaluationParam {
    query: String,
    candidate_limit: usize,
    holdout_examples: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepresentationTrainingParam {
    holdout_tasks: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepresentationModelIdParam {
    model_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepresentationRegressionParam {
    model_id: i64,
    holdout_queries: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SemanticRecallParam {
    candidate_limit: usize,
    holdout_queries: usize,
}

#[derive(Debug, Deserialize)]
struct LimitParam {
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct PrimitiveObserveParam {
    target: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FalsificationMeasurementParam {
    run_id: String,
    measurement: FalsificationMeasurementInput,
}

pub fn run_stdio<R: BufRead, W: Write>(
    server: &mut RpcServer,
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

fn encode_cycle_progress(progress: CycleProgress) -> Result<Value, RpcFault> {
    match progress {
        CycleProgress::NeedIntent { cycle_id, request } => Ok(json!({
            "status": "need_intent",
            "cycleId": cycle_id,
            "request": request,
        })),
        CycleProgress::NeedTeacher { cycle_id, request } => Ok(json!({
            "status": "need_teacher",
            "cycleId": cycle_id,
            "request": request,
        })),
        CycleProgress::Completed(outcome) => encode(CompletedCycleWire::from(*outcome)),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompletedCycleWire {
    status: &'static str,
    cycle_id: CycleId,
    disposition: spoon_engine::CycleDisposition,
    answer: Option<SpoonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    procedure_ir: Option<Value>,
    episode: spoon_core::Episode,
}

impl From<CycleOutcome> for CompletedCycleWire {
    fn from(outcome: CycleOutcome) -> Self {
        Self {
            status: "completed",
            cycle_id: outcome.cycle_id,
            disposition: outcome.disposition,
            answer: outcome.answer,
            procedure_ir: outcome.procedure_ir,
            episode: outcome.episode,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeFeedbackWire {
    id: Uuid,
    episode_id: EpisodeId,
    observed_result: SpoonValue,
    evaluation: Evaluation,
    source: FeedbackSource,
    idempotency_key: String,
    created_at: i64,
}

impl From<EpisodeFeedback> for EpisodeFeedbackWire {
    fn from(feedback: EpisodeFeedback) -> Self {
        Self {
            id: feedback.id,
            episode_id: feedback.episode_id,
            observed_result: feedback.observed_result,
            evaluation: feedback.evaluation,
            source: feedback.source,
            idempotency_key: feedback.idempotency_key,
            created_at: feedback.created_at,
        }
    }
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
    serde_json::from_value(params).map_err(|error| {
        RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "invalid_params",
            "cause": error.to_string(),
        }))
    })
}

fn take_optional_idempotency_key(params: &mut Value) -> Result<Option<String>, RpcFault> {
    let Some(object) = params.as_object_mut() else {
        return Err(RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "invalid_params",
            "cause": "params must be an object",
        })));
    };
    let Some(value) = object.remove("idempotencyKey") else {
        return Ok(None);
    };
    let Some(key) = value.as_str() else {
        return Err(RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "invalid_params",
            "cause": "idempotencyKey must be a string",
        })));
    };
    if key.trim().is_empty() {
        return Err(RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "invalid_params",
            "cause": "idempotencyKey cannot be empty",
        })));
    }
    Ok(Some(key.to_owned()))
}

fn encode<T: Serialize>(value: T) -> Result<Value, RpcFault> {
    serde_json::to_value(value).map_err(|error| {
        RpcFault::new(-32603, "serialization failed").with_data(json!({
            "kind": "serialization_failed",
            "cause": error.to_string(),
        }))
    })
}

fn validate_capability_invocation_request(input: &CapabilityInvokeParam) -> Result<(), RpcFault> {
    let digest = input.content_id.strip_prefix("sha256:");
    if digest.is_none_or(|digest| {
        digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err(RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "invalid_capability_identity"
        })));
    }
    if input.procedure_id.is_empty()
        || input.procedure_id.len() > MAX_PUBLIC_PROCEDURE_ID_BYTES
        || input.procedure_id.chars().any(char::is_control)
    {
        return Err(RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "invalid_capability_procedure_identity"
        })));
    }
    let input_bytes = serde_json::to_vec(&input.input).map_err(|_| {
        RpcFault::new(-32602, "invalid params")
            .with_data(json!({"kind": "invalid_capability_input"}))
    })?;
    if input_bytes.len() > MAX_PUBLIC_CAPABILITY_INPUT_BYTES {
        return Err(RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "capability_input_too_large",
            "maxBytes": MAX_PUBLIC_CAPABILITY_INPUT_BYTES
        })));
    }
    Ok(())
}

fn validate_language_render_request(input: &LanguageRenderParam) -> Result<(), RpcFault> {
    let encoded = serde_json::to_vec(input).map_err(|_| {
        RpcFault::new(-32602, "invalid params")
            .with_data(json!({"kind": "invalid_language_render_request"}))
    })?;
    if encoded.len() > MAX_PUBLIC_LANGUAGE_RENDER_INPUT_BYTES {
        return Err(RpcFault::new(-32602, "invalid params").with_data(json!({
            "kind": "language_render_input_too_large",
            "maxBytes": MAX_PUBLIC_LANGUAGE_RENDER_INPUT_BYTES,
        })));
    }
    Ok(())
}

/// This endpoint deliberately does not resolve caller-provided evidence or
/// provenance identifiers into trusted Engine facts. It only proves that the
/// bounded renderer accepted references and did not emit unsupported claims.
/// Consequently the public audit reports their status as unverified and never
/// returns raw evidence/provenance fields.
fn public_language_render_response(
    dialogue_move: DialogueMove,
    rendered: RenderedResponse,
    claims_submitted: usize,
) -> Value {
    json!({
        "text": rendered.text,
        "includedClaimIds": rendered.included_claim_ids,
        "omittedClaimIds": rendered.omitted_claim_ids,
        "uncertainty": rendered.uncertainty,
        "tone": rendered.tone,
        "dialogueMove": dialogue_move,
        "audit": {
            "renderer": "bounded_response_plan_v1",
            "claimsSubmitted": claims_submitted,
            "evidenceStatus": "caller_supplied_unverified",
            "provenanceRedacted": true,
            "redacted": true,
        }
    })
}

fn language_render_error(_: LanguageError) -> RpcFault {
    RpcFault::new(-32602, "invalid language response plan").with_data(json!({
        "kind": "invalid_language_response_plan",
        "redacted": true,
    }))
}

fn public_capability_invocation(outcome: CapabilityExecutionOutcome) -> Value {
    let invocation = outcome.invocation;
    json!({
        "contentId": invocation.content_id,
        "procedureId": invocation.procedure_id,
        "output": invocation.output,
        "outputDigest": invocation.output_digest,
        "receipt": {
            "primitive": invocation.receipt.primitive,
            "effect": invocation.receipt.effect,
            "payloadDigest": invocation.receipt.payload_digest,
            "bounds": invocation.receipt.bounds,
            "redacted": true,
            "replayable": invocation.receipt.replayable,
        },
        "usage": invocation.usage,
        "episodeId": outcome.episode.id,
        "redacted": true,
    })
}

fn capability_adapter_unavailable() -> RpcFault {
    RpcFault::new(-32020, "capability adapter unavailable").with_data(json!({
        "kind": "capability_adapter_unavailable"
    }))
}

fn capability_authorization_error(_: EngineError) -> RpcFault {
    RpcFault::new(-32021, "capability authorization failed").with_data(json!({
        "kind": "capability_authorization_failed",
        "redacted": true
    }))
}

fn capability_invocation_error(error: EngineError) -> RpcFault {
    match error {
        EngineError::CapabilityInvocationFailed { episode_id, .. } => {
            RpcFault::new(-32022, "capability invocation failed").with_data(json!({
                "kind": "capability_invocation_failed",
                "episodeId": episode_id,
                "redacted": true
            }))
        }
        _ => RpcFault::new(-32022, "capability invocation failed").with_data(json!({
            "kind": "capability_invocation_failed",
            "redacted": true
        })),
    }
}

fn app_error(error: impl std::fmt::Display) -> RpcFault {
    RpcFault::new(-32000, "application error").with_data(json!({
        "kind": "application_error",
        "cause": error.to_string(),
    }))
}

fn graph_error(error: GraphError) -> RpcFault {
    match error {
        GraphError::RevisionConflict {
            entity,
            expected,
            actual,
        } => RpcFault::new(-32002, "graph revision conflict").with_data(json!({
            "kind": "revision_conflict",
            "entity": entity,
            "expectedVersion": expected,
            "actualVersion": actual,
        })),
        GraphError::NotFound(entity) => {
            RpcFault::new(-32004, "graph entity not found").with_data(json!({
                "kind": "graph_not_found",
                "entity": entity,
            }))
        }
        GraphError::ImmutableFieldChange { entity, field } => {
            RpcFault::new(-32602, "immutable graph field changed").with_data(json!({
                "kind": "immutable_field_change",
                "entity": entity,
                "field": field,
            }))
        }
        GraphError::NonMonotonicRevision {
            entity,
            expected_next,
            proposed,
        } => RpcFault::new(-32602, "invalid graph revision").with_data(json!({
            "kind": "non_monotonic_revision",
            "entity": entity,
            "expectedVersion": expected_next,
            "proposedVersion": proposed,
        })),
        other => RpcFault::new(-32000, "graph operation failed").with_data(json!({
            "kind": "graph_error",
            "cause": other.to_string(),
        })),
    }
}

fn engine_error(error: EngineError) -> RpcFault {
    match error {
        EngineError::ExecutionFailed { episode_id, source } => {
            RpcFault::new(-32010, "execution failed").with_data(json!({
                "kind": "execution_failed",
                "episodeId": episode_id,
                "cause": source.to_string(),
            }))
        }
        EngineError::Graph(error) => graph_error(error),
        other => app_error(other),
    }
}

fn credit_error(error: EngineError) -> RpcFault {
    match error {
        EngineError::InvalidInput(cause)
            if cause.contains("idempotency key") && cause.contains("already bound") =>
        {
            RpcFault::new(-32015, "credit analysis idempotency conflict").with_data(json!({
                "kind": "credit_idempotency_conflict",
                "cause": cause,
            }))
        }
        EngineError::InvalidInput(cause) => {
            RpcFault::new(-32602, "invalid params").with_data(json!({
                "kind": "invalid_params",
                "cause": cause,
            }))
        }
        other => RpcFault::new(-32014, "credit analysis failed").with_data(json!({
            "kind": "credit_analysis_error",
            "cause": other.to_string(),
        })),
    }
}

fn adaptation_error(error: EngineError) -> RpcFault {
    match error {
        EngineError::Adapt(spoon_adapt::AdaptError::NotFound(identifier)) => {
            RpcFault::new(-32024, "adaptation not found").with_data(json!({
                "kind": "adaptation_not_found",
                "identifier": identifier,
            }))
        }
        EngineError::Adapt(spoon_adapt::AdaptError::Invalid(detail))
        | EngineError::InvalidInput(detail) => RpcFault::new(-32602, "invalid adaptation")
            .with_data(json!({
                "kind": "invalid_adaptation",
                "detail": detail,
            })),
        EngineError::Adapt(spoon_adapt::AdaptError::Unauthorized(detail)) => {
            RpcFault::new(-32021, "adaptation authorization rejected").with_data(json!({
                "kind": "adaptation_unauthorized",
                "detail": detail,
            }))
        }
        EngineError::Adapt(spoon_adapt::AdaptError::OfflineCapabilityRequired(detail)) => {
            RpcFault::new(-32022, "offline adaptation required").with_data(json!({
                "kind": "offline_adaptation_required",
                "detail": detail,
            }))
        }
        EngineError::Adapt(spoon_adapt::AdaptError::Graph(error)) | EngineError::Graph(error) => {
            graph_error(error)
        }
        other => RpcFault::new(-32020, "adaptation failed").with_data(json!({
            "kind": "adaptation_internal_error",
            "cause": other.to_string(),
        })),
    }
}

fn contradiction_error(error: EngineError) -> RpcFault {
    match error {
        EngineError::Adapt(spoon_adapt::AdaptError::NotFound(identifier)) => {
            RpcFault::new(-32034, "contradiction not found").with_data(json!({
                "kind": "contradiction_not_found",
                "identifier": identifier,
            }))
        }
        EngineError::Adapt(spoon_adapt::AdaptError::Invalid(detail))
        | EngineError::InvalidInput(detail) => RpcFault::new(-32602, "invalid contradiction")
            .with_data(json!({
                "kind": "invalid_contradiction",
                "detail": detail,
            })),
        EngineError::Adapt(spoon_adapt::AdaptError::Unauthorized(detail)) => {
            RpcFault::new(-32031, "contradiction authorization rejected").with_data(json!({
                "kind": "contradiction_unauthorized",
                "detail": detail,
            }))
        }
        other => RpcFault::new(-32030, "contradiction operation failed").with_data(json!({
            "kind": "contradiction_internal_error",
            "cause": other.to_string(),
        })),
    }
}

fn serialize_response(response: RpcResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal error"}}"#.into()
    })
}

fn parse_uuid(value: &str) -> Result<Uuid, RpcFault> {
    Uuid::parse_str(value).map_err(|error| {
        RpcFault::new(-32602, "invalid id").with_data(json!({
            "kind": "invalid_id",
            "cause": error.to_string(),
        }))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConceptIdParam {
    concept_id: String,
}
impl ConceptIdParam {
    fn concept_id(&self) -> Result<ConceptId, RpcFault> {
        Ok(ConceptId(parse_uuid(&self.concept_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConceptVersionParam {
    concept_id: String,
    version: u32,
}

impl ConceptVersionParam {
    fn concept_id(&self) -> Result<ConceptId, RpcFault> {
        Ok(ConceptId(parse_uuid(&self.concept_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviseConceptParams {
    concept: Concept,
    expected_version: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionedConceptWire {
    version: u32,
    concept: Concept,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelationshipIdParam {
    relationship_id: String,
}
impl RelationshipIdParam {
    fn relationship_id(&self) -> Result<RelationshipId, RpcFault> {
        Ok(RelationshipId(parse_uuid(&self.relationship_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelationshipVersionParam {
    relationship_id: String,
    version: u32,
}

impl RelationshipVersionParam {
    fn relationship_id(&self) -> Result<RelationshipId, RpcFault> {
        Ok(RelationshipId(parse_uuid(&self.relationship_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviseRelationshipParams {
    relationship: Relationship,
    expected_version: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionedRelationshipWire {
    version: u32,
    relationship: Relationship,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcedureIdParam {
    procedure_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcedureVersionParam {
    procedure_id: String,
    version: u32,
}

impl ProcedureVersionParam {
    fn procedure_id(&self) -> Result<ProcedureId, RpcFault> {
        Ok(ProcedureId(parse_uuid(&self.procedure_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviseProcedureParams {
    procedure: Procedure,
    expected_version: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionedProcedureWire {
    version: u32,
    procedure: Procedure,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdaptationPlanIdParam {
    plan_id: String,
}

impl AdaptationPlanIdParam {
    fn plan_id(&self) -> Result<AdaptationPlanId, RpcFault> {
        Ok(AdaptationPlanId(parse_uuid(&self.plan_id)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyAdaptationParams {
    plan_id: String,
    idempotency_key: String,
    applied_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreditAnalysisIdParam {
    analysis_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreditAnalysisKeyParam {
    idempotency_key: String,
}

impl ApplyAdaptationParams {
    fn into_request(self) -> Result<ApplyAdaptationRequest, RpcFault> {
        Ok(ApplyAdaptationRequest {
            plan_id: AdaptationPlanId(parse_uuid(&self.plan_id)?),
            idempotency_key: self.idempotency_key,
            applied_at: self.applied_at,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContradictionIdParam {
    contradiction_id: i64,
}

impl ContradictionIdParam {
    fn contradiction_id(&self) -> Result<ContradictionId, RpcFault> {
        positive_contradiction_id(self.contradiction_id)
    }
}

fn positive_contradiction_id(value: i64) -> Result<ContradictionId, RpcFault> {
    if value <= 0 {
        return Err(
            RpcFault::new(-32602, "invalid contradiction id").with_data(json!({
                "kind": "invalid_contradiction_id",
            })),
        );
    }
    Ok(ContradictionId(value))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordContradictionParams {
    left: ClaimParams,
    right: ClaimParams,
    created_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClaimParams {
    id: String,
    statement: String,
    implication: ImplicationParams,
    supporting_episodes: Vec<EpisodeId>,
    #[serde(default)]
    scope: Vec<ScopeAssignmentParams>,
}

impl From<ClaimParams> for Claim {
    fn from(value: ClaimParams) -> Self {
        Self {
            id: value.id,
            statement: value.statement,
            implication: value.implication.into(),
            supporting_episodes: value.supporting_episodes,
            scope: value.scope.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImplicationParams {
    predicate: String,
    value: SpoonValue,
}

impl From<ImplicationParams> for Implication {
    fn from(value: ImplicationParams) -> Self {
        Self {
            predicate: value.predicate,
            value: value.value,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopeAssignmentParams {
    feature: String,
    value: SpoonValue,
    learned_from: EpisodeId,
}

impl From<ScopeAssignmentParams> for ScopeAssignment {
    fn from(value: ScopeAssignmentParams) -> Self {
        Self {
            feature: value.feature,
            value: value.value,
            learned_from: value.learned_from,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RefineContradictionParams {
    contradiction_id: i64,
    discriminator: DemonstratedFeatureParams,
    updated_at: i64,
}

impl RefineContradictionParams {
    fn contradiction_id(&self) -> Result<ContradictionId, RpcFault> {
        positive_contradiction_id(self.contradiction_id)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DemonstratedFeatureParams {
    feature: String,
    left_value: SpoonValue,
    left_episode: EpisodeId,
    right_value: SpoonValue,
    right_episode: EpisodeId,
}

impl DemonstratedFeatureParams {
    fn into_feature(self) -> Result<DemonstratedFeature, RpcFault> {
        DemonstratedFeature::new(
            self.feature,
            self.left_value,
            self.left_episode,
            self.right_value,
            self.right_episode,
        )
        .map_err(|error| {
            RpcFault::new(-32602, "invalid discriminator").with_data(json!({
                "kind": "invalid_discriminator",
                "cause": error.to_string(),
            }))
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClaimIdParam {
    claim_id: String,
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum UncertaintyWire {
    Certain,
    HeldContradictions { contradiction_ids: Vec<i64> },
}

impl From<Uncertainty> for UncertaintyWire {
    fn from(value: Uncertainty) -> Self {
        match value {
            Uncertainty::Certain => Self::Certain,
            Uncertainty::HeldContradictions(ids) => Self::HeldContradictions {
                contradiction_ids: ids.into_iter().map(|id| id.0).collect(),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContradictionWire {
    id: i64,
    left: ClaimWire,
    right: ClaimWire,
    status: spoon_adapt::ContradictionStatus,
    refinement: Option<RefinementWire>,
    created_at: i64,
    updated_at: i64,
}

impl From<Contradiction> for ContradictionWire {
    fn from(value: Contradiction) -> Self {
        Self {
            id: value.id.0,
            left: value.left.into(),
            right: value.right.into(),
            status: value.status,
            refinement: value.refinement.map(RefinementWire::from),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimWire {
    id: String,
    statement: String,
    implication: ImplicationWire,
    supporting_episodes: Vec<EpisodeId>,
    scope: Vec<ScopeAssignmentWire>,
}

impl From<Claim> for ClaimWire {
    fn from(value: Claim) -> Self {
        Self {
            id: value.id,
            statement: value.statement,
            implication: value.implication.into(),
            supporting_episodes: value.supporting_episodes,
            scope: value
                .scope
                .into_iter()
                .map(ScopeAssignmentWire::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct ImplicationWire {
    predicate: String,
    value: SpoonValue,
}

impl From<Implication> for ImplicationWire {
    fn from(value: Implication) -> Self {
        Self {
            predicate: value.predicate,
            value: value.value,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopeAssignmentWire {
    feature: String,
    value: SpoonValue,
    learned_from: EpisodeId,
}

impl From<ScopeAssignment> for ScopeAssignmentWire {
    fn from(value: ScopeAssignment) -> Self {
        Self {
            feature: value.feature,
            value: value.value,
            learned_from: value.learned_from,
        }
    }
}

#[derive(Serialize)]
struct RefinementWire {
    left: ClaimWire,
    right: ClaimWire,
    discriminator: DemonstratedFeatureWire,
}

impl From<Refinement> for RefinementWire {
    fn from(value: Refinement) -> Self {
        Self {
            left: value.left.into(),
            right: value.right.into(),
            discriminator: value.discriminator.into(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DemonstratedFeatureWire {
    feature: String,
    left_value: SpoonValue,
    left_episode: EpisodeId,
    right_value: SpoonValue,
    right_episode: EpisodeId,
}

impl From<DemonstratedFeature> for DemonstratedFeatureWire {
    fn from(value: DemonstratedFeature) -> Self {
        Self {
            feature: value.feature,
            left_value: value.left_value,
            left_episode: value.left_episode,
            right_value: value.right_value,
            right_episode: value.right_episode,
        }
    }
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
    inputs: BTreeMap<String, SpoonValue>,
    prediction: Option<SpoonValue>,
    #[serde(default)]
    permission_mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordFeedbackParams {
    episode_id: String,
    observed_result: SpoonValue,
    idempotency_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthenticatedObservationParams {
    predicate: String,
    value: SpoonValue,
    #[serde(default)]
    scope: BTreeMap<String, SpoonValue>,
    evaluation: Evaluation,
    verifier_identity: String,
}

impl RecordFeedbackParams {
    fn episode_id(&self) -> Result<EpisodeId, RpcFault> {
        Ok(EpisodeId(parse_uuid(&self.episode_id)?))
    }
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
    substitutions: BTreeMap<String, SpoonValue>,
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
    session_id: Option<String>,
    session_visibility: Option<SessionVisibility>,
    #[serde(default)]
    include_isolated: bool,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CycleBudgetParams {
    max_exec_steps: u32,
    max_context_items: usize,
    max_teacher_turns: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BeginCycleParams {
    situation: String,
    #[serde(default)]
    working_directory: Option<String>,
    #[serde(default)]
    environment: BTreeMap<String, SpoonValue>,
    #[serde(default)]
    assumptions: Vec<Assumption>,
    budget: CycleBudgetParams,
    teacher_allowed: bool,
    #[serde(default)]
    interpreter_allowed: bool,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    recall_mode: RecallMode,
    #[serde(default)]
    permission_mode: Option<String>,
}

impl BeginCycleParams {
    fn into_cycle_input(self) -> CycleInput {
        CycleInput {
            situation: self.situation,
            working_directory: self.working_directory,
            environment: self.environment,
            assumptions: self.assumptions,
            budget: CycleBudget {
                max_exec_steps: self.budget.max_exec_steps,
                max_context_items: self.budget.max_context_items,
                max_teacher_turns: self.budget.max_teacher_turns,
            },
            teacher_allowed: self.teacher_allowed,
            interpreter_allowed: self.interpreter_allowed,
            session_id: self.session_id,
            recall_mode: self.recall_mode,
            permission_mode: self.permission_mode,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateSessionParams {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    visibility: SessionVisibility,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionLookupParams {
    id_or_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeCycleParams {
    cycle_id: String,
    proposal: TeacherProposalWire,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResumeIntentParams {
    cycle_id: String,
    proposal: IntentProposalWire,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkipIntentParams {
    cycle_id: String,
    reason: String,
    #[serde(default)]
    diagnostic: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AbortCycleParams {
    cycle_id: String,
    reason: String,
}

impl ResumeCycleParams {
    fn cycle_id(&self) -> Result<CycleId, RpcFault> {
        Ok(CycleId(parse_uuid(&self.cycle_id)?))
    }
}

impl ResumeIntentParams {
    fn cycle_id(&self) -> Result<CycleId, RpcFault> {
        Ok(CycleId(parse_uuid(&self.cycle_id)?))
    }
}

impl SkipIntentParams {
    fn cycle_id(&self) -> Result<CycleId, RpcFault> {
        Ok(CycleId(parse_uuid(&self.cycle_id)?))
    }
}

impl AbortCycleParams {
    fn cycle_id(&self) -> Result<CycleId, RpcFault> {
        Ok(CycleId(parse_uuid(&self.cycle_id)?))
    }
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
            session_id: self
                .session_id
                .map(|id| parse_uuid(&id).map(spoon_core::SessionId))
                .transpose()?,
            session_visibility: self.session_visibility,
            exclude_isolated: !self.include_isolated,
        })
    }
}
