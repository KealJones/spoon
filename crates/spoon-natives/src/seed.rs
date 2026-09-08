//! Putting the bootstrap concepts into a store.
//!
//! Registering a native makes the code reachable; it does not make the concept
//! exist. A brand new brain needs a `Native` realization stored for each
//! bootstrap concept, or the evaluator will find nothing to apply and treat
//! every call as data.

use chrono::Utc;
use spoon_concept::{
    Activation, Concept, ConceptMeta, NativeId, Provenance, Realization, RealizationSpec, Tier,
};
use spoon_eval::NativeRegistry;
use spoon_store::{Result, Store};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeedStats {
    pub concepts: usize,
    pub realizations: usize,
    /// Bootstrap natives that no longer exist in the binary and were dropped.
    pub retired: usize,
    /// Learned phrasings that pointed at one of those and were forgotten.
    pub forgotten: usize,
}

/// Store a `Native` realization for every registered native, naming the
/// concept after the native itself.
///
/// Idempotent: running it against an existing brain refreshes the bootstrap
/// realizations without disturbing anything learned since.
///
/// Refreshing is only half the job. A native that gets renamed or removed
/// leaves its realization behind in every brain that ever ran an older build,
/// pointing at a Rust function that is gone. The concept still looks realized,
/// so evaluation keeps picking it and keeps stalling, and the gap is never
/// reported because on paper the capability exists. Seeding retires those too.
pub fn seed_bootstrap(store: &Store, registry: &NativeRegistry) -> Result<SeedStats> {
    let now = Utc::now();
    let mut stats = SeedStats::default();

    for (id, entry) in registry.entries() {
        let name = id.as_str();
        let target = Concept::named(name);
        store.register_symbol(name)?;

        store.put_meta(
            &ConceptMeta::new(target.clone(), Provenance::Bootstrap, Tier::Kernel, now)
                .with_surface_forms([name])
                .with_note(entry.doc),
        )?;
        stats.concepts += 1;

        store.put_realization(&Realization {
            target,
            name: format!("native-{name}").into(),
            spec: RealizationSpec::Native {
                native: NativeId::new(name),
            },
            effect: entry.effect,
            activation: Activation::new(now),
            provenance: Provenance::Bootstrap,
            tier: Tier::Kernel,
        })?;
        stats.realizations += 1;
    }

    crate::source::seed(store)?;

    // Only bootstrap natives are ours to retire. A learned composed body or a
    // rule the Teacher wrote is not made stale by a Rust rename, and dropping
    // one would throw away the evidence behind it.
    let mut orphaned = Vec::new();
    for r in store.all_realizations()? {
        let RealizationSpec::Native { native } = &r.spec else {
            continue;
        };
        if r.provenance == Provenance::Bootstrap && !registry.contains(native) {
            store.retire_realization(&r.name)?;
            // Marked, not just deleted. A name that used to work leaks into
            // places a realization lookup cannot reach: the Teacher writes
            // durable advice about it, and that advice keeps steering the ears
            // back at a head that is gone. Anything holding a name needs to be
            // able to ask whether it still means something.
            store.assert_concept(
                &Concept::call("retired", [r.target.clone()]),
                Provenance::Bootstrap,
                None,
                None,
            )?;
            orphaned.push(r.target.clone());
            stats.retired += 1;
        }
    }

    // A phrasing that produces a dead head is worse than no phrasing at all.
    //
    // Retiring the realization is not enough on its own: the learned pair that
    // says "how many X in Y" reads as `count-matching<...>` outlives it, so
    // the ears keep confidently producing a head nothing can carry out. The
    // brain that had used the capability most was the one that could no longer
    // answer, while a fresh brain got it right, because the fresh one had no
    // phrasing to mislead it.
    //
    // Only pairs that mention a head that just died. A phrasing that still
    // resolves is none of this function's business.
    if !orphaned.is_empty() {
        for pair in store.all_pairs(usize::MAX)? {
            let mentions_dead = pair.steps.iter().any(|step| {
                spoon_concept::pre_order(step).any(|node| orphaned.iter().any(|o| node == o))
            });
            if mentions_dead {
                store.forget_pair(pair.id)?;
                stats.forgotten += 1;
            }
        }
    }

    // Broader check: phrasings referencing symbols the store has never heard
    // of. This catches stale phrasings left over from a rename that the
    // orphan check above missed (it only looks at heads that just died THIS
    // run, not symbols that were already dead from a previous rename).
    let report = store.purge_stale_pairs()?;
    stats.forgotten += report.removed as usize;
    let retired_realizations = store.purge_stale_realizations()?;
    stats.retired += retired_realizations as usize;

    Ok(stats)
}
