//! Grow: how Spoon acquires capabilities it does not have.
//!
//! - `synth`: budgeted, typed, enumerative program synthesis from I/O examples
//!   with observational-equivalence pruning over the pure CAN actions.
//! - `consolidate`: Stitch-style abstraction of repeated sub-programs into new
//!   library actions.
//!
//! Only `Effect::Pure`, non-Deprecated actions are enumerated. No LLM used.

pub mod consolidate;
pub mod oe;
pub mod synth;

pub use consolidate::{consolidate, ConsolidationReport};
pub use synth::{describe, synthesize, SynthBudget, SynthOutcome};

use spoon_core::types::{
    Action, ActionId, Effect, Impl, Input, Provenance, Role, Spec, Stats, Tier, Program,
};

/// Wrap a found Program as a provisional CAN action.
///
/// - `id` = `"learned.<name_hint>_<hash8>"`
/// - `verbs` = `spec.verbs`, falling back to `[spec.name_hint]`
/// - inputs from `spec.params` + `spec.param_names` (fallback `"arg0".."argN"`)
/// - `output` = `spec.ret`
/// - `effect` = `Pure` (synthesiser only enumerates pure actions)
/// - `tier` = `Provisional`, `provenance` = `Synthesized { spec_id }`
pub fn action_from_program(spec: &Spec, program: &Program) -> Action {
    let id_str = format!("learned.{}_{}", spec.name_hint, &program.hash()[..8]);

    let inputs: Vec<Input> = spec
        .params
        .iter()
        .enumerate()
        .map(|(i, ty)| {
            let name = spec
                .param_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("arg{}", i));
            Input::required(&name, ty.clone())
        })
        .collect();

    let verbs = if spec.verbs.is_empty() {
        vec![spec.name_hint.clone()]
    } else {
        spec.verbs.clone()
    };

    Action {
        id: ActionId(id_str),
        inputs,
        output: spec.ret.clone(),
        effect: Effect::Pure,
        imp: Impl::Program { program: program.clone() },
        role: Role::Command,
        verbs,
        phrasings: spec.phrasings.clone(),
        description: spec.description.clone(),
        tier: Tier::Provisional,
        provenance: Provenance::Synthesized { spec_id: spec.id.clone() },
        stats: Stats::default(),
    }
}
