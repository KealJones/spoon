//! Admitting what an analysis proposed: facts and language relationships.
//!
//! The front model may assert facts. It may not assert them without a source,
//! and the only sources that exist in this design are the utterance itself and
//! the context packet. There is no retrieval here, so a fact with neither is
//! model-weight recall dressed as knowledge, and a small model citing its own
//! weights is a fabricated citation.
//!
//! Everything admitted here starts Provisional and Deferred. Promotion runs
//! through the existing evidence path, unchanged by this module.
//!
//! Admission is deliberately total: nothing here can fail the cycle. A residual
//! that cannot be grounded is dropped with a diagnostic and the part still
//! executes, because one bad proposed fact is not a reason to withhold the
//! user's answer.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use spoon_core::utterance::{
    LanguageWrite, LanguageWriteKind, ResidualPolarity, ResidualProvenance, UtteranceAnalysis,
};
use spoon_core::{
    ConceptId, EpisodeId, Lifecycle, ObservedFact, Relationship, Value, VerifiabilityTier,
};

/// A refusal, kept rather than discarded so an episode records what was
/// proposed and why it did not land.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionDiagnostic {
    pub subject: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactAdmission {
    pub facts: Vec<ObservedFact>,
    /// Facts whose `(predicate, scope)` already held a different value. They
    /// are admitted alongside rather than overwriting, and flagged for the
    /// existing reconciliation path.
    pub contradictions: Vec<FactContradiction>,
    pub diagnostics: Vec<AdmissionDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactContradiction {
    pub predicate: String,
    pub existing: Value,
    pub proposed: Value,
}

/// Admits the residual claims of one analysis.
///
/// `fact_aliases` is the set of packet aliases that themselves trace to a prior
/// observation. A context citation naming anything else, such as a catalog
/// entry, is citing a capability rather than evidence, and is refused.
///
/// `existing` maps a predicate to the value already on record, used only to
/// detect contradiction. Nothing here overwrites.
pub fn admit_residuals(
    analysis: &UtteranceAnalysis,
    episode: &EpisodeId,
    fact_aliases: &BTreeSet<String>,
    existing: &BTreeMap<String, Value>,
) -> FactAdmission {
    let mut admission = FactAdmission::default();
    let mut ordinal = 0usize;

    for part in &analysis.parts {
        for residual in &part.residual {
            let subject = format!("{}:{}", part.id, residual.id);

            let provenance = match &residual.provenance {
                ResidualProvenance::Utterance { span } => {
                    // The span was already proven to cover complete tokens at
                    // grounding, so this only re-derives the surface text for
                    // the audit record.
                    match analysis.original.slice(span) {
                        Some(surface) => format!("utterance:{surface}"),
                        None => {
                            admission.diagnostics.push(AdmissionDiagnostic {
                                subject,
                                reason: "the cited span is not readable in the original stream"
                                    .to_string(),
                            });
                            continue;
                        }
                    }
                }
                ResidualProvenance::Context { alias } => {
                    if !fact_aliases.contains(alias) {
                        admission.diagnostics.push(AdmissionDiagnostic {
                            subject,
                            reason: format!(
                                "alias {alias:?} is not an observation, so it cannot be the source of a fact"
                            ),
                        });
                        continue;
                    }
                    format!("context:{alias}")
                }
            };

            let mut scope = residual.scope.clone();
            scope.insert("provenance".to_string(), Value::Text(provenance));
            if residual.polarity == ResidualPolarity::Deny {
                scope.insert("polarity".to_string(), Value::Text("deny".into()));
            }

            if let Some(previous) = existing.get(&residual.predicate)
                && previous != &residual.value
            {
                admission.contradictions.push(FactContradiction {
                    predicate: residual.predicate.clone(),
                    existing: previous.clone(),
                    proposed: residual.value.clone(),
                });
            }

            let mut fact =
                ObservedFact::new(residual.predicate.clone(), residual.value.clone(), scope);
            fact.id = format!("{episode}:{ordinal}");
            fact.source_episode = Some(*episode);
            // Deferred, never Hard: the user asserting something is testimony,
            // not a deterministic check.
            fact.tier = Some(VerifiabilityTier::Deferred);
            fact.verifier = None;
            admission.facts.push(fact);
            ordinal += 1;
        }
    }

    admission
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageWriteAdmission {
    pub relationships: Vec<Relationship>,
    pub diagnostics: Vec<AdmissionDiagnostic>,
}

/// Admits proposed language relationships.
///
/// `resolve` maps a request-local packet alias to the concept the Engine
/// already minted for it. The model proposes surface forms and aliases; it
/// never supplies an identifier, so an alias that does not resolve is refused
/// rather than used to create something new.
///
/// `intent_of_executed` names the parts that actually executed. An `intent-of`
/// edge claims a semantic key resolves to a procedure, and that claim is only
/// earned once the key really ran.
pub fn admit_language_writes<F>(
    analysis: &UtteranceAnalysis,
    episode: &EpisodeId,
    executed_any: bool,
    mut resolve: F,
) -> LanguageWriteAdmission
where
    F: FnMut(&str) -> Option<ConceptId>,
{
    let mut admission = LanguageWriteAdmission::default();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();

    for write in &analysis.language_writes {
        let subject = format!("{:?}:{}", write.kind, write.surface);

        if write.kind == LanguageWriteKind::IntentOf && !executed_any {
            admission.diagnostics.push(AdmissionDiagnostic {
                subject,
                reason: "an intent key is only admitted after the part it names executed"
                    .to_string(),
            });
            continue;
        }

        let Some(target) = resolve(&write.target_alias) else {
            admission.diagnostics.push(AdmissionDiagnostic {
                subject,
                reason: format!(
                    "alias {:?} does not resolve to a known concept",
                    write.target_alias
                ),
            });
            continue;
        };

        if !seen.insert((write.surface.clone(), write.target_alias.clone())) {
            admission.diagnostics.push(AdmissionDiagnostic {
                subject,
                reason: "duplicate proposal in the same analysis".to_string(),
            });
            continue;
        }

        admission
            .relationships
            .push(relationship_for(write, target, episode));
    }

    admission
}

fn relationship_for(write: &LanguageWrite, target: ConceptId, episode: &EpisodeId) -> Relationship {
    // Source and target are the same concept because the surface form is not
    // itself a concept. The edge records that this concept is also known by
    // this wording, carried in the scope condition.
    let mut relationship = Relationship::new(target, target, write.kind.as_relationship_kind());
    relationship.lifecycle = Lifecycle::Provisional;
    relationship.evidence = vec![*episode];
    relationship.scope = vec![spoon_core::ScopeCondition {
        description: format!("surface form {:?}", write.surface),
        learned_from: Some(*episode),
    }];
    relationship
}

/// The closed set of language relationship kinds. `Relationship.kind` is a free
/// string in core, so the restriction has to be enforced at admission.
pub fn is_admissible_kind(kind: &str) -> bool {
    matches!(kind, "alias-of" | "termed" | "intent-of")
}
