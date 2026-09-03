//! Copula complement shapes, shared by the parser and the realizer.
//!
//! Two `is`-complements carry a second argument that a plain attribute would
//! throw away, so both become two-place forms:
//!
//! - `X is north of Y`, `X is to the left of Y`, `X is afraid of Y` ->
//!   `Pred { pred: "north-of" | "left-of" | "afraid-of", args: [X, Y] }`.
//!   Function words inside the phrase are dropped so the relation name is
//!   stable whichever way the phrase is spelled.
//! - `X is in P` (a locative preposition) -> `be(the location of X, P)`, i.e.
//!   the subject is the *owner* of a definite `location` referent whose only
//!   modifier is the preposition that was used. The discourse layer already
//!   stores and queries a possessed noun as `rel.<noun>(owner, value)`, which
//!   is the same `rel.location` the move verbs write.

use super::lexicon::Lexicon;

/// Prepositions whose copula complement states where the subject is.
const LOCATIVE_PREPS: &[&str] = &["in", "at", "inside"];

/// Noun of the referent that holds a locative copula complement.
pub const LOCATION_NOUN: &str = "location";

/// Longest phrase accepted before the `of` of a relational complement.
pub const MAX_RELATION_WORDS: usize = 3;

/// True when `X is <prep> P` says where X is.
pub fn is_locative_prep(w: &str) -> bool {
    LOCATIVE_PREPS.contains(&w)
}

/// Relation name for the words between `is` and `of`: `["to", "the", "left"]`
/// -> `left-of`. None when nothing meaningful is left.
pub fn relation_of_name(words: &[String]) -> Option<String> {
    let kept: Vec<&str> = words
        .iter()
        .map(|w| w.as_str())
        .filter(|w| !Lexicon::is_function_word(w) && !Lexicon::is_prep(w))
        .collect();
    if kept.is_empty() {
        return None;
    }
    Some(format!("{}-of", kept.join("-")))
}

/// Inverse of [`relation_of_name`] for the realizer: `left-of` -> `left of`.
/// None for any other predicate, including comparatives (`bigger-than`).
pub fn relation_of_words(pred: &str) -> Option<String> {
    let stem = pred.strip_suffix("-of")?;
    if stem.is_empty() {
        return None;
    }
    Some(format!("{} of", stem.replace('-', " ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(ws: &[&str]) -> Vec<String> {
        ws.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn relation_names_drop_function_words() {
        assert_eq!(relation_of_name(&words(&["north"])).as_deref(), Some("north-of"));
        assert_eq!(relation_of_name(&words(&["to", "the", "left"])).as_deref(), Some("left-of"));
        assert_eq!(relation_of_name(&words(&["in", "front"])).as_deref(), Some("front-of"));
        assert_eq!(relation_of_name(&words(&["afraid"])).as_deref(), Some("afraid-of"));
        assert_eq!(relation_of_name(&words(&["the"])), None);
        assert_eq!(relation_of_name(&[]), None);
    }

    #[test]
    fn relation_words_round_trip() {
        for name in ["north-of", "left-of", "afraid-of"] {
            let spelled = relation_of_words(name).expect(name);
            let again = relation_of_name(&words(&spelled.split(' ').collect::<Vec<_>>()));
            assert_eq!(again.as_deref(), Some(name), "{spelled}");
        }
        assert_eq!(relation_of_words("bigger-than"), None);
        assert_eq!(relation_of_words("own"), None);
        assert_eq!(relation_of_words("-of"), None);
    }
}
