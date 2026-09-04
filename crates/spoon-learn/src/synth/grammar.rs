//! What the search is searching over.
//!
//! Two halves of the same question. The problem is what the examples pin down:
//! the arity, the inputs to run against, and the outputs to match. The grammar
//! is what there is to build with: every pure operator the store knows about,
//! plus the terminals the examples themselves suggest.

use std::collections::{HashMap, HashSet};

use spoon_concept::{
    Concept, ConceptId, ContentId, Effect, RealizationSpec, SymbolId, Tier, is_ground_term,
    max_hole, pre_order,
};
use spoon_eval::NativeRegistry;
use spoon_seat::Spec;
use spoon_store::Store;

/// Widest call the search will build.
///
/// Nothing in the bootstrap set needs more than three arguments to be useful,
/// and each extra position multiplies the level by the size of the bank.
const MAX_ARITY: usize = 3;

// ---------------------------------------------------------------------------
// The problem, extracted from the spec
// ---------------------------------------------------------------------------

/// A spec reduced to exactly what the search needs, with everything validated
/// once up front so the inner loop can assume it.
pub(super) struct Problem {
    pub(super) target: SymbolId,
    pub(super) arity: usize,
    pub(super) inputs: Vec<Vec<Concept>>,
    pub(super) outputs: Vec<Concept>,
    pub(super) expected: Vec<ContentId>,
}

impl Problem {
    pub(super) fn from_spec(spec: &Spec) -> Result<Problem, String> {
        let target = head_symbol(&spec.target).ok_or_else(|| {
            "the target has to be a named concept or a call on one; a realization cannot attach \
             to a literal value or a hole"
                .to_string()
        })?;

        if spec.examples.is_empty() {
            return Err(
                "a spec with no examples constrains nothing, so every program would fit it"
                    .to_string(),
            );
        }

        let arity = spec.examples[0].0.len();
        for (index, (inputs, output)) in spec.examples.iter().enumerate() {
            if inputs.len() != arity {
                return Err(format!(
                    "example {index} takes {} arguments but example 0 takes {arity}",
                    inputs.len()
                ));
            }
            if inputs.iter().any(|c| !is_ground_term(c)) || !is_ground_term(output) {
                return Err(format!(
                    "example {index} contains a hole; examples have to be concrete for the search \
                     to check a candidate against them"
                ));
            }
        }

        Ok(Problem {
            target,
            arity,
            inputs: spec.examples.iter().map(|(i, _)| i.clone()).collect(),
            outputs: spec.examples.iter().map(|(_, o)| o.clone()).collect(),
            expected: spec.examples.iter().map(|(_, o)| o.content_id()).collect(),
        })
    }

    /// Two examples that give the same inputs different answers.
    ///
    /// No body can satisfy both, because evaluation is deterministic. Worth
    /// spotting up front: the alternative is searching the entire space to
    /// discover something the spec already said.
    pub(super) fn is_contradictory(&self) -> bool {
        let mut answers: HashMap<Vec<ContentId>, ContentId> = HashMap::new();
        for (inputs, expected) in self.inputs.iter().zip(self.expected.iter()) {
            let key: Vec<ContentId> = inputs.iter().map(Concept::content_id).collect();
            if let Some(previous) = answers.insert(key, *expected)
                && previous != *expected
            {
                return true;
            }
        }
        false
    }
}

/// The symbol a realization would attach to: the concept itself when it is a
/// named atomic, or the head when the target is written as a call.
fn head_symbol(target: &Concept) -> Option<SymbolId> {
    match target {
        Concept::Atomic(ConceptId::Named(sym)) => Some(*sym),
        Concept::Compound { .. } => target.head_symbol(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The grammar
// ---------------------------------------------------------------------------

/// One way to build a compound: a head concept applied to a fixed number of
/// arguments.
pub(super) struct Operator {
    pub(super) head: Concept,
    pub(super) arity: usize,
    /// Sort key. Search order has to be identical run to run, and a printable
    /// name also makes the order predictable to a human reading a trace.
    label: String,
}

/// Every pure way to build something, drawn from the store rather than from a
/// hardcoded list.
///
/// This is the point of searching over concepts instead of over kernel
/// functions: a capability learned last week is an operator this week, with no
/// special case anywhere. `Composed` bodies join the table on the same footing
/// as natives, and their arity comes from the holes they bind.
///
/// Excluded: anything not `Effect::Pure`, deprecated realizations, natives that
/// declare a stronger effect than the realization claims, rules (their
/// applicability depends on a pattern match rather than on a head and an
/// arity), neural realizations (an LLM has no business inside the interior),
/// and externals (they exist to touch the world). The target itself is
/// excluded too: a body that calls the concept it defines is either circular or
/// a copy of a realization that already exists.
pub(super) fn operators(
    store: &Store,
    registry: &NativeRegistry,
    exclude: SymbolId,
) -> spoon_store::Result<Vec<Operator>> {
    let names: HashMap<SymbolId, String> = store.all_symbols()?.into_iter().collect();

    let mut seen: HashSet<(SymbolId, usize)> = HashSet::new();
    let mut out: Vec<Operator> = Vec::new();

    for realization in store.all_realizations()? {
        if realization.tier == Tier::Deprecated || realization.effect != Effect::Pure {
            continue;
        }
        let Some(sym) = realization.target.as_symbol() else {
            continue;
        };
        if sym == exclude {
            continue;
        }

        let arities: Vec<usize> = match &realization.spec {
            RealizationSpec::Native { native } => match registry.get(native) {
                Some(entry) if entry.effect == Effect::Pure => (1..=MAX_ARITY)
                    .filter(|k| entry.arity.accepts(*k))
                    .collect(),
                _ => continue,
            },
            RealizationSpec::Composed { body } => match max_hole(body) {
                Some(highest) => {
                    let arity = highest.as_u32() as usize + 1;
                    if arity > MAX_ARITY {
                        continue;
                    }
                    vec![arity]
                }
                // A body with no holes ignores its arguments, so calling it is
                // never more useful than naming the value it produces.
                None => continue,
            },
            _ => continue,
        };

        for arity in arities {
            if seen.insert((sym, arity)) {
                out.push(Operator {
                    head: Concept::symbol(sym),
                    arity,
                    label: names
                        .get(&sym)
                        .cloned()
                        .unwrap_or_else(|| format!("{:016x}", sym.as_u64())),
                });
            }
        }
    }

    out.sort_by(|a, b| a.label.cmp(&b.label).then(a.arity.cmp(&b.arity)));
    Ok(out)
}

/// The size-1 vocabulary.
///
/// Holes first, because a body that ignores its arguments is rarely the one
/// wanted and the first match at a given size wins. Then every atomic concept
/// appearing anywhere in the examples, then a stock pool. Constants drawn from
/// the examples are not a nicety: a program that needs `10` can never be found
/// if `10` is not sitting in the bank, and the examples are the only place the
/// search can learn that `10` matters.
pub(super) fn terminals(problem: &Problem) -> Vec<Concept> {
    let mut out: Vec<Concept> = Vec::new();
    let mut seen: HashSet<ContentId> = HashSet::new();

    for index in 0..problem.arity {
        let hole = Concept::hole(index as u32);
        if seen.insert(hole.content_id()) {
            out.push(hole);
        }
    }

    for inputs in &problem.inputs {
        for concept in inputs {
            collect_atomics(concept, &mut seen, &mut out);
        }
    }
    // Outputs are drawn from too. A target that always answers with the same
    // concept is spelled by that concept and by nothing else, and an output
    // often carries the very constant the body needs.
    for output in &problem.outputs {
        collect_atomics(output, &mut seen, &mut out);
    }

    for stock in [
        Concept::int(0),
        Concept::int(1),
        Concept::bool(true),
        Concept::bool(false),
        Concept::text(""),
    ] {
        if seen.insert(stock.content_id()) {
            out.push(stock);
        }
    }

    out
}

fn collect_atomics(concept: &Concept, seen: &mut HashSet<ContentId>, out: &mut Vec<Concept>) {
    for node in pre_order(concept) {
        if node.is_atomic() && seen.insert(node.content_id()) {
            out.push(node.clone());
        }
    }
}
