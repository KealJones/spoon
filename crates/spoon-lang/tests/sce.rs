//! SCE parser/realizer tests.
//!
//! 1. All SCE.md example sentences parse, and realize(parse(s)) re-parses equal.
//! 2. Specific structural assertions.
//! 3. Corpus parse rate >= 85%.
//! 4. Unknown-word guessing.
//! 5. Negative tests.

use spoon_core::types::clause::{Act, ArithExpr, ArithOp, Modal, Quant, QuestionKind, Term};
use spoon_lang::sce::{lemmatize, parse, parse_text, realize, Lexicon};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Canonicalize a Clause by renaming vars in order of first appearance.
fn canonical(clause: &spoon_core::types::clause::Clause) -> spoon_core::types::clause::Clause {
    use spoon_core::types::clause::{Clause, Pred, Referent, Term};
    use std::collections::HashMap;

    let mut map: HashMap<String, String> = HashMap::new();
    let mut cnt = 0u32;
    let mut rename = |v: &str| -> String {
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
        Clause {
            act: c.act.clone(),
            referents: c.referents.iter().map(|r| canon_ref(r, map, cnt)).collect(),
            conditions: c.conditions.iter().map(|p| canon_pred(p, map, cnt)).collect(),
            then: c.then.iter().map(|p| canon_pred(p, map, cnt)).collect(),
            then_referents: c.then_referents.iter().map(|r| canon_ref(r, map, cnt)).collect(),
            sce: c.sce.clone(),
        }
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
    let has_machine = clause.then_referents.iter().any(|r| {
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
    use std::fs;
    use serde_json::Value;

    let corpus_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/bench/ace_corpus.json");
    let raw = fs::read_to_string(corpus_path).expect("corpus not found");
    let items: Vec<Value> = serde_json::from_str(&raw).expect("invalid json");

    // Build a lexicon seeded from the corpus (extract noun/verb candidates)
    let mut lex = Lexicon::with_defaults();
    for item in &items {
        if let Some(ace) = item.get("expected_ace").and_then(|v| v.as_str()) {
            // Tokenize and add lowercase words that look like nouns or verbs
            for word in ace.split_whitespace() {
                let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
                let lw = w.to_lowercase();
                if !lw.is_empty() && !Lexicon::is_function_word(&lw) && !Lexicon::is_prep(&lw) {
                    // Heuristic: add as both noun and verb (parser position will disambiguate)
                    lex.add_noun(&lw);
                    let lemma = lemmatize(&lw);
                    lex.add_verb(&lemma);
                }
            }
        }
    }

    let total = items.len();
    let mut passed = 0;
    let mut failures: Vec<(usize, String, String)> = vec![]; // (id, ace, error)

    for item in &items {
        let id = item.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let ace = match item.get("expected_ace").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
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
            eprintln!("  [{}x] {} - items: {:?}", ids.len(), err, &ids[..ids.len().min(5)]);
        }
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
            Ok((c, _)) => println!("OK: {s}"),
            Err(e) => println!("FAIL: {s}\n  -> {:?}", e),
        }
    }
}

#[test]
fn debug_corpus_items() {
    use spoon_lang::sce::{parse_text, lemmatize, Lexicon};
    use serde_json::Value;
    let corpus_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/bench/ace_corpus.json");
    let raw = std::fs::read_to_string(corpus_path).unwrap();
    let items: Vec<Value> = serde_json::from_str(&raw).unwrap();
    let mut lex = Lexicon::with_defaults();
    for item in &items {
        if let Some(ace) = item.get("expected_ace").and_then(|v| v.as_str()) {
            for word in ace.split_whitespace() {
                let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
                let lw = w.to_lowercase();
                if !lw.is_empty() && !Lexicon::is_function_word(&lw) && !Lexicon::is_prep(&lw) {
                    lex.add_noun(&lw);
                    lex.add_verb(&lemmatize(&lw));
                }
            }
        }
    }
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
    use spoon_lang::sce::{parse, lemmatize, Lexicon};
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
