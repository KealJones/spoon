//! Per-part dispatch: running one utterance's speech acts independently.
//!
//! An utterance that is a greeting plus two questions has three outcomes, not
//! one. Some parts execute, some need the Teacher, some need the user. They
//! must be able to finish at different times without losing each other's work,
//! which is what this module owns.
//!
//! The state here is deliberately separate from the Engine's execution
//! internals. What to run next, what a failure blocks, how outcomes become
//! claims, and which evidence backs each claim are all decidable without
//! touching a procedure evaluator, so they are decided here where they can be
//! tested directly.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use spoon_core::language::{
    DialogueAct, DialogueMove, EvidenceReference, GroundedClaim, IntentDisposition, LanguageError,
    PlannedClaim, RenderVariant, ResponsePlan, ResponseTone, TextSpan, Uncertainty,
};
use spoon_core::realizer::ClaimDependencies;
use spoon_core::utterance::{MentionResolution, PartId, UtteranceAnalysis};
use spoon_core::{EpisodeId, SourceKind, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartState {
    /// Not dispatched yet.
    Pending,
    Executed,
    /// Needs the user before it can run.
    Clarified,
    /// Cannot run: refused, untaught, or out of budget.
    Abstained,
    /// A part it depends on did not produce a value.
    Blocked,
}

/// Where a claim's evidence comes from. Keeping this an enum rather than a
/// pre-built reference puts the spec's evidence table in one place instead of
/// scattering id formats across the Engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOrigin {
    /// Deterministic local execution.
    Procedure { id: String, version: u32 },
    /// An observation or capability receipt.
    Observation {
        fact_id: String,
        receipt: Option<String>,
    },
    /// The utterance itself. A greeting is grounded in the observable fact that
    /// the user greeted, which is honest provenance rather than no provenance.
    Utterance { span: TextSpan },
}

impl EvidenceOrigin {
    fn reference(&self, episode: &EpisodeId, part: &PartId) -> EvidenceReference {
        match self {
            Self::Procedure { .. } => EvidenceReference {
                id: format!("{episode}:part:{part}"),
                source_kind: SourceKind::SelfVerified,
                linked_episode: Some(episode.clone()),
            },
            Self::Observation { fact_id, .. } => EvidenceReference {
                id: fact_id.clone(),
                source_kind: SourceKind::Observed,
                linked_episode: Some(episode.clone()),
            },
            Self::Utterance { span } => EvidenceReference {
                id: format!("{episode}:utterance:{}-{}", span.start_byte, span.end_byte),
                source_kind: SourceKind::Observed,
                linked_episode: Some(episode.clone()),
            },
        }
    }

    fn provenance(&self) -> Vec<String> {
        match self {
            Self::Procedure { id, version } => vec![format!("procedure:{id}@{version}")],
            Self::Observation { receipt, .. } => receipt.iter().cloned().collect(),
            Self::Utterance { .. } => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartOutcome {
    pub part: PartId,
    pub state: PartState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Engine-authored wording for this part. The model never supplies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<EvidenceOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<Uncertainty>,
    /// Why the part did not run. Retained for auditability even though an
    /// unsupported claim never renders as a fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The turn that rendered this outcome. An outcome renders exactly once,
    /// which is what keeps a clarification from re-answering a finished part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_in_turn: Option<String>,
}

impl PartOutcome {
    pub fn executed(
        part: PartId,
        value: Value,
        claim_text: impl Into<String>,
        origin: EvidenceOrigin,
    ) -> Self {
        Self {
            part,
            state: PartState::Executed,
            value: Some(value),
            claim_text: Some(claim_text.into()),
            origin: Some(origin),
            uncertainty: None,
            reason: None,
            rendered_in_turn: None,
        }
    }

    /// A dialogue act with no computation behind it, such as a greeting.
    pub fn spoken(part: PartId, claim_text: impl Into<String>, span: TextSpan) -> Self {
        Self {
            part,
            state: PartState::Executed,
            value: None,
            claim_text: Some(claim_text.into()),
            origin: Some(EvidenceOrigin::Utterance { span }),
            uncertainty: None,
            reason: None,
            rendered_in_turn: None,
        }
    }

    pub fn clarified(part: PartId, question: impl Into<String>, span: TextSpan) -> Self {
        Self {
            part,
            state: PartState::Clarified,
            value: None,
            claim_text: Some(question.into()),
            origin: Some(EvidenceOrigin::Utterance { span }),
            uncertainty: None,
            reason: None,
            rendered_in_turn: None,
        }
    }

    pub fn abstained(part: PartId, reason: impl Into<String>) -> Self {
        Self {
            part,
            state: PartState::Abstained,
            value: None,
            claim_text: None,
            origin: None,
            uncertainty: None,
            reason: Some(reason.into()),
            rendered_in_turn: None,
        }
    }

    fn blocked(part: PartId, on: &PartId) -> Self {
        Self {
            part,
            state: PartState::Blocked,
            value: None,
            claim_text: None,
            origin: None,
            uncertainty: None,
            reason: Some(format!("depends on {on}, which produced no value")),
            rendered_in_turn: None,
        }
    }

    fn produces_value(&self) -> bool {
        self.state == PartState::Executed
    }
}

/// One utterance's dispatch, from grounding to a rendered reply.
///
/// The analysis and the order are frozen at construction. Resuming after a
/// suspend must never re-derive them: a model asked to segment the same
/// utterance twice can segment it differently, which would orphan the outcomes
/// already collected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartsRun {
    pub analysis: UtteranceAnalysis,
    pub order: Vec<PartId>,
    pub outcomes: BTreeMap<PartId, PartOutcome>,
    pub episode: EpisodeId,
}

impl PartsRun {
    pub fn new(analysis: UtteranceAnalysis, episode: EpisodeId) -> Result<Self, LanguageError> {
        let order = analysis.dispatch_order()?;
        Ok(Self {
            analysis,
            order,
            outcomes: BTreeMap::new(),
            episode,
        })
    }

    /// The next part whose dependencies have all produced values. `None` means
    /// nothing more can run, either because everything finished or because what
    /// remains is blocked.
    pub fn next_ready(&self) -> Option<&PartId> {
        self.order.iter().find(|id| {
            if self.outcomes.contains_key(*id) {
                return false;
            }
            let Some(part) = self.analysis.part(id) else {
                return false;
            };
            part.depends_on().iter().all(|need| {
                self.outcomes
                    .get(need)
                    .is_some_and(PartOutcome::produces_value)
            })
        })
    }

    /// Records an outcome and propagates the consequences. A part that produced
    /// no value blocks its dependents, transitively, while independent siblings
    /// are untouched.
    pub fn record(&mut self, outcome: PartOutcome) {
        let id = outcome.part.clone();
        let produced = outcome.produces_value();
        self.outcomes.insert(id.clone(), outcome);
        if !produced {
            self.block_dependents_of(&id);
        }
    }

    fn block_dependents_of(&mut self, failed: &PartId) {
        // Fixed point, because blocking one part can block its own dependents.
        loop {
            let newly_blocked: Vec<(PartId, PartId)> = self
                .analysis
                .parts
                .iter()
                .filter(|part| !self.outcomes.contains_key(&part.id))
                .filter_map(|part| {
                    part.depends_on()
                        .iter()
                        .find(|need| {
                            *need == failed
                                || self
                                    .outcomes
                                    .get(need)
                                    .is_some_and(|outcome| !outcome.produces_value())
                        })
                        .map(|need| (part.id.clone(), need.clone()))
                })
                .collect();
            if newly_blocked.is_empty() {
                return;
            }
            for (part, need) in newly_blocked {
                self.outcomes
                    .insert(part.clone(), PartOutcome::blocked(part, &need));
            }
        }
    }

    /// Marks every part that has not run as abstained. Used when the Teacher
    /// budget runs out mid-utterance: work already done is never discarded to
    /// punish a part that could not be taught.
    pub fn abstain_remaining(&mut self, reason: &str) {
        let pending: Vec<PartId> = self
            .order
            .iter()
            .filter(|id| !self.outcomes.contains_key(*id))
            .cloned()
            .collect();
        for id in pending {
            self.outcomes
                .insert(id.clone(), PartOutcome::abstained(id, reason));
        }
    }

    /// Coerces every part the analysis already refused to run. Parts marked
    /// Clarify or Abstain at analysis time never reach dispatch.
    pub fn seed_non_executable(&mut self) {
        let seeds: Vec<PartOutcome> = self
            .analysis
            .parts
            .iter()
            .filter(|part| !part.is_executable())
            .filter(|part| !self.outcomes.contains_key(&part.id))
            .map(|part| {
                let span = part.spans.first().copied().unwrap_or(TextSpan::new(0, 0));
                match part.intent.disposition {
                    IntentDisposition::Clarify => {
                        let ambiguity = part
                            .mentions
                            .iter()
                            .chain(part.context_bindings.iter())
                            .find_map(|mention| match &mention.resolved {
                                MentionResolution::Unresolved { ambiguity } => {
                                    Some(ambiguity.clone())
                                }
                                _ => None,
                            })
                            .or_else(|| {
                                part.intent
                                    .candidates
                                    .iter()
                                    .flat_map(|candidate| candidate.ambiguities.iter())
                                    .next()
                                    .cloned()
                            })
                            .unwrap_or_else(|| "the request is ambiguous".to_string());
                        PartOutcome::clarified(part.id.clone(), ambiguity, span)
                    }
                    _ => PartOutcome::abstained(part.id.clone(), "the analysis abstained"),
                }
            })
            .collect();
        for seed in seeds {
            self.record(seed);
        }
    }

    pub fn is_complete(&self) -> bool {
        self.order.iter().all(|id| self.outcomes.contains_key(id))
    }

    pub fn executed_count(&self) -> usize {
        self.outcomes
            .values()
            .filter(|outcome| outcome.state == PartState::Executed)
            .count()
    }

    pub fn needs_clarification(&self) -> bool {
        self.outcomes
            .values()
            .any(|outcome| outcome.state == PartState::Clarified)
    }

    /// The value a dependent part consumes, so the Engine can bind a
    /// `part_ref` mention without the analysis ever being rewritten.
    pub fn resolved_value(&self, part: &PartId) -> Option<&Value> {
        self.outcomes
            .get(part)
            .and_then(|outcome| outcome.value.as_ref())
    }

    /// Claim-level dependency edges, so the realizer cannot word a consumer
    /// ahead of its producer.
    pub fn claim_dependencies(&self) -> ClaimDependencies {
        self.analysis
            .parts
            .iter()
            .filter_map(|part| {
                let needs: BTreeSet<String> = part
                    .depends_on()
                    .iter()
                    .filter(|need| {
                        self.outcomes
                            .get(need)
                            .is_some_and(|outcome| outcome.claim_text.is_some())
                    })
                    .map(|need| claim_id(need))
                    .collect();
                if needs.is_empty() {
                    None
                } else {
                    Some((claim_id(&part.id), needs))
                }
            })
            .collect()
    }

    /// Builds the reply for this turn and marks the outcomes it consumed.
    ///
    /// Only outcomes that have not already rendered are included, which is what
    /// makes a clarification reply answer the newly unblocked part without
    /// repeating the parts that finished in the first turn.
    pub fn response_plan(&mut self, turn: &str, tone: ResponseTone) -> ResponsePlan {
        let mut claims = Vec::new();
        let mut acts = Vec::new();
        let mut uncertainties = Vec::new();
        let mut rendered = Vec::new();

        // Source order, not dispatch order. Execution dependencies decide what
        // runs first; they do not decide what the user reads first.
        for id in self.analysis.source_order() {
            let Some(outcome) = self.outcomes.get(&id) else {
                continue;
            };
            if outcome.rendered_in_turn.is_some() {
                continue;
            }
            let Some(part) = self.analysis.part(&id) else {
                continue;
            };

            match (&outcome.claim_text, &outcome.origin) {
                (Some(text), Some(origin)) => {
                    let act = match outcome.state {
                        PartState::Clarified => DialogueAct::Clarify,
                        _ => part.act,
                    };
                    claims.push(PlannedClaim::Grounded(GroundedClaim {
                        id: claim_id(&id),
                        text: text.clone(),
                        evidence: vec![origin.reference(&self.episode, &id)],
                        provenance: origin.provenance(),
                        act: Some(act),
                    }));
                    acts.push(act);
                }
                _ => {
                    claims.push(PlannedClaim::Unsupported {
                        id: claim_id(&id),
                        reason: outcome
                            .reason
                            .clone()
                            .unwrap_or_else(|| "the part produced no grounded claim".to_string()),
                    });
                }
            }
            if let Some(uncertainty) = &outcome.uncertainty {
                uncertainties.push(uncertainty.clone());
            }
            rendered.push(id);
        }

        for id in rendered {
            if let Some(outcome) = self.outcomes.get_mut(&id) {
                outcome.rendered_in_turn = Some(turn.to_string());
            }
        }

        let grounded = claims
            .iter()
            .filter(|claim| matches!(claim, PlannedClaim::Grounded(_)))
            .count();
        ResponsePlan {
            dialogue_move: DialogueMove::new(DialogueAct::plan_act(&acts, grounded)),
            claims,
            uncertainty: Uncertainty::merge(uncertainties),
            tone,
            // Joined is the multi-part default. Plain would put each answer on
            // its own line, which reads as a list rather than a reply.
            variant: RenderVariant::Joined,
        }
    }
}

/// Claims are addressed by part so an outcome and the sentence it produced stay
/// traceable to each other in a stored episode.
pub fn claim_id(part: &PartId) -> String {
    format!("claim_{part}")
}
