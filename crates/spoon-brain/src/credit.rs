//! Working out which stage produced a wrong answer.
//!
//! A turn passes through interpretation, name reconciliation, retrieval,
//! realization selection, execution, and phrasing. When the answer is wrong,
//! exactly one of those is usually at fault and the rest were fine. Without
//! apportioning it, "learning from mistakes" degrades into penalising whatever
//! happens to be nearby, which teaches Spoon the wrong lesson and does it
//! confidently.
//!
//! The estimate is fuzzy on purpose and says so. Blame is spread across
//! candidates in proportion to the evidence rather than pinned on one, because
//! a confident wrong diagnosis is worse than an honest spread.

use serde::{Deserialize, Serialize};

use crate::episode::{EarsPath, Episode};

/// Where a turn can go wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Stage {
    /// The utterance was misread.
    Interpretation,
    /// A name was resolved to the wrong existing concept, or wrongly left as a
    /// new one.
    Reconciliation,
    /// The facts needed were not found, though they were there.
    Retrieval,
    /// A realization existed and the wrong one was chosen.
    Selection,
    /// The chosen realization ran and produced the wrong thing.
    Execution,
    /// Nothing could do it. Not a defect: a gap is a real answer.
    Capability,
    /// The interior was right and the wording was not.
    Phrasing,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Interpretation => "interpretation",
            Stage::Reconciliation => "reconciliation",
            Stage::Retrieval => "retrieval",
            Stage::Selection => "selection",
            Stage::Execution => "execution",
            Stage::Capability => "capability",
            Stage::Phrasing => "phrasing",
        }
    }
}

/// How much each stage is suspected, summing to 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blame {
    pub episode: u64,
    pub shares: Vec<(Stage, f64)>,
}

impl Blame {
    /// The single most suspected stage, when one stands out.
    ///
    /// `None` when the top two are within a hair of each other: naming a
    /// culprit the evidence does not single out is how a system ends up
    /// confidently wrong about its own failures.
    pub fn prime_suspect(&self) -> Option<Stage> {
        let mut ordered = self.shares.clone();
        ordered.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        match (ordered.first(), ordered.get(1)) {
            (Some((stage, top)), Some((_, second))) if top - second > 0.10 => Some(*stage),
            (Some((stage, _)), None) => Some(*stage),
            _ => None,
        }
    }

    pub fn share(&self, stage: Stage) -> f64 {
        self.shares
            .iter()
            .find(|(s, _)| *s == stage)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }
}

/// Apportion blame for a turn the user said was wrong.
///
/// Reads only what the episode already recorded. Nothing here re-runs the turn,
/// because the store has moved on since and a replay would be answering a
/// different question.
pub fn assign(episode: &Episode) -> Blame {
    let mut raw: Vec<(Stage, f64)> = Vec::new();

    // The ears failing outright is the least ambiguous signal there is.
    match episode.ears_path {
        EarsPath::Failed => raw.push((Stage::Interpretation, 5.0)),
        EarsPath::Model => raw.push((Stage::Interpretation, 1.0)),
        // A native reading came from a phrasing learned earlier, which can be
        // stale or over-general, so it is not above suspicion either.
        EarsPath::Native => raw.push((Stage::Interpretation, 1.5)),
    }

    // Words the ears could not place mean the reading was partly guesswork.
    if !episode.unknown_words.is_empty() {
        raw.push((
            Stage::Interpretation,
            1.0 * episode.unknown_words.len() as f64,
        ));
        // An unplaced word is also exactly what reconciliation exists to catch.
        raw.push((Stage::Reconciliation, 1.5));
    }

    // A realization that ran and failed is the most direct evidence available.
    let failures = episode.realizations.iter().filter(|(_, ok)| !ok).count();
    if failures > 0 {
        raw.push((Stage::Execution, 3.0 * failures as f64));
        // Something else was passed over in favour of the thing that failed.
        if episode.realizations.len() > failures {
            raw.push((Stage::Selection, 1.5));
        }
    }

    // Nothing could do it, which is a gap rather than a mistake.
    if !episode.gaps.is_empty() {
        raw.push((Stage::Capability, 2.5 * episode.gaps.len().min(3) as f64));
    }

    // A question that produced nothing, with no gap and no failure, points at
    // retrieval: the facts were either absent or not found.
    let answered_nothing = episode.result.is_none()
        || episode.reply.contains("do not know")
        || episode.reply.contains("did not follow");
    if answered_nothing && episode.gaps.is_empty() && failures == 0 {
        raw.push((Stage::Retrieval, 2.0));
    }

    // Everything worked and the answer was still wrong, so what is left is how
    // it was said.
    if failures == 0 && episode.gaps.is_empty() && episode.result.is_some() {
        raw.push((Stage::Phrasing, 1.0));
        if episode.mouth_path == crate::episode::MouthPath::Model {
            raw.push((Stage::Phrasing, 1.0));
        }
    }

    normalise(episode.id, raw)
}

/// Fold duplicate stages together and scale to sum to one.
fn normalise(episode: u64, raw: Vec<(Stage, f64)>) -> Blame {
    let mut totals: std::collections::BTreeMap<Stage, f64> = Default::default();
    for (stage, weight) in raw {
        *totals.entry(stage).or_default() += weight;
    }
    let sum: f64 = totals.values().sum();
    let shares = if sum <= 0.0 {
        // Nothing in the record points anywhere. Saying so beats inventing a
        // culprit to fill the slot.
        Vec::new()
    } else {
        let mut shares: Vec<(Stage, f64)> = totals.into_iter().map(|(s, w)| (s, w / sum)).collect();
        shares.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        shares
    };
    Blame { episode, shares }
}

/// Blame across many turns, so a stage that fails repeatedly stands out from
/// one that failed once.
pub fn assign_all(episodes: &[Episode]) -> Vec<(Stage, f64)> {
    let mut totals: std::collections::BTreeMap<Stage, f64> = Default::default();
    for episode in episodes.iter().filter(|e| e.looks_unsatisfying()) {
        for (stage, share) in assign(episode).shares {
            *totals.entry(stage).or_default() += share;
        }
    }
    let mut out: Vec<(Stage, f64)> = totals.into_iter().collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    out
}
