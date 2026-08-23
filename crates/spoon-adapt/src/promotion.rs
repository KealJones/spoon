//! Pure, conservative evaluation for a candidate skill before shadow deployment.
//!
//! This module deliberately has no store or mutation dependency.  The engine's
//! trust boundary is responsible for admitting authenticated replay evidence;
//! a passing decision here is only permission to *shadow*, never to install or
//! mutate a procedure.

use serde::{Deserialize, Serialize};
use spoon_core::EpisodeId;

/// Metrics captured while replaying one already-verified episode.
///
/// `None` means the metric was not measured and cannot be claimed as a win.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionReplay {
    pub episode_id: EpisodeId,
    pub incumbent_correct: bool,
    pub challenger_correct: bool,
    pub incumbent_trace_steps: Option<u32>,
    pub challenger_trace_steps: Option<u32>,
    pub incumbent_candidates_explored: Option<u32>,
    pub challenger_candidates_explored: Option<u32>,
    /// True only when this replay is outside the candidate's derivation domain.
    pub transfer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionWin {
    Compression,
    SearchCost,
    Coverage,
    Transfer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionVerdict {
    /// No trusted replay records were supplied.
    InsufficientEvidence,
    /// The challenger changed a previously verified result.
    Regression { episode_id: EpisodeId },
    /// The challenger preserved correctness but demonstrated no measured gain.
    NoMeasuredWin,
    /// The candidate may proceed to shadow deployment; it is not promoted.
    ShadowEligible { wins: Vec<PromotionWin> },
}

impl PromotionVerdict {
    pub fn shadow_eligible(&self) -> bool {
        matches!(self, Self::ShadowEligible { .. })
    }
}

/// Enforces Phase 4's replay rule: lose on none, win on at least one.
pub struct PromotionGate;

impl PromotionGate {
    pub fn evaluate(replays: impl IntoIterator<Item = PromotionReplay>) -> PromotionVerdict {
        let mut saw_replay = false;
        let mut wins = Vec::new();

        for replay in replays {
            saw_replay = true;
            if !replay.challenger_correct {
                return PromotionVerdict::Regression {
                    episode_id: replay.episode_id,
                };
            }

            if !replay.incumbent_correct {
                wins.push(PromotionWin::Coverage);
            }
            if replay
                .incumbent_trace_steps
                .zip(replay.challenger_trace_steps)
                .is_some_and(|(incumbent, challenger)| challenger < incumbent)
            {
                wins.push(PromotionWin::Compression);
            }
            if replay
                .incumbent_candidates_explored
                .zip(replay.challenger_candidates_explored)
                .is_some_and(|(incumbent, challenger)| challenger < incumbent)
            {
                wins.push(PromotionWin::SearchCost);
            }
            if replay.transfer {
                wins.push(PromotionWin::Transfer);
            }
        }

        if !saw_replay {
            PromotionVerdict::InsufficientEvidence
        } else if wins.is_empty() {
            PromotionVerdict::NoMeasuredWin
        } else {
            PromotionVerdict::ShadowEligible { wins }
        }
    }
}

#[cfg(test)]
mod tests {
    use spoon_core::EpisodeId;

    use super::{PromotionGate, PromotionReplay, PromotionVerdict, PromotionWin};

    fn replay(incumbent_correct: bool, challenger_correct: bool) -> PromotionReplay {
        PromotionReplay {
            episode_id: EpisodeId::new(),
            incumbent_correct,
            challenger_correct,
            incumbent_trace_steps: Some(4),
            challenger_trace_steps: Some(4),
            incumbent_candidates_explored: None,
            challenger_candidates_explored: None,
            transfer: false,
        }
    }

    #[test]
    fn regression_overrides_a_win_on_another_replay() {
        let failed = replay(true, false);
        assert_eq!(
            PromotionGate::evaluate([replay(false, true), failed.clone()]),
            PromotionVerdict::Regression {
                episode_id: failed.episode_id
            }
        );
    }

    #[test]
    fn requires_a_measured_win_after_preserving_correctness() {
        assert_eq!(
            PromotionGate::evaluate([replay(true, true)]),
            PromotionVerdict::NoMeasuredWin
        );
    }

    #[test]
    fn admits_shadow_only_for_a_measured_improvement() {
        let mut candidate = replay(true, true);
        candidate.challenger_trace_steps = Some(3);
        candidate.incumbent_candidates_explored = Some(8);
        candidate.challenger_candidates_explored = Some(5);
        candidate.transfer = true;

        assert_eq!(
            PromotionGate::evaluate([candidate]),
            PromotionVerdict::ShadowEligible {
                wins: vec![
                    PromotionWin::Compression,
                    PromotionWin::SearchCost,
                    PromotionWin::Transfer,
                ]
            }
        );
    }
}
