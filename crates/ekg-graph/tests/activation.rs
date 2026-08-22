use ekg_core::{Concept, Lifecycle, MutabilityClass, Relationship};
use ekg_graph::{
    ActivationSeed, ActivationSpreadQuery, KnowledgeStore, RelationshipDirection,
    TraversalDirection, TypedRelationshipTraversal,
};

fn concept(name: &str) -> Concept {
    Concept::new(name, MutabilityClass::Definitional)
}

fn query(seed: ekg_core::ConceptId) -> ActivationSpreadQuery {
    ActivationSpreadQuery {
        seeds: vec![ActivationSeed {
            concept: seed,
            activation: 1.0,
        }],
        traversals: vec![TypedRelationshipTraversal {
            kind: "supports".into(),
            direction: TraversalDirection::Outgoing,
            decay: 0.5,
        }],
        max_hops: 2,
        max_candidates: 32,
        max_expansions: 128,
        min_activation: 0.001,
    }
}

#[test]
fn typed_activation_spread_respects_kind_direction_hops_and_decay() {
    let store = KnowledgeStore::in_memory().unwrap();
    let a = concept("a");
    let b = concept("b");
    let c = concept("c");
    let noise = concept("noise");
    let incoming = concept("incoming");
    for item in [&a, &b, &c, &noise, &incoming] {
        store.insert_concept(item).unwrap();
    }

    let mut ab = Relationship::new(a.id, b.id, "supports");
    ab.strength = 0.8;
    store.insert_relationship(&ab).unwrap();
    let mut bc = Relationship::new(b.id, c.id, "supports");
    bc.strength = 0.5;
    store.insert_relationship(&bc).unwrap();
    store
        .insert_relationship(&Relationship::new(a.id, noise.id, "mentions"))
        .unwrap();
    store
        .insert_relationship(&Relationship::new(incoming.id, a.id, "supports"))
        .unwrap();

    let result = store.activation_spread(&query(a.id)).unwrap();

    assert_eq!(result.candidates.len(), 2);
    assert_eq!(result.candidates[0].concept, b.id);
    assert!((result.candidates[0].activation - 0.4).abs() < 1e-12);
    assert_eq!(result.candidates[0].min_hops, 1);
    assert_eq!(result.candidates[0].strongest_path.len(), 1);
    assert_eq!(result.candidates[0].strongest_path[0].relationship, ab.id);
    assert_eq!(
        result.candidates[0].strongest_path[0].direction,
        RelationshipDirection::Outgoing
    );
    assert_eq!(result.candidates[1].concept, c.id);
    assert!((result.candidates[1].activation - 0.1).abs() < 1e-12);
    assert_eq!(result.candidates[1].min_hops, 2);
    assert!(
        result
            .candidates
            .iter()
            .all(|item| item.concept != noise.id)
    );
    assert!(
        result
            .candidates
            .iter()
            .all(|item| item.concept != incoming.id)
    );
    assert!(!result.truncated);
}

#[test]
fn bidirectional_typed_traversal_records_actual_direction() {
    let store = KnowledgeStore::in_memory().unwrap();
    let center = concept("center");
    let outgoing = concept("outgoing");
    let incoming = concept("incoming");
    for item in [&center, &outgoing, &incoming] {
        store.insert_concept(item).unwrap();
    }
    store
        .insert_relationship(&Relationship::new(center.id, outgoing.id, "supports"))
        .unwrap();
    store
        .insert_relationship(&Relationship::new(incoming.id, center.id, "supports"))
        .unwrap();
    let mut input = query(center.id);
    input.max_hops = 1;
    input.traversals[0].direction = TraversalDirection::Both;

    let result = store.activation_spread(&input).unwrap();

    assert_eq!(result.candidates.len(), 2);
    let outgoing_candidate = result
        .candidates
        .iter()
        .find(|item| item.concept == outgoing.id)
        .unwrap();
    let incoming_candidate = result
        .candidates
        .iter()
        .find(|item| item.concept == incoming.id)
        .unwrap();
    assert_eq!(
        outgoing_candidate.strongest_path[0].direction,
        RelationshipDirection::Outgoing
    );
    assert_eq!(
        incoming_candidate.strongest_path[0].direction,
        RelationshipDirection::Incoming
    );
}

#[test]
fn independent_paths_accumulate_activation_without_normalizing_candidates() {
    let store = KnowledgeStore::in_memory().unwrap();
    let seed = concept("seed");
    let left = concept("left");
    let right = concept("right");
    let joined = concept("joined");
    for item in [&seed, &left, &right, &joined] {
        store.insert_concept(item).unwrap();
    }
    for (source, target, strength) in [
        (seed.id, left.id, 0.5),
        (seed.id, right.id, 0.5),
        (left.id, joined.id, 0.5),
        (right.id, joined.id, 0.5),
    ] {
        let mut relationship = Relationship::new(source, target, "supports");
        relationship.strength = strength;
        store.insert_relationship(&relationship).unwrap();
    }
    let mut input = query(seed.id);
    input.traversals[0].decay = 1.0;

    let result = store.activation_spread(&input).unwrap();
    let joined_candidate = result
        .candidates
        .iter()
        .find(|item| item.concept == joined.id)
        .unwrap();

    assert!((joined_candidate.activation - 0.4375).abs() < 1e-12);
    assert_eq!(joined_candidate.min_hops, 2);
    let activation_sum: f64 = result.candidates.iter().map(|item| item.activation).sum();
    assert!(activation_sum > 1.0);
}

#[test]
fn expansion_and_candidate_budgets_are_hard_and_deterministic() {
    let store = KnowledgeStore::in_memory().unwrap();
    let seed = concept("seed");
    store.insert_concept(&seed).unwrap();
    let mut neighbors = Vec::new();
    for (index, strength) in [0.9, 0.8, 0.7, 0.6].into_iter().enumerate() {
        let neighbor = concept(&format!("n{index}"));
        store.insert_concept(&neighbor).unwrap();
        let mut relationship = Relationship::new(seed.id, neighbor.id, "supports");
        relationship.strength = strength;
        store.insert_relationship(&relationship).unwrap();
        neighbors.push(neighbor);
    }
    let mut input = query(seed.id);
    input.max_hops = 1;
    input.max_candidates = 2;
    input.max_expansions = 3;

    let first = store.activation_spread(&input).unwrap();
    let second = store.activation_spread(&input).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.expansions, 3);
    assert_eq!(first.candidates.len(), 2);
    assert!(first.truncated);
    assert_eq!(first.candidates[0].concept, neighbors[0].id);
    assert_eq!(first.candidates[1].concept, neighbors[1].id);
}

#[test]
fn inactive_relationships_and_concepts_do_not_receive_or_propagate_activation() {
    let store = KnowledgeStore::in_memory().unwrap();
    let seed = concept("seed");
    let active = concept("active");
    let mut retired = concept("retired");
    retired.lifecycle = Lifecycle::Retired;
    let hidden = concept("hidden");
    for item in [&seed, &active, &retired, &hidden] {
        store.insert_concept(item).unwrap();
    }
    store
        .insert_relationship(&Relationship::new(seed.id, active.id, "supports"))
        .unwrap();
    store
        .insert_relationship(&Relationship::new(seed.id, retired.id, "supports"))
        .unwrap();
    let mut retired_edge = Relationship::new(seed.id, hidden.id, "supports");
    retired_edge.lifecycle = Lifecycle::Retired;
    store.insert_relationship(&retired_edge).unwrap();

    let result = store.activation_spread(&query(seed.id)).unwrap();

    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].concept, active.id);
}

#[test]
fn invalid_or_unbounded_activation_queries_are_rejected() {
    let store = KnowledgeStore::in_memory().unwrap();
    let seed = concept("seed");
    store.insert_concept(&seed).unwrap();

    let mut invalid_decay = query(seed.id);
    invalid_decay.traversals[0].decay = f64::NAN;
    assert!(store.activation_spread(&invalid_decay).is_err());

    let mut too_many_hops = query(seed.id);
    too_many_hops.max_hops = ekg_graph::MAX_ACTIVATION_HOPS + 1;
    assert!(store.activation_spread(&too_many_hops).is_err());

    let mut too_many_expansions = query(seed.id);
    too_many_expansions.max_expansions = ekg_graph::MAX_ACTIVATION_EXPANSIONS + 1;
    assert!(store.activation_spread(&too_many_expansions).is_err());
}
