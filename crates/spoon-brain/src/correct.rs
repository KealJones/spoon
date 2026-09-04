//! Taking "no, wrong" seriously.
//!
//! A correction is the most informative thing a user ever says, and the
//! cheapest to waste. Apologising and moving on throws away the one moment
//! where Spoon is told exactly which of its guesses was bad.
//!
//! Silence is not the opposite. A user who says nothing may be satisfied or may
//! simply have given up, so an uncorrected turn is weak evidence at best and is
//! never counted as approval.

use chrono::{DateTime, Utc};
use spoon_concept::Concept;
use spoon_store::Store;

/// What a correction did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Correction {
    /// The turn being corrected.
    pub episode: u64,
    /// Assertions withdrawn. Withdrawn, not deleted: something that was
    /// believed and turned out wrong is part of the record.
    pub retracted: Vec<Concept>,
    /// Realizations marked down for having produced the bad answer.
    pub penalised: Vec<String>,
}

impl Correction {
    pub fn is_empty(&self) -> bool {
        self.retracted.is_empty() && self.penalised.is_empty()
    }
}

/// Undo what the previous turn concluded, and record why.
///
/// Retraction sets a validity end rather than removing the row, so
/// `assertions_at` an earlier instant still shows what Spoon believed at the
/// time. That history is what makes a wrong answer diagnosable later.
pub fn apply(
    store: &Store,
    previous: &crate::Episode,
    at: DateTime<Utc>,
) -> spoon_store::Result<Correction> {
    let mut correction = Correction {
        episode: previous.id,
        ..Default::default()
    };

    // Withdraw whatever that turn asserted.
    if let Some(claim) = &previous.result
        && previous.reply.starts_with("noted")
    {
        for record in store.live_assertions(claim)? {
            store.retract(record.id, at)?;
            correction.retracted.push(claim.clone());
        }
    }

    // Mark down whatever produced the answer. The realization may have been
    // blameless and the interpretation at fault, which is why this is evidence
    // rather than a verdict: one bad outcome nudges the score, it does not
    // condemn.
    for (name, succeeded) in &previous.realizations {
        if !succeeded {
            continue;
        }
        match store.record_realization_use(name, false, at) {
            Ok(()) => correction.penalised.push(name.clone()),
            // The realization has been retired or replaced since the turn ran.
            // There is nothing left to mark down, and a correction must not
            // fail over bookkeeping: the user's repair is the point, and the
            // retraction above has already happened.
            Err(spoon_store::StoreError::MissingRecord { .. }) => {}
            Err(err) => return Err(err),
        }
    }

    Ok(correction)
}

/// Does this utterance repair the previous turn rather than start a new one?
///
/// A finite, learnable set of markers. Noticing a repair does not require
/// understanding the sentence, which is why it works without a model, and the
/// list is short on purpose: a false positive silently deletes something the
/// user asserted.
pub fn is_correction(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        "no wait",
        "wait no",
        "actually no",
        "no,",
        "no.",
        "nope",
        "that's wrong",
        "thats wrong",
        "that is wrong",
        "wrong",
        "i meant",
        "i ment",
        "sorry i meant",
        "not that",
        "scratch that",
        "never mind",
        "nevermind",
        "nvm",
        "correction",
        "no i said",
        "not what i said",
    ];
    let lowered = text.trim().to_lowercase();
    MARKERS.iter().any(|m| {
        lowered == *m
            || lowered.starts_with(&format!("{m} "))
            || lowered.starts_with(&format!("{m}, "))
            || lowered.contains(&format!(" {m} "))
    })
}
