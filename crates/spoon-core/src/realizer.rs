//! Claim-faithful surface realization without letting a model write text.
//!
//! A deterministic join of grounded claims is safe but blunt. Letting a model
//! write the reply reads better and reintroduces every failure a grounded
//! system exists to avoid: invented facts, dropped claims, injected negation,
//! hedging that was never in the plan.
//!
//! This module takes the third option. The model selects an Engine-owned
//! template and an order for the claims. It emits no user-visible characters at
//! all, so fabrication is not checked for, it is structurally impossible. The
//! cost is real and accepted: replies are less varied than free generation.
//!
//! The one place text is adjusted is sentence mechanics. A template that
//! continues a sentence strips the previous claim's terminator, and a claim
//! that lands mid-sentence may have its initial lowercased, but only when the
//! original utterance itself used that word lowercase. That keeps a proper noun
//! from being quietly decapitalized on a guess.

use serde::{Deserialize, Serialize};

use crate::language::{
    DialogueAct, GroundedClaim, LanguageError, PlannedClaim, ResponsePlan, ResponseTone, TokenKind,
    TokenStream,
};

/// How many claims a template accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateArity {
    Exact(usize),
    Variadic,
}

/// Per-slot sentence mechanics. These are Engine-owned rewrites of punctuation
/// and case only; they never change a claim's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlotMechanics {
    /// Drop one trailing sentence terminator, because the template continues
    /// the sentence rather than ending it.
    pub strip_terminator: bool,
    /// Lowercase the first character, but only with evidence from the original
    /// utterance that the word is normally lowercase.
    pub lowercase_initial: bool,
}

/// One tone's wording of a template. For a fixed template these are format
/// strings with `{i}` placeholders; for a variadic one they are separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateForms {
    pub neutral: &'static str,
    pub direct: &'static str,
    pub warm: &'static str,
    pub formal: &'static str,
}

impl TemplateForms {
    pub fn for_tone(&self, tone: ResponseTone) -> &'static str {
        match tone {
            ResponseTone::Neutral => self.neutral,
            ResponseTone::Direct => self.direct,
            ResponseTone::Warm => self.warm,
            ResponseTone::Formal => self.formal,
        }
    }

    const fn uniform(form: &'static str) -> Self {
        Self {
            neutral: form,
            direct: form,
            warm: form,
            formal: form,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateShape {
    /// A format string per tone, with `{i}` naming the i-th ordered claim.
    Fixed(TemplateForms),
    /// A separator per tone, joining any number of claims.
    Joined(TemplateForms),
}

#[derive(Debug, Clone, Copy)]
pub struct RealizationTemplate {
    pub id: &'static str,
    pub arity: TemplateArity,
    pub shape: TemplateShape,
    /// Required act per slot. `None` accepts any act.
    pub slot_acts: &'static [Option<DialogueAct>],
    pub mechanics: &'static [SlotMechanics],
    /// When true, slot 0 must produce a value slot 1 consumes. Used by
    /// narrative templates whose wording asserts a sequence.
    pub requires_dependency_order: bool,
}

const PLAIN: SlotMechanics = SlotMechanics {
    strip_terminator: false,
    lowercase_initial: false,
};
const CONTINUES: SlotMechanics = SlotMechanics {
    strip_terminator: true,
    lowercase_initial: false,
};
const TRAILING: SlotMechanics = SlotMechanics {
    strip_terminator: false,
    lowercase_initial: true,
};

/// The pinned template set. This is versioned Engine data, not model output. A
/// template id outside this set is a rejected realization.
pub const TEMPLATES: &[RealizationTemplate] = &[
    RealizationTemplate {
        id: "join.sentences",
        arity: TemplateArity::Variadic,
        shape: TemplateShape::Joined(TemplateForms::uniform(" ")),
        slot_acts: &[],
        mechanics: &[],
        requires_dependency_order: false,
    },
    RealizationTemplate {
        id: "join.and",
        arity: TemplateArity::Exact(2),
        shape: TemplateShape::Fixed(TemplateForms {
            neutral: "{0}, and {1}",
            direct: "{0}, and {1}",
            warm: "{0}, and {1}",
            formal: "{0}, and additionally {1}",
        }),
        slot_acts: &[None, None],
        mechanics: &[CONTINUES, TRAILING],
        requires_dependency_order: false,
    },
    RealizationTemplate {
        id: "join.and.list",
        arity: TemplateArity::Exact(3),
        shape: TemplateShape::Fixed(TemplateForms::uniform("{0}, {1}, and {2}")),
        slot_acts: &[None, None, None],
        mechanics: &[CONTINUES, CONTINUES, TRAILING],
        requires_dependency_order: false,
    },
    RealizationTemplate {
        id: "join.then",
        arity: TemplateArity::Exact(2),
        shape: TemplateShape::Fixed(TemplateForms {
            neutral: "{0} Then {1}",
            direct: "{0} Then {1}",
            warm: "{0} Then {1}",
            formal: "{0} Subsequently {1}",
        }),
        slot_acts: &[None, None],
        mechanics: &[PLAIN, TRAILING],
        requires_dependency_order: true,
    },
    RealizationTemplate {
        id: "join.lead.ack",
        arity: TemplateArity::Exact(2),
        shape: TemplateShape::Fixed(TemplateForms::uniform("{0} {1}")),
        slot_acts: &[Some(DialogueAct::Acknowledge), None],
        mechanics: &[PLAIN, PLAIN],
        requires_dependency_order: false,
    },
    RealizationTemplate {
        id: "join.ack.and",
        arity: TemplateArity::Exact(3),
        shape: TemplateShape::Fixed(TemplateForms::uniform("{0} {1}, and {2}")),
        slot_acts: &[Some(DialogueAct::Acknowledge), None, None],
        mechanics: &[PLAIN, CONTINUES, TRAILING],
        requires_dependency_order: false,
    },
];

pub fn template(id: &str) -> Option<&'static RealizationTemplate> {
    TEMPLATES.iter().find(|template| template.id == id)
}

/// Untrusted realizer output. There is deliberately no text field: the model
/// picks a shape and an order, and the Engine supplies every character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RealizationProposal {
    pub template_id: String,
    /// A permutation of the plan's grounded claim ids.
    pub slot_order: Vec<String>,
    pub tone: ResponseTone,
}

/// A realized reply plus the claim order that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Realization {
    pub text: String,
    pub template_id: String,
    pub slot_order: Vec<String>,
    pub tone: ResponseTone,
}

/// Dependency edges between claims, so a template asserting a sequence cannot
/// reorder a consumer ahead of its producer. Keyed consumer -> producers.
pub type ClaimDependencies = std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;

impl RealizationProposal {
    /// Validates and applies the proposal. Any failure returns an error so the
    /// caller falls back to the deterministic renderer with the plan intact.
    pub fn realize(
        &self,
        plan: &ResponsePlan,
        dependencies: &ClaimDependencies,
        original: &TokenStream,
    ) -> Result<Realization, LanguageError> {
        let Some(template) = template(&self.template_id) else {
            return Err(LanguageError::Invalid(format!(
                "realization names template {:?}, which is not in the pinned set",
                self.template_id
            )));
        };

        let grounded: Vec<&GroundedClaim> = plan
            .claims
            .iter()
            .filter_map(|claim| match claim {
                PlannedClaim::Grounded(claim) => Some(claim),
                PlannedClaim::Unsupported { .. } => None,
            })
            .collect();

        if grounded.is_empty() {
            return Err(LanguageError::Invalid(
                "a realization needs at least one grounded claim".into(),
            ));
        }

        match template.arity {
            TemplateArity::Exact(arity) if arity != grounded.len() => {
                return Err(LanguageError::Invalid(format!(
                    "template {:?} takes {arity} claims but the plan has {}",
                    template.id,
                    grounded.len()
                )));
            }
            _ => {}
        }

        // The order must be an exact permutation. Omitting a claim would drop
        // an answer, and repeating one would assert it twice.
        if self.slot_order.len() != grounded.len() {
            return Err(LanguageError::Invalid(format!(
                "slot order covers {} claims but the plan has {}",
                self.slot_order.len(),
                grounded.len()
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut ordered: Vec<&GroundedClaim> = Vec::with_capacity(grounded.len());
        for id in &self.slot_order {
            if !seen.insert(id.clone()) {
                return Err(LanguageError::Invalid(format!(
                    "slot order repeats claim {id:?}"
                )));
            }
            let Some(claim) = grounded.iter().find(|claim| &claim.id == id) else {
                // Covers both an unknown id and an Unsupported claim being
                // presented as if it were a fact.
                return Err(LanguageError::Invalid(format!(
                    "slot order names {id:?}, which is not a grounded claim in the plan"
                )));
            };
            ordered.push(claim);
        }

        let plan_act = plan.dialogue_move.act;
        for (index, required) in template.slot_acts.iter().enumerate() {
            if let Some(required) = required
                && ordered[index].effective_act(plan_act) != *required
            {
                return Err(LanguageError::Invalid(format!(
                    "template {:?} requires {:?} in slot {index}, found {:?}",
                    template.id,
                    required,
                    ordered[index].effective_act(plan_act)
                )));
            }
        }

        // A consumer may never be worded ahead of its producer. Otherwise
        // "double that is 8, and 2 + 2 is 4" reads as though the second
        // sentence followed from the first.
        for (position, claim) in ordered.iter().enumerate() {
            let Some(producers) = dependencies.get(&claim.id) else {
                continue;
            };
            for producer in producers {
                let producer_position = ordered
                    .iter()
                    .position(|candidate| &candidate.id == producer);
                if let Some(producer_position) = producer_position
                    && producer_position > position
                {
                    return Err(LanguageError::Invalid(format!(
                        "claim {:?} consumes {producer:?} and cannot be worded before it",
                        claim.id
                    )));
                }
            }
        }

        if template.requires_dependency_order {
            let consumes_first = dependencies
                .get(&ordered[1].id)
                .is_some_and(|producers| producers.contains(&ordered[0].id));
            if !consumes_first {
                return Err(LanguageError::Invalid(format!(
                    "template {:?} asserts a sequence, but {:?} does not follow from {:?}",
                    template.id, ordered[1].id, ordered[0].id
                )));
            }
        }

        let pieces: Vec<String> = ordered
            .iter()
            .enumerate()
            .map(|(index, claim)| {
                let mechanics = template.mechanics.get(index).copied().unwrap_or_default();
                apply_mechanics(&claim.text, mechanics, original)
            })
            .collect();

        let text = match template.shape {
            TemplateShape::Joined(forms) => pieces.join(forms.for_tone(self.tone)),
            TemplateShape::Fixed(forms) => {
                let mut rendered = forms.for_tone(self.tone).to_string();
                for (index, piece) in pieces.iter().enumerate() {
                    rendered = rendered.replace(&format!("{{{index}}}"), piece);
                }
                rendered
            }
        };

        Ok(Realization {
            text,
            template_id: template.id.to_string(),
            slot_order: self.slot_order.clone(),
            tone: self.tone,
        })
    }
}

fn apply_mechanics(text: &str, mechanics: SlotMechanics, original: &TokenStream) -> String {
    let mut result = text.to_string();

    if mechanics.strip_terminator {
        // One terminator only. A claim ending in "..." keeps all of them,
        // because collapsing an ellipsis would change what the claim said.
        let ends_a_sentence = (result.ends_with('.') && !result.ends_with(".."))
            || result.ends_with('!')
            || result.ends_with('?');
        if ends_a_sentence {
            result.pop();
        }
    }

    if mechanics.lowercase_initial {
        result = lowercase_initial_with_evidence(&result, original);
    }

    result
}

/// Lowercases a claim's first character only when the original utterance used
/// that same word with a lowercase initial. Without that evidence the word may
/// be a name, and decapitalizing a name to make a sentence flow is a worse
/// error than a capital letter mid-sentence.
fn lowercase_initial_with_evidence(text: &str, original: &TokenStream) -> String {
    let Some(first) = text.chars().next() else {
        return text.to_string();
    };
    if !first.is_uppercase() {
        return text.to_string();
    }

    let word: String = text
        .chars()
        .take_while(|character| character.is_alphanumeric())
        .collect();
    if word.is_empty() {
        return text.to_string();
    }

    let attested = original
        .tokens
        .iter()
        .filter(|token| token.kind == TokenKind::Word)
        .filter_map(|token| original.slice(&token.span))
        .any(|surface| {
            surface.eq_ignore_ascii_case(&word)
                && surface.chars().next().is_some_and(char::is_lowercase)
        });
    if !attested {
        return text.to_string();
    }

    let mut characters = text.chars();
    let mut lowered = String::with_capacity(text.len());
    if let Some(first) = characters.next() {
        lowered.extend(first.to_lowercase());
    }
    lowered.push_str(characters.as_str());
    lowered
}
