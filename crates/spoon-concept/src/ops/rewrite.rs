//! Small builders that rebuild a term without throwing away its sharing.
//!
//! These exist because the obvious hand-written version of each one is a
//! deep copy. Written once here, they keep the head `Arc` and every untouched
//! argument intact.

use std::sync::Arc;

use crate::concept::Concept;

/// Rebuild a compound with each argument passed through `f`, keeping the head.
///
/// Non-compounds come back unchanged rather than being an error. An atomic
/// concept has no arguments, so mapping over them is a no-op, and making
/// callers special-case that would put the same `match` at every site.
pub fn map_args<F>(term: &Concept, mut f: F) -> Concept
where
    F: FnMut(&Concept) -> Concept,
{
    match term {
        Concept::Compound { head, args } => Concept::Compound {
            head: head.clone(),
            args: args.iter().map(&mut f).collect(),
        },
        _ => term.clone(),
    }
}

/// [`map_args`] for transformations that can fail.
///
/// Fails on the first error, so an evaluator mapping arguments through a
/// fallible step does not go on to evaluate the rest of a doomed call.
pub fn try_map_args<F, E>(term: &Concept, mut f: F) -> Result<Concept, E>
where
    F: FnMut(&Concept) -> Result<Concept, E>,
{
    match term {
        Concept::Compound { head, args } => {
            let mapped: Vec<Concept> = args.iter().map(&mut f).collect::<Result<_, E>>()?;
            Ok(Concept::Compound {
                head: head.clone(),
                args: Arc::from(mapped),
            })
        }
        _ => Ok(term.clone()),
    }
}

/// Peel nested compound heads into a single head and a flat argument list.
///
/// Partial application shows up as a compound whose head is itself a compound:
/// applying `f` to `a` and then that to `b` gives `((f a) b)`. Dispatch cares
/// about `f` and the arguments it ultimately received, not about how the
/// application was spelled, so this normalizes the spine: `((f a) b)` yields
/// `(f, [a, b])`.
///
/// Arguments come back in application order, outermost application last. A
/// non-compound is its own spine with no arguments.
pub fn flatten_spine(term: &Concept) -> (&Concept, Vec<&Concept>) {
    let mut layers: Vec<&[Concept]> = Vec::new();
    let mut cursor = term;
    while let Concept::Compound { head, args } = cursor {
        layers.push(args);
        cursor = head;
    }
    let total = layers.iter().map(|layer| layer.len()).sum();
    let mut flat = Vec::with_capacity(total);
    for layer in layers.iter().rev() {
        flat.extend(layer.iter());
    }
    (cursor, flat)
}
