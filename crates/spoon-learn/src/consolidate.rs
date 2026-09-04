//! Naming what Spoon keeps rebuilding.
//!
//! Synthesis grows the library. Nothing shrinks it. Left alone, a brain that
//! learns twenty ways to say "add one to twice a thing" stores twenty bodies
//! that share a shape and knows about none of the sharing. This pass is the
//! one that notices: it finds the shape that keeps recurring, works out
//! whether naming it actually pays, gives it a name a human can read, and
//! rewrites the bodies to call it. That is the difference between a library
//! that compresses and one that only accumulates.
//!
//! The algorithm is Stitch's, over Concepts instead of programs (see
//! `docs/PRIOR_ART_MEMO.md` section 5). Anti-unification does the hard part:
//! given several instances of a shape it computes the most specific pattern
//! covering all of them, with holes exactly where they disagree. Everything
//! else here is deciding which of those patterns deserve a name.
//!
//! # The failure mode this file is mostly about
//!
//! Anti-unification always succeeds. Two terms with nothing in common
//! generalize to a bare hole, and a hole covers the universe. An abstraction
//! like that is not a compression win, it is a claim that two unrelated
//! procedures are the same procedure, and Spoon will then transfer evidence
//! between them. That is harmful transfer, and it is worse than never having
//! consolidated at all, because a missing abstraction costs storage while a
//! wrong one costs correctness. So there are two independent filters, and a
//! candidate has to clear both: [`ConsolidateConfig::min_concrete_ratio`]
//! refuses shapes that are mostly hole, and [`ConsolidateConfig::min_utility`]
//! refuses shapes that recur but pay for nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use chrono::{Duration, Utc};
use spoon_concept::{
    Activation, Concept, ConceptId, ConceptMeta, Effect, HoleId, Path, Provenance, Realization,
    RealizationKind, RealizationSpec, SymbolTable, Tier, anti_unify, generalizes, hole_count,
    holes, positions, pre_order, replace_at, substitute,
};
use spoon_store::{Result, Store};

/// Head symbols folded into a derived name before it stops being readable.
const MAX_NAME_PARTS: usize = 3;

/// Nesting past which a term is left alone rather than handed to the recursive
/// structural ops. See [`ConsolidateConfig::max_body_depth`].
const DEFAULT_MAX_DEPTH: usize = 64;

/// What counts as worth naming.
///
/// Every threshold here is a judgement call about a tradeoff, so none of them
/// is hidden inside the algorithm. The defaults are tuned for bodies the size
/// the synthesizer actually produces.
#[derive(Debug, Clone)]
pub struct ConsolidateConfig {
    /// How many occurrences a shape needs before it is a pattern rather than a
    /// coincidence. Two is a coincidence often enough to matter, so the
    /// default is three.
    pub min_count: usize,

    /// Utility a candidate must exceed. Zero means "must save something".
    pub min_utility: f64,

    /// Fraction of the pattern's nodes that must be concrete rather than
    /// holes.
    ///
    /// This is the over-generalization guard, and it is the one that stops
    /// harmful transfer. A pattern that is mostly holes covers almost every
    /// term of its shape, so naming it asserts a kinship between procedures
    /// that merely happen to have the same arity. `Add<Mul<?0, 2>, 1>` is
    /// six-sevenths concrete and says something. `Pair<?0, ?1>` is half
    /// concrete and says only "two of something". `?0<?1, ?2>` says nothing at
    /// all. The default of 0.6 sits just above the "head applied to all
    /// leaves" case, which is where meaning stops.
    pub min_concrete_ratio: f64,

    /// Largest subterm considered as a candidate. Bigger shapes recur rarely
    /// and cost quadratic work to group, so they are not worth chasing.
    pub max_pattern_nodes: usize,

    /// Bodies larger than this are left alone. Candidate collection sizes
    /// every subterm, which is quadratic in the body, and an enormous body is
    /// not where the reusable shapes are.
    pub max_body_nodes: usize,

    /// Bodies deeper than this are left alone.
    ///
    /// The structural ops in `spoon-concept` (`anti_unify`, `generalizes`,
    /// `content_id`, `positions`) all recurse on the term. Screening depth
    /// once, iteratively, up front is what lets every call after it be safe on
    /// input Spoon did not construct itself.
    pub max_body_depth: usize,

    /// Names for the symbols in the corpus. Without it an abstraction cannot
    /// be called anything better than its digest, and every name in it is
    /// already taken as far as collision checking is concerned.
    pub names: Option<Arc<SymbolTable>>,
}

impl Default for ConsolidateConfig {
    fn default() -> Self {
        ConsolidateConfig {
            min_count: 3,
            min_utility: 0.0,
            min_concrete_ratio: 0.6,
            max_pattern_nodes: 64,
            max_body_nodes: 4096,
            max_body_depth: DEFAULT_MAX_DEPTH,
            names: None,
        }
    }
}

/// A shape worth naming, with the evidence that says so.
#[derive(Debug, Clone, PartialEq)]
pub struct Abstraction {
    /// The generalized body, with holes where the instances differed.
    ///
    /// Holes are renumbered from zero in first-appearance order, because a
    /// `Composed` realization binds holes positionally: `Hole(0)` has to be
    /// the first argument of the call or the rewrite means something else.
    pub body: Concept,
    /// How many arguments a call takes: the number of distinct holes.
    pub arity: usize,
    /// Occurrences across the corpus, counting repeats inside one body. Three
    /// uses in one procedure is as real a recurrence as one use in three.
    pub instances: usize,
    /// Nodes saved across every occurrence, minus what the definition costs.
    pub utility: f64,
    /// Readable kebab-case name, unique against every name already known.
    pub name: String,
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// Find the shapes worth naming in a corpus of realization bodies.
///
/// Returns them best-first and non-overlapping: no two abstractions claim the
/// same region of the same body, so they can all be applied to the corpus
/// without one undoing another.
pub fn consolidate(bodies: &[Concept], config: ConsolidateConfig) -> Vec<Abstraction> {
    let mut candidates: Vec<Candidate> = group(bodies, &config)
        .into_values()
        .filter_map(|sites| build(sites, &config))
        .collect();

    // Best first. Every tiebreak is a total order over deterministic values,
    // because two runs over the same corpus must produce the same library or
    // the store diverges from itself between restarts.
    candidates.sort_by(|a, b| {
        b.utility
            .partial_cmp(&a.utility)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.instances.cmp(&a.instances))
            .then_with(|| b.pattern.size().cmp(&a.pattern.size()))
            .then_with(|| a.pattern.content_id().cmp(&b.pattern.content_id()))
    });

    // Greedy, non-overlapping. A candidate that shares a region with one
    // already taken is dropped whole rather than shrunk: keeping the losing
    // half of an overlap would mean an abstraction whose recorded instance
    // count no longer matches where it can actually be applied.
    let mut claimed: Vec<(usize, Path)> = Vec::new();
    let mut chosen: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        let clashes = candidate.sites.iter().any(|(body, path)| {
            claimed
                .iter()
                .any(|(other_body, other)| other_body == body && overlaps(path, other))
        });
        if clashes {
            continue;
        }
        claimed.extend(candidate.sites.iter().cloned());
        chosen.push(candidate);
    }

    let names = config.names.as_deref();
    let mut taken = known_names(names);
    chosen
        .into_iter()
        .map(|candidate| Abstraction {
            arity: holes(&candidate.pattern).len(),
            name: unique_name(base_name(&candidate.pattern, names), &mut taken),
            instances: candidate.instances,
            utility: candidate.utility,
            body: candidate.pattern,
        })
        .collect()
}

/// One shape and everywhere it was seen.
struct Candidate {
    pattern: Concept,
    /// Where the occurrences live, as (body index, path).
    sites: Vec<(usize, Path)>,
    instances: usize,
    utility: f64,
}

/// A candidate occurrence before it has been grouped: which body, where in it,
/// and the subterm itself.
type Occurrence = (usize, Path, Concept);

/// Bucket every compound subterm of every body by its shape.
///
/// The key deliberately forgets which constants appear, so `Add<1, 2>` and
/// `Add<3, 4>` land together and anti-unification gets a chance to find what
/// they share. It keeps head symbols, because the head is what a compound
/// *does*: grouping `Add<_, _>` with `Sub<_, _>` would only produce candidates
/// the over-generalization guard has to throw away again.
///
/// Leaves are never candidates. Naming a single atom buys nothing: the
/// definition costs more than the atom it replaces, every time.
fn group(bodies: &[Concept], config: &ConsolidateConfig) -> BTreeMap<String, Vec<Occurrence>> {
    let mut groups: BTreeMap<String, Vec<Occurrence>> = BTreeMap::new();
    for (index, body) in bodies.iter().enumerate() {
        if !depth_at_most(body, config.max_body_depth) {
            continue;
        }
        if pre_order(body).count() > config.max_body_nodes {
            continue;
        }
        for (path, node) in positions(body) {
            if !node.is_compound() || node.size() > config.max_pattern_nodes {
                continue;
            }
            let mut key = String::new();
            write_shape(node, &mut key);
            groups
                .entry(key)
                .or_default()
                .push((index, path, node.clone()));
        }
    }
    groups
}

/// Generalize one bucket and decide whether the result earns a name.
fn build(sites: Vec<Occurrence>, config: &ConsolidateConfig) -> Option<Candidate> {
    if sites.len() < config.min_count.max(1) {
        return None;
    }

    // Left fold. `anti_unify` maps a repeated disagreement to a repeated hole,
    // and it treats the accumulator's own holes as ordinary terms, so folding
    // preserves shared structure across all N instances rather than only the
    // first two: `Pair<7, 7>`, `Pair<9, 9>`, `Pair<4, 4>` still generalize to
    // `Pair<?0, ?0>` and not to two independent holes.
    let mut pattern = sites[0].2.clone();
    for (_, _, instance) in sites.iter().skip(1) {
        pattern = anti_unify(&pattern, instance).0;
    }
    let pattern = canonical_holes(&pattern);

    if over_general(&pattern, config.min_concrete_ratio) {
        return None;
    }
    let utility = utility(&pattern, sites.iter().map(|(_, _, term)| term));
    if utility <= config.min_utility {
        return None;
    }

    Some(Candidate {
        instances: sites.len(),
        sites: sites
            .into_iter()
            .map(|(body, path, _)| (body, path))
            .collect(),
        pattern,
        utility,
    })
}

/// Is this pattern too generic to mean anything?
///
/// See [`ConsolidateConfig::min_concrete_ratio`]. A non-compound pattern is
/// rejected outright: a bare hole covers every term there is, and a bare atom
/// is a leaf we should never have considered.
fn over_general(pattern: &Concept, min_ratio: f64) -> bool {
    if !pattern.is_compound() {
        return true;
    }
    let total = pattern.size();
    let concrete = total.saturating_sub(hole_count(pattern));
    (concrete as f64) / (total as f64) < min_ratio
}

/// Nodes saved across every occurrence, minus what the definition costs.
///
/// At each site the instance is replaced by a call, which costs two nodes for
/// the application and its head plus one copy of each argument. A hole that
/// appears twice in the pattern therefore pays twice and is charged once,
/// which is why shared holes are worth preserving and not just tidy.
///
/// The definition is charged its own size once. An abstraction used three
/// times that saves one node each has saved three nodes and cost however big
/// it is, which is a loss, and this is the arithmetic that says so.
fn utility<'a>(pattern: &Concept, instances: impl Iterator<Item = &'a Concept>) -> f64 {
    let params: Vec<HoleId> = holes(pattern).into_iter().collect();
    let mut saved: i64 = 0;
    for instance in instances {
        let Some(bindings) = generalizes(pattern, instance) else {
            // The pattern came from anti-unifying these instances, so it
            // covers all of them. Skipping rather than asserting keeps a
            // surprise here from being a panic.
            continue;
        };
        let args: usize = params
            .iter()
            .map(|hole| bindings.get(*hole).map_or(1, Concept::size))
            .sum();
        let call = 2 + args;
        saved += instance.size() as i64 - call as i64;
    }
    saved as f64 - pattern.size() as f64
}

// ---------------------------------------------------------------------------
// Rewriting
// ---------------------------------------------------------------------------

/// Rewrite `body` to call `abstraction` everywhere it matches.
///
/// `None` when the abstraction does not appear, so callers can tell "already
/// as compressed as it gets" from "compressed further" without comparing
/// terms.
///
/// Matches are taken outermost-first and never nested, so the arguments handed
/// to one call are never themselves rewritten into another call of the same
/// abstraction. Rewriting inside out would change what the arguments are.
pub fn apply_abstraction(body: &Concept, abstraction: &Abstraction) -> Option<Concept> {
    // A non-compound pattern matches everything, including the body itself,
    // and rewriting a term into a call that takes that term as its argument is
    // a loop dressed as progress.
    if !abstraction.body.is_compound() {
        return None;
    }
    if !depth_at_most(body, DEFAULT_MAX_DEPTH) {
        return None;
    }

    let params: Vec<HoleId> = holes(&abstraction.body).into_iter().collect();
    let head = Concept::named(&abstraction.name);

    let mut calls: Vec<(Path, Concept)> = Vec::new();
    for (path, node) in positions(body) {
        if calls.iter().any(|(taken, _)| overlaps(taken, &path)) {
            continue;
        }
        let Some(bindings) = generalizes(&abstraction.body, node) else {
            continue;
        };
        let args: Vec<Concept> = params
            .iter()
            .map(|hole| bindings.get(*hole).cloned().unwrap_or(Concept::Hole(*hole)))
            .collect();
        calls.push((path, Concept::apply(head.clone(), args)));
    }

    if calls.is_empty() {
        return None;
    }
    let mut rewritten = body.clone();
    for (path, call) in calls {
        rewritten = replace_at(&rewritten, &path, call)?;
    }
    Some(rewritten)
}

// ---------------------------------------------------------------------------
// The store-facing pass
// ---------------------------------------------------------------------------

/// Consolidate every composed realization in the store, in place.
///
/// Each accepted abstraction becomes a new `Composed` realization at
/// [`Tier::Consolidated`] with [`Provenance::Consolidated`], and every body it
/// covers is rewritten to call it. Rewrites reuse the source realization's
/// name, so its accumulated evidence follows the body rather than being reset
/// by the compression.
pub fn consolidate_store(store: &Store, config: ConsolidateConfig) -> Result<Vec<Abstraction>> {
    let sources: Vec<Realization> = store
        .realizations_by_kind(RealizationKind::Composed)?
        .into_iter()
        .filter(|r| r.tier != Tier::Deprecated)
        .collect();
    if sources.is_empty() {
        return Ok(Vec::new());
    }

    let mut config = config;
    if config.names.is_none() {
        config.names = Some(Arc::new(store.load_symbol_table()?));
    }

    let mut bodies: Vec<Concept> = sources
        .iter()
        .map(|r| match &r.spec {
            RealizationSpec::Composed { body } => body.clone(),
            // `realizations_by_kind` filtered on the denormalized kind column,
            // so this cannot fire unless the column and the spec disagree.
            _ => Concept::Hole(HoleId(0)),
        })
        .collect();

    let abstractions = consolidate(&bodies, config);
    if abstractions.is_empty() {
        return Ok(Vec::new());
    }

    let now = Utc::now();
    let mut dirty: BTreeSet<usize> = BTreeSet::new();

    for abstraction in &abstractions {
        let target = Concept::named(&abstraction.name);

        // Which bodies actually take the rewrite, and what authority they
        // needed. An abstraction lifted out of a body cannot require more
        // authority than the body it came from, so the join over its users is
        // a safe ceiling: it may over-declare, never under-declare.
        let mut effect = Effect::Pure;
        let mut users = 0usize;
        for (index, body) in bodies.iter_mut().enumerate() {
            // A realization must never be rewritten into a call to itself.
            if sources[index].target == target {
                continue;
            }
            let Some(next) = apply_abstraction(body, abstraction) else {
                continue;
            };
            *body = next;
            effect = effect.join(sources[index].effect);
            dirty.insert(index);
            users += 1;
        }
        if users == 0 {
            continue;
        }

        store.register_symbol(&abstraction.name)?;
        store.put_meta(
            &ConceptMeta::new(
                target.clone(),
                Provenance::Consolidated,
                Tier::Consolidated,
                now,
            )
            .with_surface_forms([abstraction.name.as_str()])
            .with_note(format!(
                "consolidated from {} occurrences across {users} realizations",
                abstraction.instances
            )),
        )?;
        store.put_realization(&Realization {
            target,
            name: format!("consolidated-{}", abstraction.name).into(),
            spec: RealizationSpec::Composed {
                body: abstraction.body.clone(),
            },
            effect,
            activation: Activation::new(now),
            provenance: Provenance::Consolidated,
            tier: Tier::Consolidated,
        })?;
    }

    for index in dirty {
        let mut updated = sources[index].clone();
        updated.spec = RealizationSpec::Composed {
            body: bodies[index].clone(),
        };
        store.put_realization(&updated)?;
    }

    Ok(abstractions)
}

/// Deprecate consolidated abstractions that never earned their keep.
///
/// An abstraction below `min_uses` after `min_age` was a bad guess: it
/// compressed the library on paper and nothing ever reached for it. Deprecated
/// rather than deleted, because [`Tier::Deprecated`] already means "never
/// selected" and the row is the only record that Spoon once believed this
/// shape mattered. That record is worth more than the bytes: it is what stops
/// the next pass from proposing the same abstraction and expecting a different
/// outcome.
///
/// Returns how many were deprecated.
pub fn evict_unused(store: &Store, min_uses: u64, min_age: Duration) -> Result<usize> {
    let now = Utc::now();
    let mut evicted = 0usize;
    for realization in store.all_realizations()? {
        if realization.provenance != Provenance::Consolidated
            || realization.tier != Tier::Consolidated
        {
            continue;
        }
        if realization.activation.uses >= min_uses {
            continue;
        }
        if now - realization.activation.created_at < min_age {
            continue;
        }
        let mut deprecated = realization;
        deprecated.tier = Tier::Deprecated;
        store.put_realization(&deprecated)?;
        evicted += 1;
    }
    Ok(evicted)
}

// ---------------------------------------------------------------------------
// Shape keys
// ---------------------------------------------------------------------------

/// Write the structural key: tree shape and head symbols, constants forgotten.
fn write_shape(term: &Concept, out: &mut String) {
    match term {
        Concept::Atomic(_) | Concept::Hole(_) => out.push('_'),
        Concept::Compound { head, args } => {
            out.push('(');
            write_head(head, out);
            for arg in args.iter() {
                out.push(' ');
                write_shape(arg, out);
            }
            out.push(')');
        }
    }
}

/// A named head is the operation being performed and is kept. A ground or hole
/// head is data in operator position and is forgotten like any other constant.
fn write_head(head: &Concept, out: &mut String) {
    match head {
        Concept::Atomic(ConceptId::Named(symbol)) => {
            let _ = write!(out, "#{:x}", symbol.0);
        }
        Concept::Compound { .. } => write_shape(head, out),
        _ => out.push('_'),
    }
}

// ---------------------------------------------------------------------------
// Small structural helpers
// ---------------------------------------------------------------------------

/// Renumber holes from zero in first-appearance order.
///
/// A `Composed` body binds holes positionally, so the numbering the fold
/// happened to produce is not a cosmetic detail: `Hole(0)` must be the first
/// argument or the call passes its arguments to the wrong places.
fn canonical_holes(term: &Concept) -> Concept {
    let mut order: Vec<HoleId> = Vec::new();
    for node in pre_order(term) {
        if let Some(hole) = node.as_hole()
            && !order.contains(&hole)
        {
            order.push(hole);
        }
    }
    if order
        .iter()
        .enumerate()
        .all(|(i, hole)| hole.0 as usize == i)
    {
        return term.clone();
    }
    let bindings = order
        .iter()
        .enumerate()
        .map(|(i, hole)| (*hole, Concept::hole(i as u32)))
        .collect();
    // Substitution is single-pass, so old and new numbering overlapping is
    // fine: an inserted hole is never revisited.
    substitute(term, &bindings)
}

/// Do these two positions inside one body cover overlapping ground?
///
/// Positions overlap exactly when one is an ancestor of the other, which as
/// paths means one is a prefix of the other.
fn overlaps(left: &Path, right: &Path) -> bool {
    let (shorter, longer) = if left.len() <= right.len() {
        (left.steps(), right.steps())
    } else {
        (right.steps(), left.steps())
    };
    longer.starts_with(shorter)
}

/// Depth check that does not recurse.
///
/// Everything downstream of candidate collection recurses on the term, so this
/// is the one place that has to be safe on a term Spoon did not build itself.
/// An explicit stack costs a few lines and removes stack overflow as an
/// outcome.
fn depth_at_most(term: &Concept, limit: usize) -> bool {
    let mut stack: Vec<(&Concept, usize)> = vec![(term, 1)];
    while let Some((node, depth)) = stack.pop() {
        if depth > limit {
            return false;
        }
        if let Concept::Compound { head, args } = node {
            stack.push((head, depth + 1));
            for arg in args.iter() {
                stack.push((arg, depth + 1));
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// Every name already spoken for.
///
/// The symbol table is the right source: it holds everything the brain has
/// ever named, so a fresh abstraction cannot shadow a native, a learned
/// concept, or an abstraction from an earlier pass. Both the registered
/// spelling and its kebab form go in, because a generated name is kebab and a
/// registered one may be `FriendWith`.
fn known_names(names: Option<&SymbolTable>) -> BTreeSet<String> {
    let mut taken = BTreeSet::new();
    if let Some(table) = names {
        for (_, name) in table.entries() {
            taken.insert(name.trim().to_lowercase());
            taken.insert(kebab(&name));
        }
    }
    taken
}

/// A name built from what the shape does, not from a counter.
///
/// `add-of-mul` tells a reader which two operations were fused. `abstraction-7`
/// tells them to go read the body. Heads are taken in pre-order, so the
/// outermost operation leads, and capped so a deeply nested shape does not
/// produce a sentence.
fn base_name(pattern: &Concept, names: Option<&SymbolTable>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(table) = names {
        for node in pre_order(pattern) {
            let Some(symbol) = node.head().and_then(Concept::as_symbol) else {
                continue;
            };
            let Some(raw) = table.resolve(symbol) else {
                continue;
            };
            let part = kebab(&raw);
            if part.is_empty() || parts.contains(&part) {
                continue;
            }
            parts.push(part);
            if parts.len() == MAX_NAME_PARTS {
                break;
            }
        }
    }
    if parts.is_empty() {
        // No table, or no named heads. A digest prefix is not readable, but it
        // is honest and it is stable, which beats inventing a word.
        return format!("shape-{}", &pattern.content_id().to_hex()[..6]);
    }
    parts.join("-of-")
}

/// First free name in the `base`, `base-2`, `base-3` series.
fn unique_name(base: String, taken: &mut BTreeSet<String>) -> String {
    let mut candidate = base.clone();
    let mut suffix = 1u32;
    while taken.contains(&candidate) {
        suffix += 1;
        candidate = format!("{base}-{suffix}");
    }
    taken.insert(candidate.clone());
    candidate
}

/// Lowercase, hyphen-separated, alphanumerics only. `FriendWith` becomes
/// `friend-with`, because the casing is a word boundary and dropping it would
/// glue the words together.
fn kebab(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    let mut pending_break = false;
    let mut prev_lower = false;
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            if (pending_break || (ch.is_uppercase() && prev_lower)) && !out.is_empty() {
                out.push('-');
            }
            out.extend(ch.to_lowercase());
            prev_lower = !ch.is_uppercase();
            pending_break = false;
        } else {
            pending_break = true;
            prev_lower = false;
        }
    }
    out
}
