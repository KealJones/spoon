//! Every head the templates match on has to be a name the mouth knows.
//!
//! A symbol id is derived from its name, so a head the table has never been
//! told about renders as hex. The failure is silent and looks like this:
//!
//!     #b19f897d4a7d4402<reverse<"hello">>
//!
//! which reached a user because the head was added to the template match and
//! not to the list of names. Nothing connected the two, so this test does.

use std::sync::Arc;

use spoon_concept::{Concept, SymbolTable};
use spoon_mouth::TemplateMouth;
use spoon_seat::Mouth;

/// Heads the templates are expected to handle. Kept here rather than imported
/// so that adding a match arm without a name shows up as a failure rather than
/// as agreement between a list and itself.
const EXPECTED: &[&str] = &[
    "noted",
    "answer",
    "partial",
    "unknown",
    "needs-permission",
    "error",
    "did-not-understand",
    "nothing-to-say",
    "cannot-yet",
    "greet",
    "acknowledge-thanks",
    "farewell",
];

#[test]
fn no_response_head_renders_as_hex() {
    let table = Arc::new(SymbolTable::new());
    let mouth = TemplateMouth::new(table);

    for head in EXPECTED {
        let response = Concept::call(head, [Concept::text("something")]);
        let rendered = mouth.say_native(&response, &[]);
        assert!(
            !rendered.contains('#'),
            "{head} rendered as hex, so it is matched in the template and \
             missing from the name list: {rendered}"
        );
        assert!(!rendered.is_empty(), "{head} rendered as nothing");
    }
}

#[test]
fn a_zero_argument_response_still_reads() {
    let table = Arc::new(SymbolTable::new());
    let mouth = TemplateMouth::new(table);
    for head in [
        "greet",
        "farewell",
        "acknowledge-thanks",
        "did-not-understand",
        "nothing-to-say",
    ] {
        let rendered = mouth.say_native(&Concept::call(head, []), &[]);
        assert!(!rendered.contains('#'), "{head}: {rendered}");
        assert!(!rendered.is_empty(), "{head} rendered as nothing");
    }
}

#[test]
fn cannot_yet_names_what_was_wanted() {
    // The point of this response is to say what could not be done. Echoing the
    // internal notation instead hands the reader a concept expression when they
    // asked a question.
    let table = Arc::new(SymbolTable::new());
    table.intern("list-reverse");
    let mouth = TemplateMouth::new(table);

    let wanted = Concept::call("list-reverse", [Concept::text("hello")]);
    let rendered = mouth.say_native(&Concept::call("cannot-yet", [wanted]), &[]);

    assert!(
        rendered.contains("list-reverse"),
        "should name the capability: {rendered}"
    );
    assert!(
        !rendered.contains('<'),
        "should not echo the notation: {rendered}"
    );
}
