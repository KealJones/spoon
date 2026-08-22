use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use ekg_adapt::{
    Claim, Contradiction, ContradictionId, DemonstratedFeature, Implication, Refinement,
    ScopeAssignment, Uncertainty,
};
use ekg_core::{
    Assumption, Concept, ConceptId, Contract, EpisodeId, EscalationRung, Evaluation, Expr,
    MutabilityClass, Param, Procedure, ProcedureId, Relationship, RelationshipId,
    Value as EkgValue,
};
use ekg_engine::{
    AdaptationPlanId, AdaptationPlanRequest, ApplyAdaptationRequest, CuriosityGap, CycleBudget,
    CycleId, CycleInput, CycleOutcome, CycleProgress, Engine, EngineError, FailureAnalysisRequest,
    GoalKind, InterfaceDescription, LocalValidation, Permission, PromotionReplay, SkillCandidate,
    TeacherProposalWire,
};
use ekg_episode::{EpisodeFeedback, EpisodeQuery, FeedbackSource};
use ekg_graph::GraphError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub struct RpcServer {
    engine: Engine,
    admin_token: Option<String>,
    feedback_source_identity: String,
}

impl RpcServer {
    pub fn from_engine(engine: Engine) -> Self {
        Self {
            engine,
            admin_token: None,
            feedback_source_identity: "ekg-server".into(),
        }
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
            "metrics.snapshot" => encode(self.engine.metrics_snapshot().map_err(engine_error)?),
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
                encode(self.engine.rank_active_managed_skills(&input.query, input.limit.unwrap_or(128)).map_err(engine_error)?)
            }
            "consolidation.executeBest" => {
                let input: SkillExecuteBestParam = decode(params)?;
                encode(self.engine.execute_best_managed_skill(&input.query, input.inputs, input.prediction).map_err(engine_error)?)
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
                        .execute_procedure(input.procedure_id()?, input.inputs, input.prediction)
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
                    tier: ekg_core::VerifiabilityTier::Deferred,
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
struct SkillExecuteParam {
    skill_id: String,
    #[serde(default)]
    inputs: BTreeMap<String, EkgValue>,
    prediction: Option<EkgValue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillRankParam { query: String, limit: Option<u32> }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillExecuteBestParam {
    query: String,
    #[serde(default)] inputs: BTreeMap<String, EkgValue>,
    prediction: Option<EkgValue>,
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
    observed_result: EkgValue,
    #[serde(default)]
    scope: BTreeMap<String, EkgValue>,
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
struct LimitParam {
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct PrimitiveObserveParam {
    target: String,
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
    disposition: ekg_engine::CycleDisposition,
    answer: Option<EkgValue>,
    episode: ekg_core::Episode,
}

impl From<CycleOutcome> for CompletedCycleWire {
    fn from(outcome: CycleOutcome) -> Self {
        Self {
            status: "completed",
            cycle_id: outcome.cycle_id,
            disposition: outcome.disposition,
            answer: outcome.answer,
            episode: outcome.episode,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeFeedbackWire {
    id: Uuid,
    episode_id: EpisodeId,
    observed_result: EkgValue,
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
        EngineError::Adapt(ekg_adapt::AdaptError::NotFound(identifier)) => {
            RpcFault::new(-32024, "adaptation not found").with_data(json!({
                "kind": "adaptation_not_found",
                "identifier": identifier,
            }))
        }
        EngineError::Adapt(ekg_adapt::AdaptError::Invalid(detail))
        | EngineError::InvalidInput(detail) => RpcFault::new(-32602, "invalid adaptation")
            .with_data(json!({
                "kind": "invalid_adaptation",
                "detail": detail,
            })),
        EngineError::Adapt(ekg_adapt::AdaptError::Unauthorized(detail)) => {
            RpcFault::new(-32021, "adaptation authorization rejected").with_data(json!({
                "kind": "adaptation_unauthorized",
                "detail": detail,
            }))
        }
        EngineError::Adapt(ekg_adapt::AdaptError::OfflineCapabilityRequired(detail)) => {
            RpcFault::new(-32022, "offline adaptation required").with_data(json!({
                "kind": "offline_adaptation_required",
                "detail": detail,
            }))
        }
        EngineError::Adapt(ekg_adapt::AdaptError::Graph(error)) | EngineError::Graph(error) => {
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
        EngineError::Adapt(ekg_adapt::AdaptError::NotFound(identifier)) => {
            RpcFault::new(-32034, "contradiction not found").with_data(json!({
                "kind": "contradiction_not_found",
                "identifier": identifier,
            }))
        }
        EngineError::Adapt(ekg_adapt::AdaptError::Invalid(detail))
        | EngineError::InvalidInput(detail) => RpcFault::new(-32602, "invalid contradiction")
            .with_data(json!({
                "kind": "invalid_contradiction",
                "detail": detail,
            })),
        EngineError::Adapt(ekg_adapt::AdaptError::Unauthorized(detail)) => {
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
    value: EkgValue,
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
    value: EkgValue,
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
    left_value: EkgValue,
    left_episode: EpisodeId,
    right_value: EkgValue,
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
    status: ekg_adapt::ContradictionStatus,
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
    value: EkgValue,
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
    value: EkgValue,
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
    left_value: EkgValue,
    left_episode: EpisodeId,
    right_value: EkgValue,
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
    inputs: BTreeMap<String, EkgValue>,
    prediction: Option<EkgValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordFeedbackParams {
    episode_id: String,
    observed_result: EkgValue,
    idempotency_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthenticatedObservationParams {
    predicate: String,
    value: EkgValue,
    #[serde(default)]
    scope: BTreeMap<String, EkgValue>,
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
    environment: BTreeMap<String, EkgValue>,
    #[serde(default)]
    assumptions: Vec<Assumption>,
    budget: CycleBudgetParams,
    teacher_allowed: bool,
}

impl BeginCycleParams {
    fn into_cycle_input(self) -> CycleInput {
        CycleInput {
            situation: self.situation,
            environment: self.environment,
            assumptions: self.assumptions,
            budget: CycleBudget {
                max_exec_steps: self.budget.max_exec_steps,
                max_context_items: self.budget.max_context_items,
                max_teacher_turns: self.budget.max_teacher_turns,
            },
            teacher_allowed: self.teacher_allowed,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeCycleParams {
    cycle_id: String,
    proposal: TeacherProposalWire,
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
        })
    }
}
