//! Choosing among competing realizations.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use spoon_concept::{Concept, Realization, Tier};

/// One candidate with the reasoning behind its rank, kept so the trace can
/// record not just what was chosen but what it beat.
#[derive(Debug, Clone)]
pub struct Scored {
    pub realization: Arc<Realization>,
    pub score: f64,
    pub success_rate: f64,
    pub context_fit: f64,
    pub activation_bonus: f64,
}

/// Neutral context fit, used when there is no evidence either way.
///
/// Absence of evidence is neutral, not negative. A realization nobody has
/// characterized yet should not be ranked below one actively known to be bad.
pub const NEUTRAL_FIT: f64 = 0.5;

/// How much one piece of contextual evidence moves the fit.
const FIT_STEP: f64 = 0.25;

/// Fit never reaches zero, so a realization is never made permanently
/// unreachable by evidence alone. Deprecating one is a deliberate act.
const MIN_FIT: f64 = 0.05;

/// Candidates within this fraction of the top score are considered
/// interchangeable for the purpose of exploration.
const EXPLORE_BAND: f64 = 0.5;

/// Probability of exploring instead of exploiting.
pub const DEFAULT_EPSILON: f64 = 0.05;

/// How close two scores must be to count as a tie rather than a ranking.
///
/// Generous enough to catch realizations that differ only by floating point,
/// which is what two fresh ones with identical history actually are.
pub const TIE_EPSILON: f64 = 1e-9;

/// Squash an ACT-R base level into `[0, 1)`.
///
/// Base levels are logarithms and can be negative, so they cannot be used as a
/// multiplier directly. The sigmoid keeps the ordering and bounds the effect,
/// so a heavily used realization gets a meaningful edge without activation
/// alone overwhelming a poor success rate.
fn squash(base_level: f64) -> f64 {
    1.0 / (1.0 + (-base_level).exp())
}

/// Score one realization.
///
/// `score = success_rate * context_fit * (1 + activation_bonus)`
///
/// Every factor is smoothed or floored so that a brand new realization scores
/// low but non-zero. Without that, the first realization to arrive would win
/// forever: it would be the only one ever selected, so it would be the only one
/// ever to accumulate evidence.
pub fn score(realization: Arc<Realization>, context_fit: f64, now: DateTime<Utc>) -> Scored {
    let success_rate = realization.activation.success_rate();
    let activation_bonus = realization
        .activation
        .base_level(now)
        .map(squash)
        .unwrap_or(0.0);
    let score = success_rate * context_fit * (1.0 + activation_bonus);
    Scored {
        realization,
        score,
        success_rate,
        context_fit,
        activation_bonus,
    }
}

/// Context fit from stored evidence.
///
/// `WorksWellWith<realization, X>` raises it when `X` is in the situation;
/// `WorksPoorlyWith` lowers it. Both are ordinary stored concepts, so this is
/// something Spoon learns rather than something hard-coded.
pub fn context_fit(evidence: &[(FitKind, Concept)], situation: &[Concept]) -> f64 {
    let mut fit = NEUTRAL_FIT;
    for (kind, subject) in evidence {
        if !situation.iter().any(|s| s == subject) {
            continue;
        }
        match kind {
            FitKind::Well => fit += FIT_STEP,
            FitKind::Poorly => fit -= FIT_STEP,
        }
    }
    fit.clamp(MIN_FIT, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitKind {
    Well,
    Poorly,
}

/// Rank candidates, best first.
///
/// Ties break on the realization name so that a deterministic run is
/// reproducible rather than dependent on store row order.
pub fn rank(mut scored: Vec<Scored>) -> Vec<Scored> {
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.realization.name.cmp(&b.realization.name))
    });
    scored
}

/// Deprecated realizations are excluded outright.
///
/// Kernel tier gets no scoring privilege in return: a learned realization that
/// performs better in context is supposed to win, which is the entire point of
/// keeping several around.
pub fn is_selectable(realization: &Realization) -> bool {
    realization.tier != Tier::Deprecated
}

/// Pick an index to try first, exploring occasionally.
///
/// Pure exploitation is self-defeating here. A realization that is never
/// selected never accumulates evidence, so it can never justify being
/// selected, and whichever arrived first wins permanently.
///
/// Exploration is suppressed for anything above a pure effect: trying an
/// unproven realization to learn from it is reasonable for a computation and
/// irresponsible for something that spends money or deletes data.
pub fn choose_first(ranked: &[Scored], explore: bool, rng: &mut Rng) -> usize {
    if ranked.len() < 2 || !explore {
        return 0;
    }

    // An exact tie is decided by a coin, not by the name.
    //
    // Ranking breaks ties alphabetically so a deterministic run reproduces, and
    // that is fine as an ordering and wrong as a choice: two realizations that
    // have never run score identically, so the one whose name sorts first would
    // take every turn but the rare exploratory one. A synthesized body and a
    // taught body of the same concept are exactly that case, and "synth" sorts
    // before "taught" for reasons that have nothing to do with which is better.
    //
    // Spreading the tie is what lets evidence accumulate on both, which is the
    // only thing that can separate them later.
    let tied = ranked
        .iter()
        .take_while(|s| (ranked[0].score - s.score).abs() < TIE_EPSILON)
        .count();
    if tied > 1 {
        return rng.below(tied);
    }

    if rng.next_f64() >= DEFAULT_EPSILON {
        return 0;
    }
    let top = ranked[0].score;
    let threshold = top * EXPLORE_BAND;
    let band = ranked
        .iter()
        .take_while(|s| s.score >= threshold)
        .count()
        .max(1);
    rng.below(band)
}

/// Small xorshift generator.
///
/// Exploration needs randomness but not cryptographic randomness, and pulling
/// in a dependency for one `f64` is not worth it. Seeded explicitly so a run
/// can be replayed.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn from_time(now: DateTime<Utc>) -> Self {
        Rng::new(now.timestamp_nanos_opt().unwrap_or(1) as u64)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}
