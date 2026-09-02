//! Kernel concept registrations.

use crate::types::*;

use super::super::Kernel;

pub fn register(k: &mut Kernel) {
    // Root
    k.register_concept(entity("Thing", &["thing", "object", "item"], &[], "any thing or object"));
    k.register_concept(entity("Agent", &["agent"], &["Thing"], "an entity that acts"));
    k.register_concept(entity(
        "Person",
        &["person", "people", "human", "man", "woman", "guy", "girl"],
        &["Agent"],
        "a human person",
    ));
    k.register_concept(entity("User", &["user"], &["Person"], "the person talking to Spoon"));
    k.register_concept(entity(
        "Assistant",
        &["assistant", "spoon"],
        &["Person"],
        "the Spoon assistant",
    ));
    k.register_concept(entity(
        "Place",
        &["place", "location"],
        &["Thing"],
        "a physical or virtual location",
    ));
    k.register_concept(entity(
        "Organization",
        &["organization", "company", "team"],
        &["Agent"],
        "an organization or institution",
    ));
    k.register_concept(entity("Animal", &["animal"], &["Thing"], "a living creature"));
    k.register_concept(entity(
        "Statement",
        &["statement", "claim", "report", "question", "answer", "request"],
        &[],
        "a linguistic act or proposition",
    ));

    // Primitive concepts
    k.register_concept(prim("Number", Type::Float, &["number"], &[], "a numeric quantity"));
    k.register_concept(prim(
        "Text",
        Type::Text,
        &["text", "string", "word"],
        &[],
        "a piece of text",
    ));
    k.register_concept(prim(
        "dialog.Move",
        Type::Json,
        &[],
        &[],
        "a conversational move",
    ));
    k.register_concept(prim(
        "File",
        Type::Path,
        &["file", "folder", "directory", "path"],
        &[],
        "a filesystem path",
    ));
    k.register_concept(prim(
        "Time",
        Type::DateTime,
        &["time", "date", "day", "moment"],
        &[],
        "a point in time",
    ));
}

fn entity(id: &str, nouns: &[&str], extends: &[&str], description: &str) -> Concept {
    Concept {
        id: ConceptId(id.into()),
        kind: ConceptKind::Entity,
        extends: extends.iter().map(|s| ConceptId(s.to_string())).collect(),
        role_of: None,
        nouns: nouns.iter().map(|s| s.to_string()).collect(),
        description: description.into(),
        tier: Tier::Kernel,
        provenance: Provenance::Kernel,
    }
}

fn prim(id: &str, ty: Type, nouns: &[&str], extends: &[&str], description: &str) -> Concept {
    Concept {
        id: ConceptId(id.into()),
        kind: ConceptKind::Primitive { ty },
        extends: extends.iter().map(|s| ConceptId(s.to_string())).collect(),
        role_of: None,
        nouns: nouns.iter().map(|s| s.to_string()).collect(),
        description: description.into(),
        tier: Tier::Kernel,
        provenance: Provenance::Kernel,
    }
}
