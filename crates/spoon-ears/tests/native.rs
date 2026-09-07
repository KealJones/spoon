//! When a greeting is the turn, and when it is only the opener.

use spoon_concept::Concept;
use spoon_ears::NativeEars;
use spoon_seat::Ears;

/// Whether the native path read this as nothing but a greeting.
///
/// Compared against the concept rather than a rendered string, because a
/// debug dump prints symbol ids and would match "greet" never or always.
fn greeted(say: &str) -> bool {
    let want = Concept::call("chat", [Concept::call("greet", [])]);
    NativeEars::new()
        .hear_native(say)
        .is_some_and(|heard| heard.steps.contains(&want))
}


#[test]
fn a_greeting_with_a_request_attached_is_not_a_greeting() {
    // "hey" as the first word used to swallow the whole turn, so
    // "hey quick one, 356 minus 43" answered hello and threw the sum away.
    for say in [
        "hey quick one, 356 minus 43",
        "yo whats 12 times 5",
        "hi can you reverse the word science",
    ] {
        assert!(!greeted(say), "{say:?} was read as nothing but a greeting");
    }
}

#[test]
fn a_bare_greeting_is_still_a_greeting() {
    for say in ["hey", "hey there", "yo yo yo", "hi how are you", "hello!"] {
        assert!(greeted(say), "{say:?} lost its greeting");
    }
}


#[test]
fn only_pleasantries_is_decided_without_a_model() {
    // The model is checked against this. A 4b model shown "hey reverse spoon
    // lol" often returns a greeting and drops the request, and answering
    // hello to a question is worse than admitting confusion: a greeting looks
    // like success, so nothing is recorded and the Teacher is never asked.
    for say in ["hey", "hey there", "yo yo yo", "hi how are you", "thanks", "bye"] {
        assert!(NativeEars::is_social_only(say), "{say:?} is small talk");
    }
    for say in [
        "hey reverse spoon lol",
        "hey quick one, 356 minus 43",
        "yo whats 12 times 5",
        "thanks can you also reverse banana",
    ] {
        assert!(
            !NativeEars::is_social_only(say),
            "{say:?} carries a request"
        );
    }
}
