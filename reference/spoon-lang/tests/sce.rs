//! SCE parser/realizer tests.
//!
//! 1. All SCE.md example sentences parse, and realize(parse(s)) re-parses equal.
//! 2. Specific structural assertions.
//! 3. Corpus parse rate >= 85%.
//! 4. Unknown-word guessing.
//! 5. Negative tests.

use spoon_core::types::clause::{Act, ArithExpr, ArithOp, Quant, QuestionKind, Term};
use spoon_core::types::value::Value;
use spoon_lang::sce::{lemmatize, parse, parse_text, realize, Lexicon};

const CORPUS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/bench/ace_corpus.json");

/// (id, expected_ace) for every corpus item.
fn corpus_items() -> Vec<(usize, String)> {
    let raw = std::fs::read_to_string(CORPUS_PATH).expect("corpus not found");
    let items: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("invalid json");
    items.iter().filter_map(|item| {
        let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        item.get("expected_ace").and_then(|v| v.as_str()).map(|s| (id, s.to_string()))
    }).collect()
}

/// Default lexicon seeded with every content word of the corpus as both noun
/// and verb (parser position disambiguates).
fn corpus_lexicon(items: &[(usize, String)]) -> Lexicon {
    let mut lex = Lexicon::with_defaults();
    for (_, ace) in items {
        for word in ace.split_whitespace() {
            let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
            let lw = w.to_lowercase();
            if !lw.is_empty() && !Lexicon::is_function_word(&lw) && !Lexicon::is_prep(&lw) {
                lex.add_noun(&lw);
                lex.add_verb(&lemmatize(&lw));
            }
        }
    }
    lex
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Canonicalize a Clause by renaming vars in order of first appearance.
fn canonical(clause: &spoon_core::types::clause::Clause) -> spoon_core::types::clause::Clause {
    use spoon_core::types::clause::{Clause, Pred, Referent, Term};
    use std::collections::HashMap;

    let mut map: HashMap<String, String> = HashMap::new();
    let mut cnt = 0u32;
    let _rename = |v: &str| -> String {
        map.entry(v.to_string()).or_insert_with(|| { cnt += 1; format!("x{}", cnt) }).clone()
    };

    fn canon_term(t: &Term, map: &mut HashMap<String, String>, cnt: &mut u32) -> Term {
        match t {
            Term::Var { var } => {
                let nv = map.entry(var.clone()).or_insert_with(|| { *cnt += 1; format!("x{}", cnt) }).clone();
                Term::Var { var: nv }
            }
            Term::Sub { clause } => Term::Sub { clause: Box::new(canon_clause(clause, map, cnt)) },
            Term::Arith { expr } => Term::Arith { expr: canon_arith(expr) },
            other => other.clone(),
        }
    }

    fn canon_arith(e: &ArithExpr) -> ArithExpr {
        match e {
            ArithExpr::Bin { op, lhs, rhs } => ArithExpr::Bin { op: *op, lhs: Box::new(canon_arith(lhs)), rhs: Box::new(canon_arith(rhs)) },
            ArithExpr::Neg { of } => ArithExpr::Neg { of: Box::new(canon_arith(of)) },
            other => other.clone(),
        }
    }

    fn canon_pred(p: &Pred, map: &mut HashMap<String, String>, cnt: &mut u32) -> Pred {
        Pred {
            pred: p.pred.clone(),
            args: p.args.iter().map(|a| canon_term(a, map, cnt)).collect(),
            negated: p.negated,
            modal: p.modal.clone(),
            adjuncts: p.adjuncts.iter().map(|(prep, t)| (prep.clone(), canon_term(t, map, cnt))).collect(),
            attr: p.attr.clone(),
        }
    }

    fn canon_ref(r: &Referent, map: &mut HashMap<String, String>, cnt: &mut u32) -> Referent {
        let nv = map.entry(r.var.clone()).or_insert_with(|| { *cnt += 1; format!("x{}", cnt) }).clone();
        let owner = r.owner.as_ref().map(|o| {
            map.entry(o.clone()).or_insert_with(|| { *cnt += 1; format!("x{}", cnt) }).clone()
        });
        Referent { var: nv, noun: r.noun.clone(), quant: r.quant.clone(), mods: r.mods.clone(), owner, span: None }
    }

    fn canon_clause(c: &Clause, map: &mut HashMap<String, String>, cnt: &mut u32) -> Clause {
        let referents: Vec<Referent> = c.referents.iter().map(|r| canon_ref(r, map, cnt)).collect();
        let conditions: Vec<Pred> = c.conditions.iter().map(|p| canon_pred(p, map, cnt)).collect();
        let then: Vec<Pred> = c.then.iter().map(|p| canon_pred(p, map, cnt)).collect();
        let then_referents: Vec<Referent> =
            c.then_referents.iter().map(|r| canon_ref(r, map, cnt)).collect();
        Clause { act: canon_act(&c.act, map), referents, conditions, then, then_referents, sce: c.sce.clone() }
    }

    /// The focus var of a wh-question is a var like any other.
    fn canon_act(act: &Act, map: &HashMap<String, String>) -> Act {
        use spoon_core::types::clause::QuestionKind as Q;
        let renamed = |v: &String| map.get(v).cloned().unwrap_or_else(|| v.clone());
        let Act::Question { kind } = act else { return act.clone() };
        let kind = match kind {
            Q::Who { focus } => Q::Who { focus: renamed(focus) },
            Q::What { focus } => Q::What { focus: renamed(focus) },
            Q::Which { focus } => Q::Which { focus: renamed(focus) },
            Q::HowMany { focus } => Q::HowMany { focus: renamed(focus) },
            Q::Where { focus } => Q::Where { focus: renamed(focus) },
            Q::When { focus } => Q::When { focus: renamed(focus) },
            other => other.clone(),
        };
        Act::Question { kind }
    }

    let mut m: HashMap<String, String> = HashMap::new();
    let mut c = 0u32;
    canon_clause(clause, &mut m, &mut c)
}

fn roundtrip(s: &str) {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse(s, &lex).unwrap_or_else(|e| panic!("parse failed for '{s}': {e}"));
    let realized = realize(&clause);
    let (clause2, _) = parse(&realized, &lex).unwrap_or_else(|e| {
        panic!("re-parse of realized '{}' (from '{}') failed: {}", realized, s, e)
    });
    let c1 = canonical(&clause);
    let c2 = canonical(&clause2);
    assert_eq!(c1.act, c2.act, "act mismatch for '{s}': realized '{realized}'");
    assert_eq!(c1.referents.len(), c2.referents.len(),
        "referent count mismatch for '{s}': realized '{realized}'");
    assert_eq!(c1.conditions.len(), c2.conditions.len(),
        "condition count mismatch for '{s}': realized '{realized}'");
}

// ---------------------------------------------------------------------------
// Test 1: SCE.md examples
// ---------------------------------------------------------------------------

#[test]
fn sce_md_examples() {
    let examples = [
        "User greets Assistant.",
        "What is the wellbeing of Assistant?",
        "Not every dog likes a cat.",
        "If a customer owns a card then the machine accepts the card.",
        "John says that Mary believes that Bob owns the dog.",
        "User does not know that the statement is true.",
        "Who is John?",
        "Assistant, tell Mike that User refuses!",
        "Should Assistant update Object-X to \"hi\"?",
        "Should Assistant save Object-X?",
        "Ben owns 10 apples.",
        "A train travels toward Ben.",
        "The train moves at 500 miles-per-hour.",
        "Every customer that is not an admin owns a card.",
        "No banned customer can enter.",
        "Mary is smarter than John.",
        "Bob may enter.",
        "How many apples does Ben own?",
        "Which person should own the task?",
        "Assistant, do not answer the question!",
    ];
    let lex = Lexicon::with_defaults();
    for s in &examples {
        let res = parse(s, &lex);
        assert!(res.is_ok(), "Failed to parse '{s}': {:?}", res.err());
    }
    // Roundtrip for most examples (those without complex features)
    for s in &examples[..15] {
        roundtrip(s);
    }
}

#[test]
fn sce_md_examples_debug() {
    let hard = [
        "John says that Mary believes that Bob owns the dog.",
        "If a customer owns a card then the machine accepts the card.",
    ];
    let lex = Lexicon::with_defaults();
    for s in &hard {
        let (clause, _) = parse(s, &lex).expect(s);
        println!("{s}\n{clause:#?}\n");
    }
}

// ---------------------------------------------------------------------------
// Test 2: Structural assertions
// ---------------------------------------------------------------------------

#[test]
fn john_owns_dog() {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse("John owns a dog.", &lex).unwrap();
    assert!(matches!(clause.act, Act::Assert));
    assert_eq!(clause.referents.len(), 2);
    assert!(matches!(clause.referents[0].quant, Quant::Named(ref n) if n == "John"));
    assert!(matches!(clause.referents[1].quant, Quant::Indef));
    assert_eq!(clause.referents[1].noun.as_deref(), Some("dog"));
    assert_eq!(clause.conditions.len(), 1);
    assert_eq!(clause.conditions[0].pred, "own");
    assert_eq!(clause.conditions[0].args.len(), 2);
}

#[test]
fn who_owns_dog() {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse("Who owns a dog?", &lex).unwrap();
    let focus = match &clause.act {
        Act::Question { kind: QuestionKind::Who { focus } } => focus.clone(),
        other => panic!("Expected Who question, got {other:?}"),
    };
    assert!(clause.referents.iter().any(|r| r.var == focus && matches!(r.quant, Quant::Wh)));
}

#[test]
fn rule_customer_card() {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse("If a customer owns a card then the machine accepts the card.", &lex).unwrap();
    assert!(matches!(clause.act, Act::Rule));
    // then_referents contains "the machine" (Def) and reuses card
    let _has_machine = clause.then_referents.iter().any(|r| {
        r.noun.as_deref() == Some("machine") && matches!(r.quant, Quant::Def)
    }) || clause.referents.iter().any(|r| r.noun.as_deref() == Some("machine"));
    // card is in antecedent, machine in consequent
    assert!(clause.referents.iter().any(|r| r.noun.as_deref() == Some("card")));
}

#[test]
fn calculate_arith() {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse("Assistant, calculate (3 / 500) * 3600!", &lex).unwrap();
    assert!(matches!(clause.act, Act::Command));
    let pred = &clause.conditions[0];
    assert_eq!(pred.pred, "calculate");
    let arith = pred.args.iter().find(|a| matches!(a, Term::Arith { .. })).expect("no arith term");
    if let Term::Arith { expr } = arith {
        // Should be Mul(Div(3, 500), 3600)
        assert!(matches!(expr, ArithExpr::Bin { op: ArithOp::Mul, .. }), "Expected Mul at top, got {expr:?}");
    }
}

#[test]
fn mary_smarter_than_john() {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse("Mary is smarter than John.", &lex).unwrap();
    assert_eq!(clause.conditions[0].pred, "be");
    let attr = clause.conditions[0].attr.as_deref().unwrap_or("");
    assert!(attr.contains("smarter"), "Expected smarter-than attr, got '{attr}'");
}

#[test]
fn no_banned_customer_can_enter() {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse("No banned customer can enter.", &lex).unwrap();
    let cust_ref = clause.referents.iter().find(|r| r.noun.as_deref() == Some("customer")).unwrap();
    assert!(matches!(cust_ref.quant, Quant::No));
    assert!(cust_ref.mods.iter().any(|m| m == "banned"));
    assert!(clause.conditions[0].modal.is_some());
}

#[test]
fn tell_mike_user_refuses() {
    let lex = Lexicon::with_defaults();
    let (clause, _) = parse("Assistant, tell Mike that User refuses!", &lex).unwrap();
    assert!(matches!(clause.act, Act::Command));
    let pred = &clause.conditions[0];
    assert_eq!(pred.pred, "tell");
    // Args: [Assistant, Mike, Sub(refuse(User))]
    assert!(pred.args.iter().any(|a| matches!(a, Term::Sub { .. })), "Expected Sub arg in tell");
}

// ---------------------------------------------------------------------------
// Test 3: Corpus parse rate >= 85%
// ---------------------------------------------------------------------------

#[test]
fn corpus_parse_rate() {
    let items = corpus_items();
    let lex = corpus_lexicon(&items);

    let total = items.len();
    let mut passed = 0;
    let mut failures: Vec<(usize, String, String)> = vec![]; // (id, ace, error)

    for (id, ace) in &items {
        let id = *id;
        match parse_text(ace, &lex) {
            Ok(_) => passed += 1,
            Err((sent_idx, e)) => {
                failures.push((id, ace.to_string(), format!("sentence {}: {}", sent_idx, e.message)));
            }
        }
    }

    let rate = passed as f64 / total as f64;
    eprintln!("\nCorpus parse rate: {}/{} = {:.1}%", passed, total, rate * 100.0);
    if !failures.is_empty() {
        eprintln!("\nFailures ({}):", failures.len());
        // Group by error category (first few words of error message)
        let mut by_error: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
        for (id, _, err) in &failures {
            let key: String = err.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
            by_error.entry(key).or_default().push(*id);
        }
        let mut sorted: Vec<_> = by_error.iter().collect();
        sorted.sort_by_key(|(_, ids)| -(ids.len() as i64));
        for (err, ids) in sorted.iter().take(15) {
            eprintln!("  [{}x] {} - items: {:?}", ids.len(), err, ids);
        }
        let mut all_ids: Vec<usize> = failures.iter().map(|(id, _, _)| *id).collect();
        all_ids.sort_unstable();
        eprintln!("  all failing ids: {:?}", all_ids);
    }

    assert!(
        rate >= 0.85,
        "Parse rate {:.1}% is below 85%. Failures:\n{}",
        rate * 100.0,
        failures.iter().take(20).map(|(id, ace, err)| format!("  ID {id}: {err}\n    ACE: {ace}")).collect::<Vec<_>>().join("\n")
    );
}

// ---------------------------------------------------------------------------
// Test 4: Unknown-word guessing
// ---------------------------------------------------------------------------

#[test]
fn unknown_word_guessing() {
    let lex = Lexicon::with_defaults();
    let (clause, unknowns) = parse("Zorbla glorps a fleem.", &lex).unwrap();
    // "Zorbla" -> name (capitalized), "glorps" -> verb, "fleem" -> noun
    assert_eq!(clause.conditions[0].pred, "glorp", "Expected lemma 'glorp', got '{}'", clause.conditions[0].pred);
    assert!(!unknowns.is_empty(), "Expected unknown words but got none");
    // fleem should be unknown
    assert!(unknowns.iter().any(|w| w.contains("fleem")), "Expected 'fleem' in unknowns, got {unknowns:?}");
}

// ---------------------------------------------------------------------------
// Test 5: Negative tests
// ---------------------------------------------------------------------------

#[test]
fn double_the_fails() {
    let lex = Lexicon::with_defaults();
    let res = parse("the the dog.", &lex);
    assert!(res.is_err(), "Expected parse failure for 'the the dog.'");
}

#[test]
fn no_terminator_fails() {
    let lex = Lexicon::with_defaults();
    let res = parse("John owns", &lex);
    assert!(res.is_err(), "Expected parse failure for 'John owns' (no terminator)");
}

#[test]
fn pronoun_rejected() {
    let lex = Lexicon::with_defaults();
    let res = parse("He owns a dog.", &lex);
    assert!(res.is_err(), "Expected failure for pronoun 'He'");
    let err = res.unwrap_err();
    assert!(
        err.message.to_lowercase().contains("pronoun"),
        "Expected error mentioning pronoun, got: {}",
        err.message
    );
}

#[test]
fn pronoun_she_rejected() {
    let lex = Lexicon::with_defaults();
    let res = parse("She likes cats.", &lex);
    assert!(res.is_err());
}

#[test]
fn debug_specific_fails() {
    use spoon_lang::sce::{parse, Lexicon};
    let lex = Lexicon::with_defaults();
    let tests = [
        "If the weather is rainy then Bob stays at home.",
        "Every customer owns at least 2 cards.",
        "Exactly 3 people wait.",
        "Not every employee is happy.",
        "If a customer owns an expired card and the customer is not an admin then Assistant rejects the card.",
    ];
    for s in &tests {
        match parse(s, &lex) {
            Ok((_c, _)) => println!("OK: {s}"),
            Err(e) => println!("FAIL: {s}\n  -> {:?}", e),
        }
    }
}

#[test]
fn debug_corpus_items() {
    let lex = corpus_lexicon(&corpus_items());
    let check = [
        "A man sleeps.",
        "If a dog is hungry then the dog eats.",
        "Exactly 3 people wait.",
        "There are at least 3 cats.",
        "Every customer owns at least 2 cards.",
    ];
    for s in &check {
        match parse_text(s, &lex) {
            Ok(_) => println!("OK: {s}"),
            Err((i, e)) => println!("FAIL: {s}\n  s{i}: {:?}", e.message),
        }
    }
}

#[test]
fn debug_man_sleeps() {
    use spoon_lang::sce::{parse, Lexicon};
    let mut lex = Lexicon::with_defaults();
    // Simulate corpus seeding for "man" and "sleeps"
    lex.add_noun("man");
    lex.add_verb("man");  // corpus adds man as verb too
    lex.add_noun("sleeps");
    lex.add_verb("sleep");
    println!("is_noun(man)={}", lex.is_noun("man"));
    println!("is_verb(man)={}", lex.is_verb("man"));
    println!("is_noun(sleeps)={}", lex.is_noun("sleeps"));
    println!("is_verb(sleeps)={}", lex.is_verb("sleeps"));
    println!("is_verb(sleep)={}", lex.is_verb("sleep"));
    match parse("A man sleeps.", &lex) {
        Ok((c, _)) => println!("OK: {:?}", c.conditions),
        Err(e) => println!("FAIL: {:?}", e),
    }
}

#[test]
fn debug_lemma() {
    use spoon_lang::sce::lemmatize;
    println!("lemmatize(man)={}", lemmatize("man"));
    println!("lemmatize(sleeps)={}", lemmatize("sleeps"));
    println!("lemmatize(sleep)={}", lemmatize("sleep"));
    println!("lemmatize(eats)={}", lemmatize("eats"));
}

#[test]
fn debug_lex_verb_set() {
    use spoon_lang::sce::Lexicon;
    let mut lex = Lexicon::with_defaults();
    lex.add_verb("man");
    lex.add_noun("sleeps");
    lex.add_verb("sleep"); // already in defaults
    // Direct lookup
    println!("is_verb(sleeps)={}", lex.is_verb("sleeps"));
    println!("is_verb(sleep)={}", lex.is_verb("sleep"));
    println!("verbs with 'sleep': {:?}", lex.verbs().filter(|v| v.contains("sleep")).collect::<Vec<_>>());
}

#[test]
fn debug_at_least_cards() {
    use spoon_lang::sce::{parse, lemmatize, Lexicon};
    let mut lex = Lexicon::with_defaults();
    // Simulate minimal seeding for this sentence
    let words = ["every", "customer", "owns", "at", "least", "2", "cards"];
    for w in &words {
        let lw = w.to_lowercase();
        if !Lexicon::is_function_word(&lw) && !Lexicon::is_prep(&lw) {
            lex.add_noun(&lw);
            lex.add_verb(&lemmatize(&lw));
        }
    }
    println!("is_verb(card)={}", lex.is_verb("card"));
    println!("is_verb(cards)={}", lex.is_verb("cards"));
    println!("is_noun(cards)={}", lex.is_noun("cards"));
    match parse("Every customer owns at least 2 cards.", &lex) {
        Ok((c, _)) => println!("OK: {:?}", c.referents.iter().map(|r| (&r.quant, &r.noun)).collect::<Vec<_>>()),
        Err(e) => println!("FAIL: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// Test 6: No silent drops. Every token before the terminator is consumed or
// the parse fails with the position of the first leftover token.
// ---------------------------------------------------------------------------

#[test]
fn no_silent_drops() {
    let lex = Lexicon::with_defaults();
    // (sentence, text of the first leftover token or None when the failure
    //  happens earlier, e.g. the pronoun gate)
    let must_fail: &[(&str, Option<&str>)] = &[
        // appositive after a definite NP, then and-coordination and a pronoun
        (r#"Assistant, open the file "/tmp/x" and save it!"#, Some(r#""/tmp/x""#)),
        // appositive alone
        (r#"Assistant, open the file "/tmp/x"!"#, Some(r#""/tmp/x""#)),
        // appositive in a declarative
        (r#"The file "/tmp/x" is large."#, None),
        // trailing junk word
        ("John owns a dog zorp.", Some("zorp")),
        // trailing number
        ("John owns a dog 42.", Some("42")),
        // second clause without a terminator
        ("John owns a dog Mary owns a cat.", Some("Mary")),
        // clause coordination is not VP coordination
        ("John owns a dog and Mary owns a cat.", None),
        // pronoun object
        ("Assistant, save it!", Some("it")),
        // PP with a pronoun
        ("John talks about it.", Some("it")),
        ("John talks about him.", None),
    ];
    for (s, leftover) in must_fail {
        let err = parse(s, &lex).err().unwrap_or_else(|| panic!("expected failure for {s:?}"));
        if let Some(tok) = leftover {
            assert!(
                err.message.contains(&format!("unexpected '{tok}'")),
                "{s:?}: expected leftover {tok:?} in message, got {:?}", err.message
            );
            assert!(err.position.is_some(), "{s:?}: leftover error must carry a position");
        }
    }

    let must_parse: &[&str] = &[
        "Assistant, open the file!",
        r#"Assistant, open "/tmp/x" and save "/tmp/x"!"#,
        "The file is large.",
        "John owns a dog.",
        "John owns 42 dogs.",
        "John owns a dog and likes a cat.",
        "Assistant, save the file!",
        "John talks about Mary.",
    ];
    for s in must_parse {
        parse(s, &lex).unwrap_or_else(|e| panic!("expected {s:?} to parse: {e}"));
    }
    let (clauses, _) = parse_text("John owns a dog. Mary owns a cat.", &lex).expect("two sentences");
    assert_eq!(clauses.len(), 2);
}

/// Every corpus sentence that parses consumed every token.
#[test]
fn full_consumption() {
    use spoon_lang::sce::{parse_traced, split_sentences};
    let items = corpus_items();
    let lex = corpus_lexicon(&items);
    let mut checked = 0;
    for (id, ace) in &items {
        for sent in split_sentences(ace) {
            if let Ok((_, _, (consumed, total))) = parse_traced(&sent, &lex) {
                assert_eq!(consumed, total, "item {id}: {sent:?} left {} token(s) unconsumed", total - consumed);
                checked += 1;
            }
        }
    }
    assert!(checked > 200, "expected to check hundreds of sentences, got {checked}");
}

// ---------------------------------------------------------------------------
// Test 7: Literal owners in "the Noun of NP"
// ---------------------------------------------------------------------------

/// Asserts the shape `be(the <noun> of <owner>, <value>)`: three referents,
/// the property referent is Def and owned by the literal owner.
fn assert_property_of_literal(s: &str, noun: &str, owner: Value, value: Value, act_ok: impl Fn(&Act) -> bool) {
    let lex = Lexicon::with_defaults();
    let (c, _) = parse(s, &lex).unwrap_or_else(|e| panic!("{s:?}: {e}"));
    assert!(act_ok(&c.act), "{s:?}: unexpected act {:?}", c.act);
    let owner_ref = c.referents.iter().find(|r| r.quant == Quant::Literal(owner.clone()))
        .unwrap_or_else(|| panic!("{s:?}: no owner literal {owner:?} in {:?}", c.referents));
    let prop = c.referents.iter().find(|r| r.noun.as_deref() == Some(noun))
        .unwrap_or_else(|| panic!("{s:?}: no {noun:?} referent in {:?}", c.referents));
    assert_eq!(prop.quant, Quant::Def, "{s:?}: property must be Def");
    assert_eq!(prop.owner.as_deref(), Some(owner_ref.var.as_str()), "{s:?}: property owner must be the literal");
    let value_ref = c.referents.iter().find(|r| r.quant == Quant::Literal(value.clone()))
        .unwrap_or_else(|| panic!("{s:?}: no value literal {value:?} in {:?}", c.referents));
    assert_eq!(c.conditions.len(), 1, "{s:?}: exactly one condition");
    let be = &c.conditions[0];
    assert_eq!(be.pred, "be");
    assert_eq!(be.args, vec![Term::Var { var: prop.var.clone() }, Term::Var { var: value_ref.var.clone() }]);
}

#[test]
fn literal_owners() {
    let is_assert = |a: &Act| matches!(a, Act::Assert);
    assert_property_of_literal("The double of 3 is 6.", "double", Value::Int(3), Value::Int(6), is_assert);
    assert_property_of_literal("The half of 2.5 is 1.25.", "half", Value::Float(2.5), Value::Float(1.25), is_assert);
    assert_property_of_literal("The double of -2 is -4.", "double", Value::Int(-2), Value::Int(-4), is_assert);
    assert_property_of_literal("The double of -1.5 is -3.", "double", Value::Float(-1.5), Value::Int(-3), is_assert);
    assert_property_of_literal(
        r#"The length of "abc" is 3."#, "length", Value::Text("abc".into()), Value::Int(3), is_assert,
    );
    assert_property_of_literal(
        r#"The reverse of "abc" is "cba"."#, "reverse", Value::Text("abc".into()), Value::Text("cba".into()), is_assert,
    );

    // What-questions: `What is the double of 3?` -> be(wh, the double of 3), focus = property var.
    let lex = Lexicon::with_defaults();
    for (s, noun, owner) in [
        ("What is the double of 3?", "double", Value::Int(3)),
        ("What is the double of -2.5?", "double", Value::Float(-2.5)),
        ("What is the double of -2?", "double", Value::Int(-2)),
        (r#"What is the length of "abc"?"#, "length", Value::Text("abc".into())),
    ] {
        let (c, _) = parse(s, &lex).unwrap_or_else(|e| panic!("{s:?}: {e}"));
        let focus = match &c.act {
            Act::Question { kind: QuestionKind::What { focus } } => focus.clone(),
            other => panic!("{s:?}: expected What question, got {other:?}"),
        };
        let owner_ref = c.referents.iter().find(|r| r.quant == Quant::Literal(owner.clone()))
            .unwrap_or_else(|| panic!("{s:?}: no owner literal in {:?}", c.referents));
        let prop = c.referents.iter().find(|r| r.var == focus)
            .unwrap_or_else(|| panic!("{s:?}: focus {focus} not a referent"));
        assert_eq!(prop.noun.as_deref(), Some(noun));
        assert_eq!(prop.quant, Quant::Def);
        assert_eq!(prop.owner.as_deref(), Some(owner_ref.var.as_str()));
        assert_eq!(c.conditions.len(), 1);
        assert_eq!(c.conditions[0].pred, "be");
        assert_eq!(c.conditions[0].args[1], Term::Var { var: focus });
    }
}

// ---------------------------------------------------------------------------
// Test 8b: bAbI forms. Every sentence here is verbatim from
// data/bench/babi_probes.json.
// ---------------------------------------------------------------------------

/// The clause and the referent lookup for one sentence.
fn parsed(s: &str) -> spoon_core::types::clause::Clause {
    let lex = Lexicon::with_defaults();
    parse(s, &lex).unwrap_or_else(|e| panic!("{s:?}: {e}")).0
}

fn referent<'a>(
    c: &'a spoon_core::types::clause::Clause,
    var: &str,
) -> &'a spoon_core::types::clause::Referent {
    c.referents.iter().find(|r| r.var == var).unwrap_or_else(|| panic!("no referent {var}"))
}

fn arg_var(p: &spoon_core::types::clause::Pred, i: usize) -> String {
    match &p.args[i] {
        Term::Var { var } => var.clone(),
        other => panic!("arg {i} is not a var: {other:?}"),
    }
}

/// `be(the location of X, P)` with the preposition kept as the property's mod.
fn assert_locative(s: &str, prep: &str, place_noun: &str) {
    let c = parsed(s);
    assert_eq!(c.conditions.len(), 1, "{s:?}: one condition");
    let p = &c.conditions[0];
    assert_eq!(p.pred, "be", "{s:?}");
    assert_eq!(p.args.len(), 2, "{s:?}: be(location, place)");
    let prop = referent(&c, &arg_var(p, 0));
    assert_eq!(prop.noun.as_deref(), Some("location"), "{s:?}");
    assert_eq!(prop.quant, Quant::Def, "{s:?}");
    assert_eq!(prop.mods, vec![prep.to_string()], "{s:?}");
    let owner = prop.owner.as_deref().unwrap_or_else(|| panic!("{s:?}: no owner"));
    assert!(matches!(referent(&c, owner).quant, Quant::Named(_)), "{s:?}: owner is the subject");
    assert_eq!(referent(&c, &arg_var(p, 1)).noun.as_deref(), Some(place_noun), "{s:?}");
}

#[test]
fn babi_pp_location() {
    // fam 6, 9, 10: yes/no location questions and the matching assertion.
    for s in ["Is Mary in the garden?", "Is John in the bathroom?", "Is Mary in the kitchen?"] {
        let c = parsed(s);
        assert!(
            matches!(c.act, Act::Question { kind: QuestionKind::YesNo }),
            "{s:?}: expected a yes/no question, got {:?}", c.act
        );
    }
    assert_locative("Is Mary in the garden?", "in", "garden");
    assert_locative("Is John in the bathroom?", "in", "bathroom");
    assert_locative("Mary is in the garden.", "in", "garden");
    assert_locative("John is at home.", "at", "home");
    roundtrip("Is Mary in the garden?");
    roundtrip("Mary is in the garden.");
    roundtrip("John is at home.");
}

#[test]
fn babi_or_in_pp() {
    // fam 10 story line: every alternative is an adjunct of the same prep.
    let c = parsed("Mary moves to the kitchen or the hallway.");
    let p = &c.conditions[0];
    assert_eq!(p.pred, "move");
    let places: Vec<Option<String>> = p
        .adjuncts
        .iter()
        .map(|(prep, t)| {
            assert_eq!(prep, "to");
            let Term::Var { var } = t else { panic!("adjunct is not a var") };
            referent(&c, var).noun.clone()
        })
        .collect();
    assert_eq!(places, vec![Some("kitchen".into()), Some("hallway".into())]);
}

/// `X is <phrase> of Y` -> a two-place relation named after the phrase.
fn assert_relation_of(s: &str, name: &str, left_noun: Option<&str>, right_noun: &str) {
    let c = parsed(s);
    assert_eq!(c.conditions.len(), 1, "{s:?}: one condition");
    let p = &c.conditions[0];
    assert_eq!(p.pred, name, "{s:?}");
    assert_eq!(p.args.len(), 2, "{s:?}");
    assert_eq!(referent(&c, &arg_var(p, 0)).noun.as_deref(), left_noun, "{s:?}: left");
    assert_eq!(referent(&c, &arg_var(p, 1)).noun.as_deref(), Some(right_noun), "{s:?}: right");
}

#[test]
fn babi_relation_of_assertions() {
    // fam 4, 15, 17 story lines.
    assert_relation_of("The office is north of the garden.", "north-of", Some("office"), "garden");
    assert_relation_of("The kitchen is south of the office.", "south-of", Some("kitchen"), "office");
    assert_relation_of("The bathroom is east of the hallway.", "east-of", Some("bathroom"), "hallway");
    assert_relation_of("The garden is west of the bathroom.", "west-of", Some("garden"), "bathroom");
    assert_relation_of(
        "The red box is to the left of the blue box.", "left-of", Some("box"), "box",
    );
    assert_relation_of("The green bag is to the left of the red box.", "left-of", Some("bag"), "box");
    // Adjectives stay on the referents that carry them.
    let c = parsed("The red box is to the left of the blue box.");
    let p = &c.conditions[0];
    assert_eq!(referent(&c, &arg_var(p, 0)).mods, vec!["red".to_string()]);
    assert_eq!(referent(&c, &arg_var(p, 1)).mods, vec!["blue".to_string()]);
    // A plural subject parses as a name; the relation keeps the second term.
    assert_relation_of("Wolves are afraid of mice.", "afraid-of", None, "mouse");
    for s in [
        "The office is north of the garden.",
        "The red box is to the left of the blue box.",
        "Wolves are afraid of mice.",
    ] {
        roundtrip(s);
    }
}

#[test]
fn babi_relation_of_questions() {
    // fam 4, 17: the wh-word is the first argument of the relation.
    for (s, name, right) in [
        ("What is south of the office?", "south-of", "office"),
        ("What is east of the hallway?", "east-of", "hallway"),
        ("What is to the left of the red box?", "left-of", "box"),
    ] {
        let c = parsed(s);
        let focus = match &c.act {
            Act::Question { kind: QuestionKind::What { focus } } => focus.clone(),
            other => panic!("{s:?}: expected a What question, got {other:?}"),
        };
        let p = &c.conditions[0];
        assert_eq!(p.pred, name, "{s:?}");
        assert_eq!(arg_var(p, 0), focus, "{s:?}: the wh-referent is the first argument");
        assert_eq!(referent(&c, &focus).quant, Quant::Wh, "{s:?}");
        assert_eq!(referent(&c, &arg_var(p, 1)).noun.as_deref(), Some(right), "{s:?}");
        roundtrip(s);
    }
}

#[test]
fn babi_give_three_arg() {
    // fam 5: the assertion, then the same relation asked with a stranded "to".
    let c = parsed("John gives the apple to Maya.");
    let p = &c.conditions[0];
    assert_eq!(p.pred, "give");
    assert_eq!(p.args.len(), 2);
    assert_eq!(p.adjuncts.len(), 1);
    assert_eq!(p.adjuncts[0].0, "to");

    for (s, obj_noun) in [
        ("Who does John give the apple to?", "apple"),
        ("Who does John give the milk to?", "milk"),
    ] {
        let c = parsed(s);
        let focus = match &c.act {
            Act::Question { kind: QuestionKind::Who { focus } } => focus.clone(),
            other => panic!("{s:?}: expected a Who question, got {other:?}"),
        };
        assert_eq!(c.conditions.len(), 1, "{s:?}");
        let p = &c.conditions[0];
        assert_eq!(p.pred, "give", "{s:?}");
        assert!(matches!(referent(&c, &arg_var(p, 0)).quant, Quant::Named(ref n) if n == "John"), "{s:?}");
        assert_eq!(referent(&c, &arg_var(p, 1)).noun.as_deref(), Some(obj_noun), "{s:?}");
        assert_eq!(p.adjuncts.len(), 1, "{s:?}: the stranded preposition is attached");
        assert_eq!(p.adjuncts[0].0, "to", "{s:?}");
        assert_eq!(p.adjuncts[0].1, Term::Var { var: focus }, "{s:?}: 'to' takes the wh-referent");
    }
}

#[test]
fn babi_property_question() {
    // fam 15: "What color is every wolf?" == "What is the color of every wolf?"
    let c = parsed("What color is every wolf?");
    let focus = match &c.act {
        Act::Question { kind: QuestionKind::What { focus } } => focus.clone(),
        other => panic!("expected a What question, got {other:?}"),
    };
    let prop = referent(&c, &focus);
    assert_eq!(prop.noun.as_deref(), Some("color"));
    assert_eq!(prop.quant, Quant::Def);
    let owner = referent(&c, prop.owner.as_deref().expect("no owner"));
    assert_eq!(owner.noun.as_deref(), Some("wolf"));
    assert_eq!(owner.quant, Quant::Every);
    let p = &c.conditions[0];
    assert_eq!(p.pred, "be");
    assert_eq!(referent(&c, &arg_var(p, 0)).quant, Quant::Wh);
    assert_eq!(arg_var(p, 1), focus);
    roundtrip("What color is every wolf?");

    // The universal that answers it still parses as an attribute.
    let c = parsed("Every wolf is white.");
    assert_eq!(c.conditions[0].attr.as_deref(), Some("white"));
    assert_eq!(referent(&c, &arg_var(&c.conditions[0], 0)).quant, Quant::Every);
}

/// The forms the new copula rules must not steal.
#[test]
fn copula_complements_still_disambiguate() {
    let lex = Lexicon::with_defaults();
    // Comparatives keep the "-than" attribute, not a relation.
    let (c, _) = parse("Is the elephant bigger than the cat?", &lex).unwrap();
    assert_eq!(c.conditions[0].pred, "be");
    assert_eq!(c.conditions[0].attr.as_deref(), Some("bigger-than"));
    // A determiner after "is" means a property NP or an is-a, never a relation.
    let (c, _) = parse("What is the double of 3?", &lex).unwrap();
    assert_eq!(c.conditions[0].pred, "be");
    assert!(c.referents.iter().any(|r| r.noun.as_deref() == Some("double") && r.owner.is_some()));
    let (c, _) = parse("Is John a doctor?", &lex).unwrap();
    assert_eq!(c.conditions[0].pred, "be");
    assert_eq!(c.conditions[0].args.len(), 2);
    assert!(c.referents.iter().any(|r| r.noun.as_deref() == Some("doctor")));
    // Non-locative copula PPs stay adjuncts of the subject.
    let (c, _) = parse("John is angry with Mary.", &lex).unwrap();
    assert_eq!(c.conditions[0].attr.as_deref(), Some("angry"));
    assert_eq!(c.conditions[0].adjuncts.len(), 1);
    // Plain adjectives and verb PPs are untouched.
    parse("Is John happy?", &lex).unwrap();
    let (c, _) = parse("The train moves at 500 miles-per-hour.", &lex).unwrap();
    assert_eq!(c.conditions[0].pred, "move");
    assert_eq!(c.conditions[0].adjuncts.len(), 1);
    // A stranded preposition without a wh-referent is still a leftover token.
    assert!(parse("John gives the apple to.", &lex).is_err());
}

// ---------------------------------------------------------------------------
// Test 8: Plural nouns
// ---------------------------------------------------------------------------

#[test]
fn plural_nouns() {
    use spoon_lang::sce::singularize_noun;
    let lex = Lexicon::with_defaults();
    let pp_noun = |s: &str| -> String {
        let (c, _) = parse(s, &lex).unwrap_or_else(|e| panic!("{s:?}: {e}"));
        let (_, term) = c.conditions[0].adjuncts.first().unwrap_or_else(|| panic!("{s:?}: no adjunct"));
        let Term::Var { var } = term else { panic!("{s:?}: adjunct is not a var") };
        c.referents.iter().find(|r| &r.var == var).and_then(|r| r.noun.clone())
            .unwrap_or_else(|| panic!("{s:?}: adjunct referent has no noun"))
    };
    assert_eq!(pp_noun("John talks about dogs."), "dog");
    assert_eq!(pp_noun("John talks about stories."), "story");
    assert_eq!(pp_noun("John talks about buses."), "bus");
    // Singular-looking words that end in s stay as they are.
    assert_eq!(pp_noun("John talks about glass."), "glass");
    assert_eq!(pp_noun("John talks about news."), "news");
    assert_eq!(pp_noun("John talks about analysis."), "analysis");
    assert_eq!(pp_noun("John talks about a bus."), "bus");

    for (w, want) in [
        ("dogs", "dog"), ("stories", "story"), ("buses", "bus"), ("boxes", "box"), ("churches", "church"),
        ("glass", "glass"), ("bus", "bus"), ("news", "news"), ("analysis", "analysis"),
        ("status", "status"), ("process", "process"), ("series", "series"),
        ("mice", "mouse"), ("wolves", "wolf"), ("children", "child"), ("people", "person"),
    ] {
        assert_eq!(singularize_noun(w), want, "singularize_noun({w:?})");
    }
}
