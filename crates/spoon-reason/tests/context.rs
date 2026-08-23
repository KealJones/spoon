use std::collections::BTreeMap;

use spoon_core::{
    Assumption, Concept, ConceptId, Episode, Evaluation, Expr, Interpretation, Lifecycle,
    MutabilityClass, Param, Procedure, Relationship, Value, VerifiabilityTier,
};
use spoon_episode::EpisodeStore;
use spoon_graph::KnowledgeStore;
use spoon_reason::{
    ContextAssembler, ContextConfig, ContextError, ContextLimits, ContextRequest,
    InterpretationCandidate, InterpretationSet, RemainingBudget,
};

fn concept(name: &str) -> Concept {
    Concept::new(name, MutabilityClass::Definitional)
}

fn interpretation(meaning: ConceptId) -> InterpretationSet {
    InterpretationSet::try_new(
        vec![InterpretationCandidate {
            meaning,
            weight: 1.0,
        }],
        Some(meaning),
    )
    .unwrap()
}

fn request(meaning: ConceptId) -> ContextRequest {
    ContextRequest {
        goal: Some("answer accurately because the caller requested it".into()),
        goal_reason: Some("the caller needs a reliable result".into()),
        interpretation: interpretation(meaning),
        entities: Vec::new(),
        assumptions: Vec::new(),
        environment: BTreeMap::new(),
        budget_remaining: RemainingBudget {
            steps: 50,
            teacher_calls: 2,
            cost: 1.25,
        },
    }
}

#[test]
fn assembles_the_full_bounded_working_context() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let active = concept("DOUBLE");
    let number = concept("NUMBER");
    let unrelated = concept("NOISE");
    for item in [&active, &number, &unrelated] {
        graph.insert_concept(item).unwrap();
    }
    graph
        .insert_relationship(&Relationship::new(active.id, number.id, "operates-on"))
        .unwrap();
    graph
        .insert_relationship(&Relationship::new(active.id, unrelated.id, "mentions"))
        .unwrap();
    let procedure = Procedure::new(
        "double",
        vec![Param::named("x")],
        Expr::Literal(Value::Int(8)),
    )
    .with_concept(active.id);
    graph.insert_procedure(&procedure).unwrap();

    let mut old = Episode::new("old");
    old.created_at = 10;
    old.action = Some("ignore".into());
    episodes.insert(&old).unwrap();
    let mut recent = Episode::new("recent");
    recent.created_at = 20;
    recent.action = Some("run DOUBLE".into());
    recent.observed_result = Some(Value::Int(8));
    recent.evaluation = Some(Evaluation {
        tier: VerifiabilityTier::Hard,
        success: true,
        details: "matched".into(),
        surprise: None,
    });
    episodes.insert(&recent).unwrap();

    let mut input = request(active.id);
    input.entities.push(number.id);
    input.assumptions.push(Assumption {
        description: "the pronoun refers to four".into(),
        basis: "assumed".into(),
        concept: Some(number.id),
    });
    input.environment.insert("locale".into(), "en-US".into());

    let config = ContextConfig {
        relationship_kinds: vec!["operates-on".into()],
        limits: ContextLimits {
            max_recent_episodes: 1,
            ..ContextLimits::default()
        },
    };
    let assembler = ContextAssembler::new(&graph, &episodes, config).unwrap();
    let context = assembler.assemble(&input).unwrap();

    assert_eq!(context.goal.as_deref(), input.goal.as_deref());
    assert_eq!(context.goal_reason.as_deref(), input.goal_reason.as_deref());
    assert_eq!(context.interpretations, input.interpretation.candidates());
    assert_eq!(context.entities, vec![active.id, number.id]);
    assert_eq!(context.relevant_knowledge.len(), 1);
    assert_eq!(
        context.relevant_knowledge[0].relationship.kind,
        "operates-on"
    );
    assert_eq!(context.relevant_knowledge[0].adjacent_concept.id, number.id);
    assert_eq!(context.relevant_knowledge[0].hops, 1);
    assert_eq!(context.recent_episodes.len(), 1);
    assert_eq!(context.recent_episodes[0].episode_id, recent.id);
    assert_eq!(
        context.recent_episodes[0].action.as_deref(),
        Some("run DOUBLE")
    );
    assert_eq!(
        context.recent_episodes[0].observed_result,
        Some(Value::Int(8))
    );
    assert_eq!(context.recent_episodes[0].succeeded, Some(true));
    assert_eq!(context.relevant_procedures.len(), 1);
    assert_eq!(context.relevant_procedures[0].id, procedure.id);
    assert_eq!(context.assumptions.len(), 1);
    assert_eq!(
        context.assumptions[0].description,
        input.assumptions[0].description
    );
    assert_eq!(context.assumptions[0].basis, input.assumptions[0].basis);
    assert_eq!(context.environment["locale"], Value::from("en-US"));
    assert_eq!(context.budget_remaining, input.budget_remaining);

    let episode_context = context.to_episode_context();
    assert_eq!(episode_context.goal, context.goal);
    assert_eq!(episode_context.goal_reason, context.goal_reason);
    assert_eq!(episode_context.interpretations.len(), 1);
    assert!(episode_context.interpretations[0].chosen);
    assert_eq!(episode_context.entities, context.entities);
    assert_eq!(
        episode_context.relevant_knowledge.len(),
        context.relevant_knowledge.len()
    );
    assert_eq!(
        episode_context.relevant_procedures.len(),
        context.relevant_procedures.len()
    );
    assert_eq!(
        episode_context.recent_episodes.len(),
        context.recent_episodes.len()
    );
    assert_eq!(episode_context.assumptions.len(), context.assumptions.len());
    assert_eq!(
        episode_context.assumptions[0].description,
        context.assumptions[0].description
    );
    assert_eq!(episode_context.environment, context.environment);
    assert_eq!(
        episode_context.budget_remaining,
        Some(context.budget_remaining)
    );

    let mut persisted = Episode::new("persist full context");
    persisted.context = episode_context;
    episodes.insert(&persisted).unwrap();
    let restored = episodes.get(persisted.id).unwrap();
    assert_eq!(restored.context.goal_reason, context.goal_reason);
    assert_eq!(restored.context.interpretations.len(), 1);
    assert_eq!(restored.context.relevant_knowledge.len(), 1);
    assert_eq!(restored.context.relevant_procedures.len(), 1);
    assert_eq!(restored.context.recent_episodes.len(), 1);
    assert_eq!(restored.context.environment, context.environment);
    assert_eq!(
        restored.context.budget_remaining,
        Some(context.budget_remaining)
    );
}

#[test]
fn includes_incoming_edges_but_only_for_configured_relationship_types() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let active = concept("active");
    let incoming = concept("incoming");
    let ignored = concept("ignored");
    for item in [&active, &incoming, &ignored] {
        graph.insert_concept(item).unwrap();
    }
    graph
        .insert_relationship(&Relationship::new(incoming.id, active.id, "supports"))
        .unwrap();
    graph
        .insert_relationship(&Relationship::new(active.id, ignored.id, "noise"))
        .unwrap();

    let config = ContextConfig {
        relationship_kinds: vec!["supports".into()],
        limits: ContextLimits::default(),
    };
    let assembler = ContextAssembler::new(&graph, &episodes, config).unwrap();
    let context = assembler.assemble(&request(active.id)).unwrap();

    assert_eq!(context.relevant_knowledge.len(), 1);
    assert_eq!(
        context.relevant_knowledge[0].adjacent_concept.id,
        incoming.id
    );
    assert_eq!(
        context.relevant_knowledge[0].relationship.source,
        incoming.id
    );
    assert_eq!(context.relevant_knowledge[0].relationship.target, active.id);
}

#[test]
fn applies_every_hard_limit_with_stable_selection() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let active = concept("active");
    let neighbor_a = concept("a");
    let neighbor_b = concept("b");
    let extra = concept("extra");
    for item in [&active, &neighbor_a, &neighbor_b, &extra] {
        graph.insert_concept(item).unwrap();
    }
    graph
        .insert_relationship(&Relationship::new(active.id, neighbor_b.id, "related"))
        .unwrap();
    graph
        .insert_relationship(&Relationship::new(active.id, neighbor_a.id, "related"))
        .unwrap();

    for (created_at, action) in [(1, "old"), (2, "new")] {
        let mut episode = Episode::new(action);
        episode.created_at = created_at;
        episode.action = Some(action.into());
        episodes.insert(&episode).unwrap();
    }

    let mut input = request(active.id);
    input.goal = Some("éclair".into());
    input.goal_reason = Some("déjà vu".into());
    input.entities.extend([neighbor_a.id, extra.id]);
    input.assumptions.extend([
        Assumption {
            description: "first".into(),
            basis: "observed".into(),
            concept: None,
        },
        Assumption {
            description: "second".into(),
            basis: "inferred".into(),
            concept: None,
        },
    ]);
    input.environment.insert("b".into(), "discarded".into());
    input.environment.insert("a".into(), "abcdef".into());

    let config = ContextConfig {
        relationship_kinds: vec!["related".into()],
        limits: ContextLimits {
            max_goal_chars: 3,
            max_entities: 2,
            max_relationships: 1,
            max_relevant_procedures: 1,
            max_recent_episodes: 1,
            max_recent_text_chars: 4,
            max_assumptions: 1,
            max_assumption_chars: 4,
            max_environment_entries: 1,
            max_environment_key_chars: 4,
            max_environment_value_chars: 4,
            max_embedded_items: 1,
            max_value_depth: 2,
            graph_hops: 1,
        },
    };
    let assembler = ContextAssembler::new(&graph, &episodes, config).unwrap();
    let first = assembler.assemble(&input).unwrap();
    let second = assembler.assemble(&input).unwrap();

    assert_eq!(first.goal.as_deref(), Some("écl"));
    assert_eq!(first.goal_reason.as_deref(), Some("déj"));
    assert_eq!(first.entities.len(), 2);
    assert_eq!(first.entities[0], active.id);
    assert_eq!(first.relevant_knowledge.len(), 1);
    assert_eq!(first.recent_episodes.len(), 1);
    assert_eq!(first.recent_episodes[0].action.as_deref(), Some("new"));
    assert_eq!(first.assumptions.len(), 1);
    assert_eq!(first.assumptions[0].description, "firs");
    assert_eq!(first.environment.len(), 1);
    assert_eq!(first.environment["a"], Value::from("abcd"));

    let first_edges = first
        .relevant_knowledge
        .iter()
        .map(|item| item.relationship.id)
        .collect::<Vec<_>>();
    let second_edges = second
        .relevant_knowledge
        .iter()
        .map(|item| item.relationship.id)
        .collect::<Vec<_>>();
    assert_eq!(first.entities, second.entities);
    assert_eq!(first_edges, second_edges);
    assert_eq!(first.recent_episodes, second.recent_episodes);
}

#[test]
fn rejects_context_configuration_above_absolute_hard_bounds() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let mut config = ContextConfig::default();
    config.limits.max_relationships = spoon_reason::MAX_CONTEXT_COLLECTION_ITEMS + 1;

    assert!(matches!(
        ContextAssembler::new(&graph, &episodes, config),
        Err(ContextError::LimitExceedsHardMaximum {
            name: "max_relationships",
            ..
        })
    ));
}

#[test]
fn relevant_history_is_prioritized_over_newer_unrelated_history() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let active = concept("active");
    graph.insert_concept(&active).unwrap();

    let mut relevant = Episode::new("relevant but older");
    relevant.created_at = 10;
    relevant.interpretations.push(Interpretation {
        meaning: active.id,
        weight: 1.0,
        chosen: true,
    });
    episodes.insert(&relevant).unwrap();

    let mut unrelated = Episode::new("unrelated and newer");
    unrelated.created_at = 20;
    episodes.insert(&unrelated).unwrap();

    let config = ContextConfig {
        limits: ContextLimits {
            max_recent_episodes: 1,
            ..ContextLimits::default()
        },
        ..ContextConfig::default()
    };
    let context = ContextAssembler::new(&graph, &episodes, config)
        .unwrap()
        .assemble(&request(active.id))
        .unwrap();

    assert_eq!(context.recent_episodes.len(), 1);
    assert_eq!(context.recent_episodes[0].episode_id, relevant.id);
}

#[test]
fn inactive_graph_material_is_excluded_from_context() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let active = concept("active");
    let kept = concept("kept");
    let mut retired_neighbor = concept("retired-neighbor");
    retired_neighbor.lifecycle = Lifecycle::Retired;
    for item in [&active, &kept, &retired_neighbor] {
        graph.insert_concept(item).unwrap();
    }

    graph
        .insert_relationship(&Relationship::new(active.id, kept.id, "supports"))
        .unwrap();
    graph
        .insert_relationship(&Relationship::new(
            active.id,
            retired_neighbor.id,
            "supports",
        ))
        .unwrap();
    let mut retired_relationship = Relationship::new(kept.id, active.id, "supports");
    retired_relationship.lifecycle = Lifecycle::Retired;
    graph.insert_relationship(&retired_relationship).unwrap();

    let kept_procedure =
        Procedure::new("kept", Vec::new(), Expr::Literal(Value::Null)).with_concept(active.id);
    graph.insert_procedure(&kept_procedure).unwrap();
    let mut retired_procedure =
        Procedure::new("retired", Vec::new(), Expr::Literal(Value::Null)).with_concept(active.id);
    retired_procedure.lifecycle = Lifecycle::Retired;
    graph.insert_procedure(&retired_procedure).unwrap();

    let config = ContextConfig {
        relationship_kinds: vec!["supports".into()],
        ..ContextConfig::default()
    };
    let context = ContextAssembler::new(&graph, &episodes, config)
        .unwrap()
        .assemble(&request(active.id))
        .unwrap();

    assert_eq!(context.relevant_knowledge.len(), 1);
    assert_eq!(context.relevant_knowledge[0].adjacent_concept.id, kept.id);
    assert_eq!(context.relevant_procedures.len(), 1);
    assert_eq!(context.relevant_procedures[0].id, kept_procedure.id);
}

#[test]
fn rejects_invalid_required_limits_unmarked_assumptions_and_budget() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let mut config = ContextConfig::default();
    config.limits.max_entities = 0;
    assert!(matches!(
        ContextAssembler::new(&graph, &episodes, config),
        Err(ContextError::InvalidLimit("max_entities"))
    ));

    let active = concept("active");
    graph.insert_concept(&active).unwrap();
    let assembler = ContextAssembler::new(&graph, &episodes, ContextConfig::default()).unwrap();
    let mut input = request(active.id);
    input.assumptions.push(Assumption {
        description: "hidden premise".into(),
        basis: "   ".into(),
        concept: None,
    });
    assert!(matches!(
        assembler.assemble(&input),
        Err(ContextError::UnmarkedAssumption { index: 0 })
    ));

    input.assumptions.clear();
    input.budget_remaining.cost = f64::NAN;
    assert!(matches!(
        assembler.assemble(&input),
        Err(ContextError::InvalidBudgetCost(_))
    ));
}

#[test]
fn rejects_active_concepts_that_are_not_in_the_graph() {
    let graph = KnowledgeStore::in_memory().unwrap();
    let episodes = EpisodeStore::in_memory().unwrap();
    let missing = ConceptId::new();
    let assembler = ContextAssembler::new(&graph, &episodes, ContextConfig::default()).unwrap();

    assert!(matches!(
        assembler.assemble(&request(missing)),
        Err(ContextError::MissingConcept(id)) if id == missing
    ));
}
