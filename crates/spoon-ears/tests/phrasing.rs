//! What the native path can learn, and what it must refuse to learn.

use spoon_concept::{Concept, SymbolTable, parse, render};
use spoon_ears::PhrasingIndex;
use spoon_store::Store;
use spoon_store::pairs::PairSource;

/// One symbol table shared by parsing and rendering, so a concept prints with
/// the name it was written with instead of its hex id.
struct Fixture {
    table: SymbolTable,
    index: PhrasingIndex,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            table: SymbolTable::new(),
            index: PhrasingIndex::new(),
        }
    }

    fn taught(examples: &[(&str, &str)]) -> Fixture {
        let mut fixture = Fixture::new();
        for (utterance, reading) in examples {
            let steps = fixture.steps(reading);
            fixture.index.learn(utterance, &steps);
        }
        fixture
    }

    fn steps(&self, source: &str) -> Vec<Concept> {
        source
            .split(';')
            .map(|line| parse(line.trim(), &self.table).expect("test fixture must parse"))
            .collect()
    }

    fn learn(&mut self, utterance: &str, reading: &str) {
        let steps = self.steps(reading);
        self.index.learn(utterance, &steps);
    }

    fn recognize(&self, text: &str) -> Option<(String, f64)> {
        self.index
            .recognize(text)
            .map(|(steps, confidence)| (self.shown(&steps), confidence))
    }

    fn heard(&self, text: &str) -> Option<String> {
        self.recognize(text).map(|(steps, _)| steps)
    }

    fn shown(&self, concepts: &[Concept]) -> String {
        concepts
            .iter()
            .map(|c| render(c, &self.table))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[test]
fn substitutes_a_new_number_into_a_learned_shape() {
    let f = Fixture::taught(&[("can u double 21 for me", "do<double<21>>")]);
    let (got, confidence) = f
        .recognize("can u double 7 for me")
        .expect("same shape, different number");
    assert_eq!(got, "do<double<7>>");
    assert!(confidence >= 0.70, "confidence was {confidence}");
}

#[test]
fn a_different_content_word_is_a_different_request() {
    let f = Fixture::taught(&[("can u double 21 for me", "do<double<21>>")]);
    assert_eq!(f.heard("can u triple 21 for me"), None);
}

#[test]
fn filler_and_request_prefixes_do_not_block_a_match() {
    let f = Fixture::taught(&[("can u double 21 for me", "do<double<21>>")]);
    assert_eq!(
        f.heard("could you please double 9").as_deref(),
        Some("do<double<9>>"),
        "politeness is not content"
    );
}

#[test]
fn two_numeric_slots_fill_in_order() {
    let f = Fixture::taught(&[("add 3 and 4 together", "do<add<3, 4>>")]);
    assert_eq!(
        f.heard("add 10 and 32 together").as_deref(),
        Some("do<add<10, 32>>")
    );
}

#[test]
fn a_number_and_a_quoted_string_are_separate_slots() {
    let f = Fixture::taught(&[("repeat \"hello\" 3 times", "do<repeat<\"hello\", 3>>")]);
    assert_eq!(
        f.heard("repeat \"goodbye\" 5 times").as_deref(),
        Some("do<repeat<\"goodbye\", 5>>")
    );
}

/// A repeated value is one slot, not two. Two slots would let `add 7 and 9`
/// bind only the first and quietly answer `add<7, 7>`.
#[test]
fn a_repeated_value_shares_one_slot() {
    let f = Fixture::taught(&[("add 5 and 5", "do<add<5, 5>>")]);
    assert_eq!(f.heard("add 8 and 8").as_deref(), Some("do<add<8, 8>>"));
    assert_eq!(
        f.heard("add 8 and 9"),
        None,
        "the two tokens disagree, so the template does not describe this"
    );
}

/// A capitalised word the reading actually mentions is a name slot. "dog" is
/// not: it is a lowercase common noun, and abstracting it would need every
/// content word abstracted, leaving `{X} has a {Y}`, which matches any
/// four-word sentence with "has a" in it. That is exactly the wrong-reading
/// failure this module exists to prevent, so one slot is right here and two is
/// not.
#[test]
fn a_name_is_a_slot_but_a_common_noun_is_not() {
    let f = Fixture::taught(&[("John has a dog", "assert-that<owns<john, dog>>")]);
    // A name slot mints a symbol from a word that was never in the store, so
    // its spelling has to be registered before it can print as anything but
    // hex. The brain does this every turn from `Heard::names`; here the test
    // stands in for that.
    f.table.intern("Mary");
    assert_eq!(
        f.heard("Mary has a dog").as_deref(),
        Some("assert-that<owns<Mary, dog>>"),
        "the name varies, the rest does not"
    );
    assert_eq!(
        f.heard("Mary has a cat"),
        None,
        "cat is not dog, and nothing licenses swapping it"
    );
}

/// Written lowercase, "john" is not name-shaped, so the utterance has no slots
/// at all and the template only ever matches itself. Conservative, and right:
/// nothing in the text marks "john" as the variable part.
#[test]
fn a_lowercase_name_yields_a_literal_template() {
    let f = Fixture::taught(&[("john has a dog", "assert-that<owns<john, dog>>")]);
    assert_eq!(
        f.heard("john has a dog").as_deref(),
        Some("assert-that<owns<john, dog>>")
    );
    assert_eq!(f.heard("mary has a dog"), None);
}

/// A number the reading never mentions cannot be a slot: filling it would
/// change the utterance without changing the answer.
#[test]
fn a_number_missing_from_the_reading_stays_literal() {
    let f = Fixture::taught(&[("show the top 5 results", "do<top-results<>>")]);
    assert_eq!(
        f.heard("show the top 5 results").as_deref(),
        Some("do<top-results<>>")
    );
    assert_eq!(
        f.heard("show the top 7 results"),
        None,
        "7 has nowhere to go, so this must not answer as though it did"
    );
}

/// Holes the model put in the reading mean "unspecified" and must survive
/// intact. Slot holes are numbered clear of them.
#[test]
fn model_holes_are_not_mistaken_for_slots() {
    let f = Fixture::taught(&[("who owns 3 dogs", "ask<owns<?0, 3>>")]);
    assert_eq!(
        f.heard("who owns 7 dogs").as_deref(),
        Some("ask<owns<?0, 7>>")
    );
}

#[test]
fn a_typo_on_a_short_utterance_still_matches() {
    let f = Fixture::taught(&[("double 21", "do<double<21>>")]);
    assert_eq!(
        f.heard("duoble 6").as_deref(),
        Some("do<double<6>>"),
        "one transposition, same word"
    );
}

/// Edit distance must not turn opposite instructions into each other. The
/// shared-first-character guard is what stops it.
#[test]
fn edit_distance_does_not_bridge_opposite_words() {
    let f = Fixture::taught(&[("double 21", "do<double<21>>")]);
    assert_eq!(f.heard("triple 21"), None);
}

/// Partial content agreement is not agreement: two of three content words is
/// under the gate, so the reading is refused rather than guessed at.
#[test]
fn a_weak_content_match_is_refused() {
    let f = Fixture::taught(&[("sort the list by size", "do<sort-by<list, size>>")]);
    assert_eq!(f.heard("sort the list"), None);
}

/// Covering every content word the template has is still not enough when the
/// input brings one of its own: the extra word is something the template
/// cannot explain, and the confidence lands under the threshold.
#[test]
fn full_coverage_with_an_extra_content_word_falls_under_the_threshold() {
    let f = Fixture::taught(&[(
        "sort the alpha beta gamma delta records",
        "do<sort<records>>",
    )]);
    assert_eq!(
        f.heard("sort the alpha beta gamma delta epsilon records"),
        None
    );
}

#[test]
fn unicode_and_emoji_do_not_panic() {
    let mut f = Fixture::new();
    f.learn("double 21 🎉", "do<double<21>>");
    f.learn("dupliquer 21 café", "do<double<21>>");
    f.learn("日本語のテキスト", "chat<greet<>>");
    f.learn("🎉🎉🎉", "chat<greet<>>");

    for probe in [
        "double 7 🎉",
        "🎉🎉🎉",
        "日本語のテキスト",
        "𝕕𝕠𝕦𝕓𝕝𝕖 7",
        "\u{200b}\u{200b}",
        "\"unterminated 5",
        "-- 5 --",
        "café ☕ 3.5",
    ] {
        let _ = f.heard(probe);
    }
}

#[test]
fn empty_and_tiny_inputs_never_match() {
    let f = Fixture::taught(&[
        ("double 21", "do<double<21>>"),
        ("a b", "chat<greet<>>"),
        ("x", "chat<greet<>>"),
    ]);
    assert_eq!(f.heard(""), None);
    assert_eq!(f.heard("   \t\n "), None);
    assert_eq!(f.heard("x"), None);
    assert_eq!(f.heard("7"), None);
}

#[test]
fn nothing_worth_abstracting_is_not_learned() {
    let mut f = Fixture::new();
    f.learn("", "do<double<21>>");
    f.learn("   ", "do<double<21>>");
    f.learn("x", "do<double<21>>");
    // Filler and nothing else: normalization leaves no tokens to build from.
    f.learn("um uh like", "do<double<21>>");
    let empty: Vec<Concept> = Vec::new();
    f.index.learn("double 21", &empty);
    assert_eq!(f.index.len(), 0);
}

#[test]
fn the_same_phrasing_is_learned_once() {
    let mut f = Fixture::new();
    f.learn("double 21", "do<double<21>>");
    f.learn("double 21", "do<double<21>>");
    f.learn("double 99", "do<double<99>>");
    assert_eq!(f.index.len(), 1, "one shape, however many times it is seen");

    // The same words read a different way is a second template: the two
    // readings should compete rather than overwrite each other.
    f.learn("double 21", "ask<double<21>>");
    assert_eq!(f.index.len(), 2);
}

#[test]
fn an_empty_index_recognizes_nothing() {
    let f = Fixture::new();
    assert!(f.index.is_empty());
    assert_eq!(f.heard("can u double 7 for me"), None);
}

// ------------------------------------------------------- learned from a brain

fn brain() -> Store {
    Store::open_in_memory().expect("in-memory brain")
}

#[test]
fn templates_come_back_out_of_the_store() {
    let table = SymbolTable::new();
    let store = brain();
    let steps = vec![parse("do<double<21>>", &table).unwrap()];
    store
        .put_pair("can u double 21 for me", &steps, PairSource::Model)
        .unwrap();

    let index = PhrasingIndex::from_store(&store).unwrap();
    assert_eq!(index.len(), 1);
    let (got, _) = index.recognize("can u double 7 for me").expect("learned");
    assert_eq!(render(&got[0], &table), "do<double<7>>");
}

/// The gate is confidence, and confidence carries the pair's history. A
/// phrasing that keeps producing bad readings has to stop winning, or one bad
/// generalization poisons the native path for good.
#[test]
fn a_phrasing_that_keeps_failing_stops_being_recognized() {
    let table = SymbolTable::new();
    let store = brain();
    let steps = vec![parse("do<double<21>>", &table).unwrap()];
    let id = store
        .put_pair("can u double 21 for me", &steps, PairSource::Model)
        .unwrap();

    assert!(
        PhrasingIndex::from_store(&store)
            .unwrap()
            .recognize("can u double 7 for me")
            .is_some()
    );

    for _ in 0..4 {
        store.record_pair_outcome(id, false).unwrap();
    }
    assert_eq!(
        PhrasingIndex::from_store(&store)
            .unwrap()
            .recognize("can u double 7 for me"),
        None,
        "four bad readings is enough to stop trusting this shape"
    );
}

/// The trust ordering does real work rather than decorating a struct: the same
/// template on the same input clears the threshold when a user approved it and
/// falls short when the model merely guessed it.
#[test]
fn a_confirmed_phrasing_generalizes_further_than_a_guess() {
    let table = SymbolTable::new();
    let utterance = "sort the alpha beta gamma delta records";
    let probe = "sort the alpha beta gamma delta epsilon records";
    let steps = vec![parse("do<sort<records>>", &table).unwrap()];

    let guessed = brain();
    guessed
        .put_pair(utterance, &steps, PairSource::Model)
        .unwrap();
    assert_eq!(
        PhrasingIndex::from_store(&guessed)
            .unwrap()
            .recognize(probe),
        None
    );

    let approved = brain();
    approved
        .put_pair(utterance, &steps, PairSource::Confirmed)
        .unwrap();
    assert!(
        PhrasingIndex::from_store(&approved)
            .unwrap()
            .recognize(probe)
            .is_some()
    );
}

// ------------------------------------------------------------------- corpus

/// Real utterances from the session that designed this system, typos and all.
fn corpus() -> Vec<String> {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/bench/session.json"
    ))
    .expect("bench corpus");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("bench corpus is json");
    parsed["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// A fixed, mechanical stand-in for what the model would have returned.
///
/// The head is the longest purely alphabetic token and the arguments are the
/// integers, in order. The rule is crude on purpose: it knows nothing about
/// this module's stopword list or its slot rules, so the corpus number it
/// produces is a measurement rather than a reflection of the implementation
/// agreeing with itself. Hand-writing 36 readings would have let the numbers be
/// steered.
fn mechanical(table: &SymbolTable, line: &str) -> Vec<Concept> {
    let words: Vec<String> = line
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let head = words
        .iter()
        .filter(|w| w.chars().all(|c| c.is_alphabetic()))
        .max_by_key(|w| w.chars().count())
        .cloned()
        .unwrap_or_else(|| "utter".to_string());
    let args: Vec<Concept> = words
        .iter()
        .filter_map(|w| w.parse::<i64>().ok())
        .map(Concept::int)
        .collect();
    vec![
        parse(
            &format!("do<{}>", render(&Concept::call(&head, args), table)),
            table,
        )
        .expect("mechanical reading parses"),
    ]
}

/// The split the brief asked for: learn the first half, see what the second
/// half gets for free.
///
/// The answer is nothing, and the corpus explains why. These are 36 distinct
/// turns of one design conversation, not a user repeating themselves. The one
/// genuinely recurring shape in the file ("kick off stage 2" / "yeah kick off
/// stage 3") sits entirely inside the first half, and the second half shares no
/// phrasing with the first at all. Induction has nothing to induce from. The
/// number is asserted exactly so that a change which starts matching across
/// these two halves shows up as a failure to be looked at, in either direction.
#[test]
fn corpus_first_half_teaches_the_second_half_nothing() {
    let table = SymbolTable::new();
    let lines = corpus();
    let split = lines.len() / 2;

    let mut index = PhrasingIndex::new();
    for line in &lines[..split] {
        index.learn(line, &mechanical(&table, line));
    }
    assert_eq!(index.len(), 18, "every first-half line yields a template");

    let recognized = lines[split..]
        .iter()
        .filter(|line| index.recognize(line).is_some())
        .count();
    assert_eq!(recognized, 0);
}

/// The measurement that means something: walk the corpus in order, try the
/// native path first, and fall back to learning the line the way the brain
/// would after a model call. This is the weaning curve on real input.
///
/// It is a small number, and the corpus is why: a design conversation is 36
/// different things said once each. The value of this test is the exact count,
/// which is a regression guard in both directions. A drop means induction
/// broke; a jump means the gates loosened and wrong readings are getting
/// through, which is the more expensive failure.
#[test]
fn corpus_streaming_recognition_rate() {
    let table = SymbolTable::new();
    let lines = corpus();

    let mut index = PhrasingIndex::new();
    let mut correct = 0usize;
    let mut wrong = 0usize;
    for line in &lines {
        let truth = mechanical(&table, line);
        match index.recognize(line) {
            Some((steps, _)) if steps == truth => correct += 1,
            Some(_) => wrong += 1,
            None => index.learn(line, &truth),
        }
    }

    println!(
        "corpus: {correct} correct, {wrong} wrong out of {} lines",
        lines.len()
    );
    assert_eq!(correct, 1);
    assert_eq!(wrong, 0, "a wrong reading costs more than a miss");
}

#[test]
fn a_phrasing_that_keeps_being_wrong_stops_winning() {
    // The store has kept success and failure counts for pairs from the
    // beginning and nothing outside the tests ever wrote to them, so a
    // phrasing that generalized badly kept firing forever and training could
    // only make the ears more confident, never better.
    let store = Store::open_in_memory().expect("store");
    let id = store
        .put_pair(
            "make it lowercase",
            &[parse("lower<?0>", &SymbolTable::new()).expect("parse")],
            PairSource::Model,
        )
        .expect("pair");
    let mut index = PhrasingIndex::from_store(&store).expect("index");

    let before = index
        .recognize("make it lowercase")
        .expect("recognized")
        .1;

    for _ in 0..6 {
        index.record(id, false);
    }
    let after = index.recognize("make it lowercase").map(|(_, c)| c);

    // Either it now scores below what the ears will act on, or it is at least
    // clearly worse than it was. Both mean the evidence is being felt.
    if let Some(confidence) = after {
        assert!(
            confidence < before,
            "standing did not fall: {before} then {confidence}"
        );
    }
}

#[test]
fn success_and_failure_move_a_phrasing_in_opposite_directions() {
    let store = Store::open_in_memory().expect("store");
    let id = store
        .put_pair(
            "make it lowercase",
            &[parse("lower<?0>", &SymbolTable::new()).expect("parse")],
            PairSource::Model,
        )
        .expect("pair");
    let mut index = PhrasingIndex::from_store(&store).expect("index");
    let start = index.recognize("make it lowercase").expect("seen").1;

    for _ in 0..5 {
        index.record(id, true);
    }
    let up = index.recognize("make it lowercase").expect("seen").1;
    assert!(up > start, "success did not help: {start} then {up}");
}
#[test]
fn a_word_argument_becomes_a_slot() {
    let table = SymbolTable::new();
    let mut index = PhrasingIndex::new();
    index.learn(
        "make COMMITTEE lowercase",
        &[parse("do<lower<\"COMMITTEE\">>", &table).expect("parse")],
    );
    let got = index.recognize("make REALIZATION lowercase");
    assert!(
        got.is_some(),
        "a template learned from one word did not generalize to another"
    );
    let (steps, _) = got.unwrap();
    assert_eq!(
        render(&steps[0], &table),
        "do<lower<\"REALIZATION\">>",
        "the slot was not refilled from the new sentence"
    );
}

#[test]
fn a_word_the_reading_does_not_quote_stays_literal() {
    // "reverse" must not become a slot, or the template matches every
    // three-word sentence. The reading holds it as a head, not as text, so
    // the steps-linkage check throws it out.
    let table = SymbolTable::new();
    let mut index = PhrasingIndex::new();
    index.learn(
        "reverse banana",
        &[parse("do<reverse<\"banana\">>", &table).expect("parse")],
    );
    assert!(
        index.recognize("upper banana").is_none(),
        "the verb was treated as a slot"
    );
    let (steps, _) = index.recognize("reverse science").expect("same shape");
    assert_eq!(render(&steps[0], &table), "do<reverse<\"science\">>");
}

#[test]
fn numbers_still_generalize_the_way_they_did() {
    let table = SymbolTable::new();
    let mut index = PhrasingIndex::new();
    index.learn(
        "what is 3 plus 4",
        &[parse("ask<add<3, 4>>", &table).expect("parse")],
    );
    let (steps, _) = index.recognize("what is 21 plus 8").expect("same shape");
    assert_eq!(render(&steps[0], &table), "ask<add<21, 8>>");
}
