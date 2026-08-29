//! Admitting proposed facts and language relationships.

use std::collections::{BTreeMap, BTreeSet};

use spoon_core::language::{
    DialogueAct, IntentDisposition, IntentFrameProposal, IntentScope, InterpretationProposal,
    TokenRange, tokenize,
};
use spoon_core::utterance::{
    LanguageWriteKind, LanguageWriteProposal, PartProposal, ResidualPolarity, ResidualProposal,
    ResidualProvenanceProposal, UtteranceAnalysis, UtteranceAnalysisProposal, UtteranceLimits,
};
use spoon_core::{ConceptId, EpisodeId, Lifecycle, Value, VerifiabilityTier};
use spoon_engine::admission::{admit_language_writes, admit_residuals, is_admissible_kind};

const TEXT: &str = "my dog is Pierre";

fn analysis(
    residuals: Vec<ResidualProposal>,
    writes: Vec<LanguageWriteProposal>,
    aliases: &BTreeSet<String>,
) -> UtteranceAnalysis {
    let stream = tokenize(TEXT).expect("tokenizes");
    let proposal = UtteranceAnalysisProposal {
        cleaned: TEXT.to_string(),
        alignment: Vec::new(),
        parts: vec![PartProposal {
            id: "p0".to_string(),
            source_tokens: vec![TokenRange::new(0, stream.tokens.len())],
            template: "my dog is {e0}".to_string(),
            act: DialogueAct::Inform,
            mentions: Vec::new(),
            context_bindings: Vec::new(),
            intent: InterpretationProposal {
                candidates: vec![IntentFrameProposal {
                    name: "state".to_string(),
                    confidence: 1.0,
                    scope: IntentScope::CurrentTurn,
                    source_tokens: Vec::new(),
                    slots: Vec::new(),
                    ambiguities: Vec::new(),
                }],
                selected: Some(0),
                disposition: IntentDisposition::Execute,
            },
            residual: residuals,
        }],
        language_writes: writes,
    };
    proposal
        .ground_for(&stream, aliases, &UtteranceLimits::default())
        .expect("fixture grounds")
}

fn residual(
    id: &str,
    predicate: &str,
    value: Value,
    provenance: ResidualProvenanceProposal,
) -> ResidualProposal {
    ResidualProposal {
        id: id.to_string(),
        predicate: predicate.to_string(),
        value,
        scope: BTreeMap::new(),
        polarity: ResidualPolarity::Assert,
        provenance,
    }
}

// ---------------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------------

#[test]
fn a_fact_the_user_said_is_admitted_as_deferred_testimony() {
    let analysis = analysis(
        vec![residual(
            "r0",
            "dog.name",
            Value::Text("Pierre".into()),
            // "Pierre" is the last token of the utterance.
            ResidualProvenanceProposal::UtteranceTokens(TokenRange::new(6, 7)),
        )],
        Vec::new(),
        &BTreeSet::new(),
    );
    let episode = EpisodeId::new();

    let admitted = admit_residuals(&analysis, &episode, &BTreeSet::new(), &BTreeMap::new());

    assert_eq!(admitted.facts.len(), 1);
    assert!(
        admitted.diagnostics.is_empty(),
        "{:?}",
        admitted.diagnostics
    );
    let fact = &admitted.facts[0];
    assert_eq!(fact.predicate, "dog.name");
    assert_eq!(fact.value, Value::Text("Pierre".into()));
    // Testimony, not a deterministic check, and never self-verified.
    assert_eq!(fact.tier, Some(VerifiabilityTier::Deferred));
    assert_eq!(fact.verifier, None);
    assert_eq!(fact.source_episode.as_ref(), Some(&episode));
    assert_eq!(fact.id, format!("{episode}:0"));
    // The exact wording that licensed the fact is retained.
    assert_eq!(
        fact.scope.get("provenance"),
        Some(&Value::Text("utterance:Pierre".into()))
    );
}

#[test]
fn a_fact_citing_a_prior_observation_is_admitted() {
    let aliases = BTreeSet::from(["f0".to_string()]);
    let analysis = analysis(
        vec![residual(
            "r0",
            "dog.age",
            Value::Int(3),
            ResidualProvenanceProposal::ContextAlias("f0".to_string()),
        )],
        Vec::new(),
        &aliases,
    );

    let admitted = admit_residuals(&analysis, &EpisodeId::new(), &aliases, &BTreeMap::new());

    assert_eq!(admitted.facts.len(), 1);
    assert_eq!(
        admitted.facts[0].scope.get("provenance"),
        Some(&Value::Text("context:f0".into()))
    );
}

#[test]
fn a_fact_citing_a_capability_rather_than_an_observation_is_refused() {
    // c0 is in the packet, so grounding accepts it, but it names a catalog
    // entry. The ability to compute something is not evidence that something
    // is true.
    let aliases = BTreeSet::from(["c0".to_string()]);
    let analysis = analysis(
        vec![residual(
            "r0",
            "dog.age",
            Value::Int(3),
            ResidualProvenanceProposal::ContextAlias("c0".to_string()),
        )],
        Vec::new(),
        &aliases,
    );

    // The observation-backed alias set is empty: c0 is not one.
    let admitted = admit_residuals(
        &analysis,
        &EpisodeId::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
    );

    assert!(admitted.facts.is_empty());
    assert_eq!(admitted.diagnostics.len(), 1);
    assert!(
        admitted.diagnostics[0]
            .reason
            .contains("is not an observation"),
        "{:?}",
        admitted.diagnostics[0]
    );
}

#[test]
fn a_contradicting_fact_is_admitted_alongside_rather_than_overwriting() {
    let analysis = analysis(
        vec![residual(
            "r0",
            "dog.name",
            Value::Text("Pierre".into()),
            ResidualProvenanceProposal::UtteranceTokens(TokenRange::new(6, 7)),
        )],
        Vec::new(),
        &BTreeSet::new(),
    );
    let existing = BTreeMap::from([("dog.name".to_string(), Value::Text("Rex".into()))]);

    let admitted = admit_residuals(&analysis, &EpisodeId::new(), &BTreeSet::new(), &existing);

    // The new fact still lands.
    assert_eq!(admitted.facts.len(), 1);
    // And the disagreement is surfaced rather than silently resolved.
    assert_eq!(admitted.contradictions.len(), 1);
    assert_eq!(
        admitted.contradictions[0].existing,
        Value::Text("Rex".into())
    );
    assert_eq!(
        admitted.contradictions[0].proposed,
        Value::Text("Pierre".into())
    );
}

#[test]
fn a_denial_records_its_polarity() {
    let stream = tokenize(TEXT).expect("tokenizes");
    let mut denial = residual(
        "r0",
        "dog.name",
        Value::Text("Rex".into()),
        ResidualProvenanceProposal::UtteranceTokens(TokenRange::new(0, stream.tokens.len())),
    );
    denial.polarity = ResidualPolarity::Deny;
    let analysis = analysis(vec![denial], Vec::new(), &BTreeSet::new());

    let admitted = admit_residuals(
        &analysis,
        &EpisodeId::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
    );

    assert_eq!(
        admitted.facts[0].scope.get("polarity"),
        Some(&Value::Text("deny".into()))
    );
}

#[test]
fn several_facts_get_distinct_ordinals() {
    let analysis = analysis(
        vec![
            residual(
                "r0",
                "dog.name",
                Value::Text("Pierre".into()),
                ResidualProvenanceProposal::UtteranceTokens(TokenRange::new(6, 7)),
            ),
            residual(
                "r1",
                "dog.exists",
                Value::Bool(true),
                ResidualProvenanceProposal::UtteranceTokens(TokenRange::new(2, 3)),
            ),
        ],
        Vec::new(),
        &BTreeSet::new(),
    );
    let episode = EpisodeId::new();

    let admitted = admit_residuals(&analysis, &episode, &BTreeSet::new(), &BTreeMap::new());

    assert_eq!(admitted.facts[0].id, format!("{episode}:0"));
    assert_eq!(admitted.facts[1].id, format!("{episode}:1"));
}

// ---------------------------------------------------------------------------
// Language writes
// ---------------------------------------------------------------------------

fn write(kind: LanguageWriteKind, surface: &str, alias: &str) -> LanguageWriteProposal {
    LanguageWriteProposal {
        kind,
        surface: surface.to_string(),
        target_alias: alias.to_string(),
        source_tokens: Vec::new(),
    }
}

#[test]
fn an_alias_write_becomes_a_provisional_relationship_with_this_episode_as_evidence() {
    let aliases = BTreeSet::from(["c0".to_string()]);
    let analysis = analysis(
        Vec::new(),
        vec![write(LanguageWriteKind::AliasOf, "pup", "c0")],
        &aliases,
    );
    let episode = EpisodeId::new();
    let concept = ConceptId::new();

    let admitted = admit_language_writes(&analysis, &episode, true, |_| Some(concept));

    assert_eq!(admitted.relationships.len(), 1);
    let relationship = &admitted.relationships[0];
    assert_eq!(relationship.kind, "alias-of");
    assert_eq!(relationship.lifecycle, Lifecycle::Provisional);
    assert_eq!(relationship.evidence, vec![episode]);
    assert!(
        relationship.scope[0].description.contains("pup"),
        "{:?}",
        relationship.scope[0]
    );
}

#[test]
fn an_intent_key_is_refused_until_something_actually_executed() {
    let aliases = BTreeSet::from(["c0".to_string()]);
    let analysis = analysis(
        Vec::new(),
        vec![write(LanguageWriteKind::IntentOf, "name.the.dog", "c0")],
        &aliases,
    );
    let concept = ConceptId::new();

    let refused = admit_language_writes(&analysis, &EpisodeId::new(), false, |_| Some(concept));
    assert!(refused.relationships.is_empty());
    assert!(
        refused.diagnostics[0]
            .reason
            .contains("after the part it names executed"),
        "{:?}",
        refused.diagnostics[0]
    );

    // The same proposal lands once a part really ran.
    let admitted = admit_language_writes(&analysis, &EpisodeId::new(), true, |_| Some(concept));
    assert_eq!(admitted.relationships.len(), 1);
    assert_eq!(admitted.relationships[0].kind, "intent-of");
}

#[test]
fn an_alias_write_is_refused_when_its_target_does_not_resolve() {
    let aliases = BTreeSet::from(["c0".to_string()]);
    let analysis = analysis(
        Vec::new(),
        vec![write(LanguageWriteKind::Termed, "puppy", "c0")],
        &aliases,
    );

    let admitted = admit_language_writes(&analysis, &EpisodeId::new(), true, |_| None);

    assert!(admitted.relationships.is_empty());
    assert!(
        admitted.diagnostics[0].reason.contains("does not resolve"),
        "{:?}",
        admitted.diagnostics[0]
    );
}

#[test]
fn a_duplicate_proposal_in_one_analysis_lands_once() {
    let aliases = BTreeSet::from(["c0".to_string()]);
    let analysis = analysis(
        Vec::new(),
        vec![
            write(LanguageWriteKind::AliasOf, "pup", "c0"),
            write(LanguageWriteKind::AliasOf, "pup", "c0"),
        ],
        &aliases,
    );
    let concept = ConceptId::new();

    let admitted = admit_language_writes(&analysis, &EpisodeId::new(), true, |_| Some(concept));

    assert_eq!(admitted.relationships.len(), 1);
    assert_eq!(admitted.diagnostics.len(), 1);
    assert!(admitted.diagnostics[0].reason.contains("duplicate"));
}

#[test]
fn the_admissible_relationship_kinds_are_a_closed_set() {
    for kind in ["alias-of", "termed", "intent-of"] {
        assert!(is_admissible_kind(kind), "{kind} should be admissible");
    }
    for kind in ["is-a", "implemented-by", "owns", "alias_of", ""] {
        assert!(!is_admissible_kind(kind), "{kind} should be refused");
    }
}

#[test]
fn every_language_write_kind_maps_to_an_admissible_relationship_kind() {
    for kind in [
        LanguageWriteKind::AliasOf,
        LanguageWriteKind::Termed,
        LanguageWriteKind::IntentOf,
    ] {
        assert!(is_admissible_kind(kind.as_relationship_kind()));
    }
}
