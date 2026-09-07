//! Sets: lists, deduplicated, with no order of their own.
//!
//! There is no separate set concept. A set is a `List<..>` that happens to
//! hold no duplicate, the same way `list-unique` already produces. Keeping
//! sets list-backed means every list native already works on one; this module
//! adds the operations that only make sense once duplicates are gone.
//!
//! First-appearance order is kept throughout, matching `list-unique`, so a set
//! built from the same input twice always looks the same rather than
//! reordering based on a hash.

use spoon_concept::Concept;
use spoon_eval::{Arity, Ctx, EvalResult, NativeRegistry};

use crate::collections::{make_list, want_list};

fn dedup(items: &[Concept]) -> Vec<Concept> {
    let mut out: Vec<Concept> = Vec::new();
    for item in items {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
    out
}

fn set_of(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let items = want_list("set-set", &args[0])?;
    Ok(make_list(dedup(&items)))
}

fn union(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let mut items = want_list("set-union", &args[0])?;
    items.extend(want_list("set-union", &args[1])?);
    Ok(make_list(dedup(&items)))
}

fn intersection(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let a = want_list("set-intersection", &args[0])?;
    let b = want_list("set-intersection", &args[1])?;
    let out: Vec<Concept> = dedup(&a).into_iter().filter(|x| b.contains(x)).collect();
    Ok(make_list(out))
}

fn difference(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let a = want_list("set-difference", &args[0])?;
    let b = want_list("set-difference", &args[1])?;
    let out: Vec<Concept> = dedup(&a).into_iter().filter(|x| !b.contains(x)).collect();
    Ok(make_list(out))
}

fn symmetric_difference(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let a = want_list("set-symmetric-difference", &args[0])?;
    let b = want_list("set-symmetric-difference", &args[1])?;
    let mut out: Vec<Concept> = dedup(&a).into_iter().filter(|x| !b.contains(x)).collect();
    out.extend(dedup(&b).into_iter().filter(|x| !a.contains(x)));
    Ok(make_list(out))
}

fn is_subset(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let a = want_list("set-is-subset", &args[0])?;
    let b = want_list("set-is-subset", &args[1])?;
    Ok(Concept::bool(a.iter().all(|x| b.contains(x))))
}

fn is_superset(_ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let a = want_list("set-is-superset", &args[0])?;
    let b = want_list("set-is-superset", &args[1])?;
    Ok(Concept::bool(b.iter().all(|x| a.contains(x))))
}

pub fn register(registry: &mut NativeRegistry) {
    registry.pure(
        "set-set",
        set_of,
        Arity::Exact(1),
        "a list with duplicates removed, keeping first appearances",
    );
    registry.pure(
        "set-union",
        union,
        Arity::Exact(2),
        "the elements of either of two sets, deduplicated",
    );
    registry.pure(
        "set-intersection",
        intersection,
        Arity::Exact(2),
        "the elements common to both of two sets",
    );
    registry.pure(
        "set-difference",
        difference,
        Arity::Exact(2),
        "the elements of the first set that are not in the second",
    );
    registry.pure(
        "set-symmetric-difference",
        symmetric_difference,
        Arity::Exact(2),
        "the elements in exactly one of two sets",
    );
    registry.pure(
        "set-is-subset",
        is_subset,
        Arity::Exact(2),
        "whether every element of the first set is in the second",
    );
    registry.pure(
        "set-is-superset",
        is_superset,
        Arity::Exact(2),
        "whether every element of the second set is in the first",
    );
}
