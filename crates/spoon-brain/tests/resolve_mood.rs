//! Asking versus telling, decided by the sentence rather than by the ears.

use spoon_brain::{Move, is_question, resolve};
use spoon_concept::Concept;

fn fact() -> Concept {
    Concept::call(
        "friends",
        [Concept::named("carol"), Concept::named("frank")],
    )
}

#[test]
fn interrogatives_are_recognized_without_a_model() {
    for say in [
        "is carol friends with frank",
        "are they related?",
        "does spoon know about json",
        "did that work",
        "how many rs in strawberry",
        "whats 2 plus 2",
        "can you reverse banana",
    ] {
        assert!(is_question(say), "{say:?} should be a question");
    }
    for say in [
        "friendship is symmetric",
        "john has a dog",
        "reverse banana",
        "carol is friends with frank",
    ] {
        assert!(!is_question(say), "{say:?} should not be a question");
    }
}

#[test]
fn a_question_never_becomes_an_assertion() {
    // The ears read "is carol friends with frank" as a statement and Spoon
    // replied "noted", writing a fact the speaker had only asked about. A
    // wrong answer is visible; a wrong assertion poisons every later question.
    let asserted = Concept::call("assert-that", [fact()]);
    assert!(matches!(
        resolve(std::slice::from_ref(&asserted), true).as_slice(),
        [Move::Ask(_)]
    ));
    assert!(matches!(
        resolve(std::slice::from_ref(&asserted), false).as_slice(),
        [Move::Assert(_)]
    ));
}

#[test]
fn an_unwrapped_step_follows_the_mood_of_the_sentence() {
    let bare = fact();
    assert!(matches!(
        resolve(std::slice::from_ref(&bare), true).as_slice(),
        [Move::Ask(_)]
    ));
    assert!(matches!(
        resolve(std::slice::from_ref(&bare), false).as_slice(),
        [Move::Do(_)]
    ));
}

#[test]
fn an_explicit_ask_stays_an_ask_either_way() {
    let asked = Concept::call("ask", [fact()]);
    for question in [true, false] {
        assert!(matches!(
            resolve(std::slice::from_ref(&asked), question).as_slice(),
            [Move::Ask(_)]
        ));
    }
}
