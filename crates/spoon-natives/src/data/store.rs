//! Belief and recall.
//!
//! These are the natives that make the store reachable from inside an
//! evaluation. `Assert<Owns<Greg, Dog>>` records a fact and
//! `Recall<Owns<Hole(0), Dog>>` asks who owns one.
//!
//! # Why every recall is bounded
//!
//! The pattern filter runs in this process, so the store hands back candidate
//! rows and we discard the ones that do not match. Two limits fall out of
//! that. [`SCAN_LIMIT`] caps how many rows are read; [`DEFAULT_LIMIT`] caps how
//! many matches come back when the caller does not say. An unbounded query
//! against a large brain is a hang, and a hang inside evaluation is
//! indistinguishable from a crash to whoever is waiting on it.
//!
//! # Why the results are sorted
//!
//! Every recall sorts by content id before truncating. The store's own row
//! order is by insertion time, which is stable but says nothing about the
//! query, so two brains holding the same facts in a different order would
//! answer the same question differently. Sorting on the structural digest
//! makes a result depend only on which concepts matched.

use spoon_concept::{Concept, Effect, Provenance, SymbolId, generalizes, hole_count, pre_order};
use spoon_eval::{
    ArgStrategy, Arity, Ctx, EvalError, EvalResult, NativeRegistry, native_error, type_error,
};

use super::{list_of, want_int, want_text};

/// How many results a recall returns when the caller does not ask for a
/// specific number.
pub const DEFAULT_LIMIT: usize = 100;

/// How many stored rows a recall reads before it stops looking. Also the
/// ceiling on an explicit limit: a caller cannot ask for more results than the
/// scan can produce.
pub const SCAN_LIMIT: usize = 10_000;

fn exists(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    Ok(Concept::bool(ctx.store().holds(&args[0])?))
}

fn assert_it(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    ctx.store()
        .assert_concept(&args[0], Provenance::Inferred, None, None)?;
    Ok(args[0].clone())
}

/// Stop believing a concept, and say how many beliefs that withdrew.
///
/// Nothing is deleted. Each live assertion gets an `invalidated_at` of the
/// context clock, so a question about an earlier instant still sees what was
/// believed then. Returning the count rather than the concept is what lets a
/// caller tell "withdrawn" from "there was nothing to withdraw", which a bare
/// echo of the input would hide.
fn retract_claim(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let now = ctx.now();
    let live = ctx.store().live_assertions(&args[0])?;
    let mut withdrawn = 0i64;
    for record in live {
        if ctx.store().retract(record.id, now)? {
            withdrawn += 1;
        }
    }
    Ok(Concept::int(withdrawn))
}

/// Find stored concepts matching a pattern that may contain holes.
///
/// `Recall<Owns<Hole(0), Dog>>` is how Spoon answers "who owns a dog". The
/// pattern is matched with [`generalizes`], so a hole stands for anything and
/// everything else has to line up exactly.
fn recall(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let limit = want_limit("recall", args.get(1))?;
    Ok(list_of(search(ctx, "recall", &args[0], limit)?))
}

/// How many stored concepts match a pattern.
///
/// Capped at [`SCAN_LIMIT`] like every other query here, so a count that comes
/// back equal to the cap means "at least this many" rather than "exactly this
/// many". Reporting a number the scan cannot stand behind would be worse than
/// reporting a bounded one.
fn count_recalled(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let found = search(ctx, "count-recalled", &args[0], SCAN_LIMIT)?;
    Ok(Concept::int(found.len() as i64))
}

/// Everything stored that mentions this concept anywhere inside it.
///
/// "Find everything about Greg." Nested mentions count: `Stated<Keal,
/// IsSad<Greg>>` is found by asking about Greg, not only by asking about
/// `IsSad<Greg>`.
fn recall_about(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let limit = want_limit("recall-about", args.get(1))?;
    let found = ctx
        .store()
        .concepts_containing(args[0].content_id(), SCAN_LIMIT)?;
    Ok(list_of(order_and_trim(found, limit)))
}

/// Every stored compound filed under a head. "Find all friendships."
///
/// The head can be given as text or as the named concept itself, because both
/// spellings turn into the same symbol and refusing one of them would only
/// make the caller convert.
fn recall_by_head(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let head = want_symbol("recall-by-head", &args[0])?;
    let limit = want_limit("recall-by-head", args.get(1))?;
    let found = ctx.store().concepts_by_head(head, SCAN_LIMIT)?;
    Ok(list_of(order_and_trim(found, limit)))
}

/// The surface forms a concept is stored under, preferred first.
///
/// An undescribed concept gives an empty list rather than an error: having no
/// name recorded is an ordinary state for a concept that was only ever
/// asserted, and a caller asking how to say something can cope with "no idea"
/// far better than with a failure.
fn describe(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let forms = match ctx.store().get_meta(&args[0])? {
        Some(meta) => meta.surface_forms.iter().map(Concept::text).collect(),
        None => Vec::new(),
    };
    Ok(list_of(forms))
}

/// The concepts that go by a surface form. The reverse of [`describe`].
///
/// Several concepts can share a form, and that ambiguity is real: "bank" is
/// two concepts. The list comes back whole so the caller can resolve it with
/// context instead of the store guessing.
fn surface_of(ctx: &mut dyn Ctx, args: &[Concept]) -> EvalResult {
    let form = want_text("surface-of", &args[0])?;
    let limit = want_limit("surface-of", args.get(1))?;
    let found = ctx.store().surface_lookup(form)?;
    Ok(list_of(order_and_trim(found, limit)))
}

// ---- shared machinery ----

/// Match a pattern against the store.
///
/// The candidate set is narrowed before matching, because scanning every row
/// to filter it in memory would not survive a real brain:
///
/// - No holes: the pattern is a concept, so ask whether that exact concept is
///   stored.
/// - A named head: ask the head index. `Owns<Hole(0), Dog>` reads the
///   ownership rows and nothing else.
/// - Otherwise, anchor on the first concrete subterm and ask the participant
///   index.
/// - A pattern that is nothing but holes matches everything, which is a
///   request to read the whole brain. That is refused by name rather than
///   answered slowly.
fn search(
    ctx: &mut dyn Ctx,
    native: &str,
    pattern: &Concept,
    limit: usize,
) -> Result<Vec<Concept>, EvalError> {
    let store = ctx.store();
    if hole_count(pattern) == 0 {
        let stored = store.has_concept(pattern.content_id())?;
        return Ok(if stored && limit > 0 {
            vec![pattern.clone()]
        } else {
            Vec::new()
        });
    }

    let candidates = if let Some(head) = pattern.head_symbol() {
        store.concepts_by_head(head, SCAN_LIMIT)?
    } else if let Some(anchor) = anchor(pattern) {
        store.concepts_containing(anchor.content_id(), SCAN_LIMIT)?
    } else {
        return Err(native_error(
            native,
            "this pattern is all holes and would match the whole brain: give it a head or a concrete argument",
        ));
    };

    let matched = candidates
        .into_iter()
        .filter(|c| generalizes(pattern, c).is_some())
        .collect();
    Ok(order_and_trim(matched, limit))
}

/// The most specific concrete subterm to search on, skipping the pattern
/// itself. Pre-order means the largest one wins, which is the narrowest index
/// lookup available.
fn anchor(pattern: &Concept) -> Option<&Concept> {
    pre_order(pattern).skip(1).find(|node| !node.is_hole())
}

/// Deterministic order, then the caller's cap. Duplicates are dropped:
/// the participant index can name the same concept through more than one
/// position.
fn order_and_trim(mut found: Vec<Concept>, limit: usize) -> Vec<Concept> {
    found.sort_by_key(|c| c.content_id());
    found.dedup();
    found.truncate(limit);
    found
}

/// Read an optional limit argument, defaulting to [`DEFAULT_LIMIT`] and
/// clamped to [`SCAN_LIMIT`].
fn want_limit(native: &str, arg: Option<&Concept>) -> Result<usize, EvalError> {
    let Some(arg) = arg else {
        return Ok(DEFAULT_LIMIT);
    };
    let raw = want_int(native, arg)?;
    if raw <= 0 {
        return Err(native_error(
            native,
            format!("a limit of {raw} asks for nothing; pass a positive count"),
        ));
    }
    Ok((raw as usize).min(SCAN_LIMIT))
}

/// A head symbol, written either as text or as the named concept itself.
fn want_symbol(native: &str, c: &Concept) -> Result<SymbolId, EvalError> {
    if let Some(symbol) = c.as_symbol() {
        return Ok(symbol);
    }
    match c.as_ground().and_then(spoon_concept::Ground::as_str) {
        Some(name) => Ok(SymbolId::of(name)),
        None => Err(type_error(native, "a name, as text or a named concept", c)),
    }
}

pub fn register(registry: &mut NativeRegistry) {
    registry.register(
        "exists",
        exists,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Read,
        "whether the store actively asserts a concept",
    );
    registry.register(
        "assert",
        assert_it,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Write,
        "assert a concept into the store",
    );
    registry.register(
        "retract-claim",
        retract_claim,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Write,
        "stop believing a concept, returning how many assertions were withdrawn",
    );
    registry.register(
        "recall",
        recall,
        Arity::Between(1, 2),
        ArgStrategy::Eager,
        Effect::Read,
        "stored concepts matching a pattern, whose holes match anything",
    );
    registry.register(
        "recall-about",
        recall_about,
        Arity::Between(1, 2),
        ArgStrategy::Eager,
        Effect::Read,
        "stored concepts that mention this one anywhere inside them",
    );
    registry.register(
        "recall-by-head",
        recall_by_head,
        Arity::Between(1, 2),
        ArgStrategy::Eager,
        Effect::Read,
        "stored compounds filed under a head name",
    );
    registry.register(
        "describe",
        describe,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Read,
        "the surface forms a concept is stored under, preferred first",
    );
    registry.register(
        "surface-of",
        surface_of,
        Arity::Between(1, 2),
        ArgStrategy::Eager,
        Effect::Read,
        "the concepts that go by a surface form",
    );
    registry.register(
        "count-recalled",
        count_recalled,
        Arity::Exact(1),
        ArgStrategy::Eager,
        Effect::Read,
        "how many concepts in memory match a pattern (not a count of list elements)",
    );
}
