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

    // Only bootstrap natives are ours to retire. A learned composed body or a
    // rule the Teacher wrote is not made stale by a Rust rename, and dropping
    // one would throw away the evidence behind it.
    for r in store.all_realizations()? {
        let RealizationSpec::Native { native } = &r.spec else {
            continue;
        };
        if r.provenance == Provenance::Bootstrap && !registry.contains(native) {
            store.retire_realization(&r.name)?;
            stats.retired += 1;
        }
    }

    Ok(stats)
}
