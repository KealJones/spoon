//! Prompt builders for the teacher seat.
//!
//! Each function loads the base template from data/prompts/ via include_str!
//! and substitutes context values using plain string replacement.

pub fn spec_prompt(capability: &str, context: &str, known_types: &[String]) -> String {
    include_str!("../../../../data/prompts/teacher_spec.md")
        .replace("{known_types}", &known_types.join(", "))
        .replace("{capability}", capability)
        .replace("{context}", context)
}

pub fn phrasings_prompt(sce: &str, verb: &str, n: usize) -> String {
    include_str!("../../../../data/prompts/teacher_phrasings.md")
        .replace("{sce}", sce)
        .replace("{verb}", verb)
        .replace("{n}", &n.to_string())
}

pub fn concept_prompt(noun: &str, context: &str, known_concepts: &[String]) -> String {
    include_str!("../../../../data/prompts/teacher_concept.md")
        .replace("{known_concepts}", &known_concepts.join(", "))
        .replace("{noun}", noun)
        .replace("{context}", context)
}

pub fn stance_prompt(topic: &str, context: &str) -> String {
    include_str!("../../../../data/prompts/teacher_stance.md")
        .replace("{topic}", topic)
        .replace("{context}", context)
}

pub fn curriculum_prompt(n: usize, existing_verbs: &[String], themes: &[String]) -> String {
    include_str!("../../../../data/prompts/teacher_curriculum.md")
        .replace("{n}", &n.to_string())
        .replace("{existing_verbs}", &existing_verbs.join(", "))
        .replace("{themes}", &themes.join(", "))
}
