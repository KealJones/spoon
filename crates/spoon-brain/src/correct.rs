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

    // New episodes carry exactly the rows they created, regardless of how the
    // mouth described them. Other assertions of the same concept stay intact.
    if let Some(assertions) = &previous.assertions {
        for (id, claim) in assertions {
            if store.retract(spoon_store::AssertionId(*id), at)? {
                correction.retracted.push(claim.clone());
            }
        }
    } else if let Some(claim) = &previous.result
        && previous.reply.starts_with("noted")
    {
        // Compatibility for episodes written before assertion lineage existed.
        for record in store.live_assertions(claim)? {
            if store.retract(record.id, at)? {
                correction.retracted.push(claim.clone());
            }
        }
    }
    if let Some(pair) = previous.phrasing {
        match store.record_pair_outcome(pair, false) {
            Ok(()) | Err(spoon_store::StoreError::MissingRecord { .. }) => {}
            Err(err) => return Err(err),
        }
    }

    // Mark down whatever produced the answer. The realization may have been
    // blameless and the interpretation at fault, which is why this is evidence
    // rather than a verdict: one bad outcome nudges the score, it does not
    // condemn.
    let mut seen = std::collections::HashSet::new();
    for (name, succeeded) in &previous.realizations {
        if !succeeded || !seen.insert(name) {
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
/// Ways a speaker signals that what they just said was wrong.
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

pub fn is_correction(text: &str) -> bool {
    let lowered = text.trim().to_lowercase();
    MARKERS.iter().any(|m| {
        lowered == *m
            || lowered.starts_with(&format!("{m} "))
            || lowered.starts_with(&format!("{m}, "))
            || lowered.contains(&format!(" {m} "))
    })
}

/// Where a repair marker sits, and what surrounds it.
///
/// A marker at the start of an utterance repairs the previous turn. A marker
/// in the middle repairs the same sentence, and the difference matters: acting
/// on the second as though it were the first retracts something the speaker
/// never mentioned.
pub struct Repair {
    /// The clause before the marker, empty when the marker opened the
    /// utterance.
    pub before: String,
    /// The clause after it.
    pub after: String,
}

/// Find the last repair marker in an utterance and split around it.
///
/// The last, not the first, because a speaker who changes their mind twice
/// means the third thing.
pub fn split_repair(text: &str) -> Option<Repair> {
    let lowered = text.to_lowercase();
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for marker in MARKERS {
        let mut from = 0;
        while let Some(at) = lowered[from..].find(marker) {
            let start = from + at;
            hits.push((start, marker.len()));
            from = start + marker.len();
        }
    }
    // Markers overlap: "no," starts inside "actually no". Splitting on the
    // shorter one leaves "actually" stranded in the clause being repaired,
    // so a match that any earlier-starting match runs into is dropped in
    // favour of the fuller phrase. Overlap rather than containment, because
    // "actually no" stops one character short of "no,".
    let outer: Vec<(usize, usize)> = hits
        .iter()
        .copied()
        .filter(|(start, len)| {
            !hits.iter().any(|(other, other_len)| {
                (*other, *other_len) != (*start, *len)
                    && *other < *start
                    && other + other_len > *start
            })
        })
        .collect();
    // The last one, because a speaker who changes their mind twice means the
    // third thing.
    let (start, len) = outer.into_iter().max_by_key(|(start, _)| *start)?;
    // Punctuation on either side belongs to the marker, not to the clauses.
    // A leading comma left on the repair counts as a word and throws off the
    // splice, which is decided by counting them.
    let trim = |s: &str| {
        s.trim()
            .trim_matches([',', '.', '!', '?', ';', ':'])
            .trim()
            .to_string()
    };
    Some(Repair {
        before: trim(&text[..start]),
        after: trim(&text[start + len..]),
    })
}

/// The sentence the speaker meant, once the repair is applied.
///
/// A repair is usually elliptical: "reverse banana, actually no, possession"
/// replaces one word and leaves the verb implied, so reading only the part
/// after the marker gets a bare noun and reading only the part before it gets
/// the answer to a question that was withdrawn mid-sentence. Both were
/// happening, across every correction case in the corpus.
///
/// Splicing by token count is enough to tell the two apart. A repair as long
/// as what it replaces is a whole new sentence and stands on its own; a
/// shorter one replaces that many words at the end of the original.
pub fn repaired(text: &str) -> Option<String> {
    let repair = split_repair(text)?;
    if repair.after.is_empty() {
        return None;
    }
    if repair.before.is_empty() {
        return Some(repair.after);
    }
    // Repair the earlier clause first. "reverse banana, no wait, coffee,
    // actually no, spoon" has to become "reverse coffee" before "spoon" can
    // replace anything, or the splice counts the abandoned words and keeps
    // some of them.
    let earlier = repaired(&repair.before).unwrap_or(repair.before);
    let before: Vec<&str> = earlier.split_whitespace().collect();
    let after: Vec<&str> = repair.after.split_whitespace().collect();
    if after.len() >= before.len() {
        return Some(repair.after);
    }
    let keep = before.len() - after.len();
    let mut words: Vec<&str> = before[..keep].to_vec();
    words.extend(after);
    Some(words.join(" "))
}
