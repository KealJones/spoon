//! Choosing among competing realizations.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use spoon_concept::{Activation, Concept, Realization, Tier};

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

/// How close two scores must be to count as a tie rather than a ranking.
///
/// Generous enough to catch realizations that differ only by floating point,
/// which is what two fresh ones with identical history actually are.
pub const TIE_EPSILON: f64 = 1e-9;

/// Draw a plausible success rate from what has actually been observed.
///
/// Thompson sampling. Each realization's success rate is a Beta posterior over
/// its own record: `Beta(successes + 1, failures + 1)`, which starts uniform
/// when nothing is known and tightens as evidence arrives. Selection samples
/// from each posterior and takes the highest draw.
///
/// This replaces an exploration rate and an optimism bonus, both of which were
/// numbers picked by hand to paper over the same problem. A realization that
/// has never run has a flat posterior, so it draws high often enough to get
/// tried without being handed a turn it did not earn. One proven over fifty
/// uses has a posterior concentrated near its rate and rarely draws low enough
/// to lose. Nothing needs tuning: the width of each belief is the uncertainty,
/// and the uncertainty is what decides how much to gamble.
///
/// A newcomer arriving against an incumbent at 0.9 gets tried roughly a tenth
/// of the time at first, and either climbs or stops being drawn. That is the
/// answer to "how does anything ever displace an established realization",
/// and it falls out of the statistics rather than being legislated.
fn sample_belief(activation: &Activation, rng: &mut Rng) -> f64 {
    let wins = activation.successes + 1;
    let losses = activation.failures + 1;
    let a = sample_gamma(wins, rng);
    let b = sample_gamma(losses, rng);
    if a + b <= 0.0 { 0.5 } else { a / (a + b) }
}

/// A draw from `Gamma(shape, 1)` for a whole-number shape.
///
/// The sum of `shape` exponential draws, which is exact for integer shape and
/// needs no special functions. Summing logs rather than multiplying uniforms
/// avoids underflow once the counts get large.
///
/// Capped because the cost is linear in the count and the payoff is not: past a
/// few hundred observations the posterior is tight enough that further
/// narrowing changes no decision, so the cap costs accuracy nobody can use.
fn sample_gamma(shape: u64, rng: &mut Rng) -> f64 {
    const CAP: u64 = 256;
    let draws = shape.min(CAP);
    let mut total = 0.0;
    for _ in 0..draws {
        // Guarded away from zero: ln(0) is negative infinity.
        let u = rng.next_f64().max(f64::MIN_POSITIVE);
        total -= u.ln();
    }
    // Scale back up when the count was capped, so the mean stays right.
    total * (shape as f64 / draws as f64)
}

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
/// The belief term is a draw from the posterior rather than its mean, which is
/// what lets an unproven realization ever be chosen. Scoring by the mean alone
/// makes the first realization to arrive win forever: it is the only one
/// selected, so it is the only one that accumulates evidence, so it stays the
/// only one selected.
///
/// Pass `None` for the mean instead of a draw, which is what a deterministic
/// run and the inspector both want.
pub fn score(
    realization: Arc<Realization>,
    context_fit: f64,
    now: DateTime<Utc>,
    rng: Option<&mut Rng>,
) -> Scored {
    let success_rate = realization.activation.success_rate();
    let activation_bonus = realization
        .activation
        .base_level(now)
        .map(squash)
        .unwrap_or(0.0);
    let belief = match rng {
        Some(rng) => sample_belief(&realization.activation, rng),
        None => success_rate,
    };
    // Activation is deliberately NOT a factor here.
    //
    // It measures how recently and often something has been used, and using it
    // to choose double-counts: how often a realization has run is already what
    // makes its posterior tight, and multiplying by it again hands the
    // incumbent up to twice the score for no evidence it is better. That single
    // term was enough to keep an untried realization at exactly zero percent of
    // turns even with sampling in place.
    //
    // It stays on `Scored` because it is the right measure elsewhere: ranking
    // which concepts to show the ears is a recency question, and choosing
    // between two ways of doing one thing is not.
    let score = belief * context_fit;
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

/// Which candidate to try first.
///
/// Almost always the top of the ranking, because with Thompson sampling the
/// ranking already is the exploration: each score came from a draw against that
/// realization's own posterior, so an uncertain one rises on its own a share of
/// the time proportional to how uncertain it is. An exploration rate layered on
/// top would be exploring twice, and the epsilon it needs is exactly the
/// hand-tuned number sampling exists to remove.
///
/// The one thing sampling does not settle is a deterministic run, where scores
/// are posterior means and two realizations with identical records genuinely
/// tie. Settling that by name would hand every turn to whichever sorts first.
pub fn choose_first(ranked: &[Scored], explore: bool, rng: &mut Rng) -> usize {
    if ranked.len() < 2 || !explore {
        return 0;
    }
    let tied = ranked
        .iter()
        .take_while(|s| (ranked[0].score - s.score).abs() < TIE_EPSILON)
        .count();
    if tied > 1 { rng.below(tied) } else { 0 }
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
