use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use spoon_core::{ConceptId, Procedure};
use spoon_engine::Engine;

use crate::ForgeError;
use crate::curriculum::{LearnedStructure, StructureType};
use crate::export::procedure_references;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructureStatus {
    /// A single acquired structure carries every declared semantic property.
    Matched,
    /// The engine holds structures of this type, but none carry all the
    /// declared properties.
    Missing,
    /// The engine has no store that holds this structure type at all, so the
    /// expectation cannot be checked here. This is not a pass.
    Unrepresented,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructureFinding {
    pub structure_type: StructureType,
    pub status: StructureStatus,
    /// The acquired structure that carried every property, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    /// Declared properties no acquired structure carried. Empty on a match.
    pub unmatched_properties: Vec<String>,
    pub candidates_considered: usize,
}

/// Compare what the engine actually acquired against the curriculum's declared
/// expectations, one expectation at a time.
///
/// Matching is deliberately literal: a candidate carries a semantic property
/// when every significant word of that property appears in the candidate's own
/// searchable text. A looser rule would let a vague expectation pass against
/// unrelated knowledge, which is the exact failure this inspection exists to
/// catch.
pub fn inspect_structures(
    expectations: &[LearnedStructure],
    engine: &Engine,
) -> Result<Vec<StructureFinding>, ForgeError> {
    let candidates = collect_candidates(engine)?;
    Ok(expectations
        .iter()
        .map(
            |expectation| match candidates.get(&expectation.structure_type) {
                None => StructureFinding {
                    structure_type: expectation.structure_type,
                    status: StructureStatus::Unrepresented,
                    matched: None,
                    unmatched_properties: expectation.semantic_properties.clone(),
                    candidates_considered: 0,
                },
                Some(pool) => finding(expectation, pool),
            },
        )
        .collect())
}

/// One acquired structure, reduced to a label and the text a property can
/// match against.
struct Candidate {
    label: String,
    text: String,
}

fn finding(expectation: &LearnedStructure, pool: &[Candidate]) -> StructureFinding {
    let mut best: Option<(&Candidate, Vec<String>)> = None;
    for candidate in pool {
        let unmatched: Vec<String> = expectation
            .semantic_properties
            .iter()
            .filter(|property| !carries(&candidate.text, property))
            .cloned()
            .collect();
        if unmatched.is_empty() {
            return StructureFinding {
                structure_type: expectation.structure_type,
                status: StructureStatus::Matched,
                matched: Some(candidate.label.clone()),
                unmatched_properties: Vec::new(),
                candidates_considered: pool.len(),
            };
        }
        if best
            .as_ref()
            .is_none_or(|(_, current)| unmatched.len() < current.len())
        {
            best = Some((candidate, unmatched));
        }
    }
    StructureFinding {
        structure_type: expectation.structure_type,
        status: StructureStatus::Missing,
        matched: None,
        unmatched_properties: best
            .map(|(_, unmatched)| unmatched)
            .unwrap_or_else(|| expectation.semantic_properties.clone()),
        candidates_considered: pool.len(),
    }
}

/// Words shorter than three characters and pure connectives carry no meaning
/// for matching and would make almost any property match almost any text.
const STOP_WORDS: [&str; 10] = [
    "and", "are", "the", "for", "with", "not", "its", "that", "this", "from",
];

fn carries(text: &str, property: &str) -> bool {
    let mut significant = property
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|word| word.len() >= 3 && !STOP_WORDS.contains(&word.as_str()))
        .peekable();
    if significant.peek().is_none() {
        return false;
    }
    significant.all(|word| text.contains(&word))
}

fn collect_candidates(
    engine: &Engine,
) -> Result<BTreeMap<StructureType, Vec<Candidate>>, ForgeError> {
    let graph = engine.graph();
    let concepts = graph.list_concepts()?;
    let procedures = graph.list_procedures()?;
    let names: HashMap<ConceptId, String> = concepts
        .iter()
        .map(|concept| (concept.id, concept.name.clone()))
        .collect();

    let mut pools: BTreeMap<StructureType, Vec<Candidate>> = BTreeMap::new();
    pools.insert(
        StructureType::Concept,
        concepts
            .iter()
            .map(|concept| Candidate {
                label: concept.name.clone(),
                text: lower(&[&concept.name, concept.description.as_deref().unwrap_or("")]),
            })
            .collect(),
    );
    pools.insert(
        StructureType::Relationship,
        graph
            .list_relationships(u32::MAX)?
            .iter()
            .map(|relationship| {
                let source = names.get(&relationship.source).map_or("", String::as_str);
                let target = names.get(&relationship.target).map_or("", String::as_str);
                Candidate {
                    label: format!("{source} {} {target}", relationship.kind),
                    text: lower(&[source, &relationship.kind, target]),
                }
            })
            .collect(),
    );
    pools.insert(
        StructureType::Procedure,
        procedures.iter().map(procedure_candidate).collect(),
    );
    pools.insert(
        StructureType::Contract,
        procedures
            .iter()
            .filter(|procedure| {
                !procedure.contract.requires.is_empty()
                    || !procedure.contract.promises.is_empty()
                    || !procedure.contract.fails_when.is_empty()
            })
            .map(|procedure| Candidate {
                label: format!("{} contract", procedure.name),
                text: lower(&[&procedure.name, &contract_text(procedure)]),
            })
            .collect(),
    );
    pools.insert(
        StructureType::DependencyGraph,
        procedures
            .iter()
            .filter(|procedure| !procedure_references(&procedure.body).is_empty())
            .map(|procedure| {
                let callees: Vec<String> = procedure_references(&procedure.body)
                    .iter()
                    .filter_map(|id| graph.get_procedure(*id).ok().flatten())
                    .map(|callee| callee.name)
                    .collect();
                Candidate {
                    label: format!("{} dependencies", procedure.name),
                    text: lower(&[&procedure.name, &callees.join(" ")]),
                }
            })
            .collect(),
    );
    pools.insert(
        StructureType::TestSet,
        procedures
            .iter()
            .filter_map(|procedure| {
                let cases = engine
                    .episodes()
                    .list_verified_regression_cases(procedure.id, procedure.version)
                    .ok()?;
                (!cases.is_empty()).then(|| Candidate {
                    label: format!("{} regression cases", procedure.name),
                    text: lower(&[
                        &procedure.name,
                        &contract_text(procedure),
                        "verified regression case replayable",
                    ]),
                })
            })
            .collect(),
    );
    pools.insert(
        StructureType::IntentFrame,
        engine
            .list_intent_catalog_entries(usize::MAX)?
            .iter()
            .map(|entry| {
                let slots: Vec<&str> = entry.slots.iter().map(|slot| slot.name.as_str()).collect();
                Candidate {
                    label: entry.key.clone(),
                    text: lower(&[&entry.key, &slots.join(" ")]),
                }
            })
            .collect(),
    );
    // Response plans, repository models, workflows, and semantic lowerings have
    // no durable engine store today. Leaving them out of the map is what makes
    // the inspection report them as unrepresented instead of quietly missing.
    pools.retain(|_, pool| !pool.is_empty());
    Ok(pools)
}

fn procedure_candidate(procedure: &Procedure) -> Candidate {
    let params: Vec<&str> = procedure
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    Candidate {
        label: procedure.name.clone(),
        text: lower(&[
            &procedure.name,
            &params.join(" "),
            &contract_text(procedure),
        ]),
    }
}

fn contract_text(procedure: &Procedure) -> String {
    procedure
        .contract
        .requires
        .iter()
        .chain(&procedure.contract.promises)
        .chain(&procedure.contract.fails_when)
        .map(|condition| condition.description.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn lower(parts: &[&str]) -> String {
    parts.join(" ").to_ascii_lowercase()
}
