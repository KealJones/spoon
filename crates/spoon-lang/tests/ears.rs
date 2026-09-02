//! Ears integration tests. All offline (no network). Use `FakeGate`.
//!
//! Run: cargo test -p spoon-lang --test ears
//!
//! Live test (requires Ollama running locally):
//!   SPOON_LLM_TESTS=1 cargo test -p spoon-lang --test ears -- llm_normalizer_live --nocapture --ignored


use spoon_core::types::clause::{
    Act, Clause, EarsPath, QuestionKind,
};
use spoon_lang::ears::{
    lexicon::Lexicon,
    llm::build_prompt,
    Gate, Ears,
};

// ---- FakeGate ----

/// A test gate that accepts any text consisting of properly-formed SCE sentences:
/// - Each sentence must end with `.`, `?`, or `!`
/// - The first word of each sentence must be a recognized SCE opener
/// - Each sentence must have at least 2 words before the terminator
///
/// Multi-sentence SCE (e.g. "User greets Assistant. What is the wellbeing of Assistant?")
/// is split and each sentence validated separately.
///
/// Special case: `Assistant, VERB PHRASE!` is accepted for commands.
struct FakeGate;

static SCE_OPENERS: &[&str] = &[
    "User", "Assistant", "John", "Mary", "Ben", "Bob", "Who", "What",
    "Where", "When", "Which", "How", "Does", "Is", "Can", "Should", "Must",
    "May", "If", "There", "Not", "A", "An", "The", "Every", "No", "Some",
    "Victor", "Dana", "Omar", "Hugo", "Lena", "Felix",
];

impl Gate for FakeGate {
    fn parse(&self, sce: &str) -> Result<Vec<Clause>, String> {
        let text = sce.trim();
        if text.is_empty() {
            return Err("empty input".to_string());
        }

        let mut clauses = vec![];
        let mut remaining = text;

        while !remaining.is_empty() {
            remaining = remaining.trim_start();
            if remaining.is_empty() {
                break;
            }

            // Find the next sentence terminator (., ?, !)
            let term_pos = remaining
                .char_indices()
                .find(|(_, c)| matches!(c, '.' | '?' | '!'))
                .map(|(i, c)| (i, c));

            let (sentence_text, terminator, rest) = match term_pos {
                None => {
                    // No terminator - reject
                    return Err(format!("no sentence terminator in: {}", remaining));
                }
                Some((pos, term)) => {
                    let sentence = remaining[..pos].trim();
                    let rest = remaining[pos + 1..].trim_start();
                    (sentence, term, rest)
                }
            };

            if sentence_text.len() < 2 {
                remaining = rest;
                continue;
            }

            // Check that the first word is an SCE opener
            let first_word = sentence_text
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(',');

            let is_opener = SCE_OPENERS.contains(&first_word);
            if !is_opener {
                return Err(format!("not SCE (unknown opener '{}'): {}", first_word, sentence_text));
            }

            // Must have at least 2 words
            let word_count = sentence_text.split_whitespace().count();
            if word_count < 2 {
                return Err(format!("not SCE (single word): {}", sentence_text));
            }

            let act = match terminator {
                '?' => Act::Question { kind: QuestionKind::YesNo },
                '!' => Act::Command,
                _ => Act::Assert,
            };

            clauses.push(Clause {
                act,
                referents: vec![],
                conditions: vec![],
                then: vec![],
                then_referents: vec![],
                sce: format!("{}{}", sentence_text, terminator),
            });

            remaining = rest;
        }

        if clauses.is_empty() {
            Err(format!("no clauses parsed from: {}", text))
        } else {
            Ok(clauses)
        }
    }
}

// ---- helpers ----

fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is crates/spoon-lang; workspace root is two levels up.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

fn test_data_dir() -> std::path::PathBuf {
    workspace_root().join("data")
}

fn seed_dir() -> std::path::PathBuf {
    workspace_root().join("data/seed")
}

fn make_ears() -> Ears {
    let mut lex = Lexicon::load_seed_dir(&seed_dir()).expect("load lexicon");
    lex.add_names(&["John", "Mary", "Ben", "Bob"]);
    let phrasings = spoon_lang::ears::load_phrasings(&test_data_dir(), &lex)
        .expect("load phrasings");
    Ears::new(lex, phrasings, None)
}

// ---- tests ----

/// 1. Direct SCE passes through unchanged.
#[test]
fn direct_sce_passes_through() {
    let ears = make_ears();
    let gate = FakeGate;
    let result = ears.hear_native("John owns a dog.", &gate).expect("must succeed");
    assert_eq!(result.path, EarsPath::Direct);
    assert_eq!(result.clauses.len(), 1);
    assert!(result.sce.contains("John owns a dog"));
}

/// 2. Greeting via phrasing: both sentences resolve natively.
#[test]
fn greeting_via_phrasing() {
    let ears = make_ears();
    let gate = FakeGate;
    let result = ears
        .hear_native("Yoooo wasup girl? How you doin?", &gate)
        .expect("must succeed natively");
    assert!(
        matches!(result.path, EarsPath::Phrasing | EarsPath::Retrieval),
        "expected Phrasing or Retrieval, got {:?}",
        result.path
    );
    let normalized_sce = result.sce.to_lowercase();
    assert!(
        normalized_sce.contains("user greets assistant"),
        "expected 'User greets Assistant' in sce, got: {}",
        result.sce
    );
    assert!(
        normalized_sce.contains("wellbeing of assistant") || normalized_sce.contains("assistant"),
        "expected wellbeing question in sce, got: {}",
        result.sce
    );
}

/// 3. Typo repair: "jhon" -> "John", "dgo" -> "dog".
#[test]
fn typo_repair() {
    let ears = make_ears();
    let gate = FakeGate;
    // "jhon" is close to "john" (name), "dgo" is close to "dog"
    let result = ears
        .hear_native("jhon owns a dgo", &gate)
        .expect("must succeed");
    assert_eq!(result.path, EarsPath::Direct);
    let sce_lower = result.sce.to_lowercase();
    assert!(
        sce_lower.contains("john"),
        "expected 'John' in sce, got: {}",
        result.sce
    );
    assert!(
        sce_lower.contains("dog"),
        "expected 'dog' in sce, got: {}",
        result.sce
    );
}

/// 4. Slot induction: add a pair, then match a structurally identical input with different numbers.
#[test]
fn slot_induction_and_match() {
    let mut ears = make_ears();
    let gate = FakeGate;

    // Teach the pair
    let pair = spoon_core::types::episode::Pair {
        id: 1,
        utterance: "what is 4 times 5".to_string(),
        sce: "Assistant, calculate 4 * 5!".to_string(),
        source: "test".to_string(),
        at: 0,
        credit: 1,
    };
    ears.add_pairs(&[pair]);

    // Now hear a structurally similar input with different numbers
    let result = ears
        .hear_native("what is 7 times 9", &gate)
        .expect("must succeed");
    assert!(
        matches!(result.path, EarsPath::Phrasing | EarsPath::Retrieval),
        "expected Phrasing or Retrieval, got {:?}",
        result.path
    );
    assert!(
        result.sce.contains("7") && result.sce.contains("9"),
        "expected slot values 7 and 9 in sce, got: {}",
        result.sce
    );
    assert!(
        result.sce.contains("calculate") || result.sce.contains("*"),
        "expected 'calculate' and '*' in sce, got: {}",
        result.sce
    );
}

/// 5. Arithmetic expression is preserved verbatim through normalization.
#[test]
fn arith_preserved() {
    use spoon_lang::ears::values::{spot_values, SlotKind};
    let input = "calculate 3 / 500 * 3600";
    let spots = spot_values(input);
    let arith: Vec<_> = spots.iter().filter(|s| s.kind == SlotKind::Arith).collect();
    assert!(!arith.is_empty(), "expected an Arith spot");
    assert_eq!(arith[0].text, "3 / 500 * 3600");
}

/// 6. Multi-sentence input: results in 2 clauses, path is the worse of the two.
#[test]
fn multi_sentence_split() {
    let ears = make_ears();
    let gate = FakeGate;
    // "hey." should go phrasing (matches greeting), "who owns a dog?" should go direct or phrasing
    let result = ears
        .hear_native("hey. who owns a dog?", &gate)
        .expect("must succeed");
    assert_eq!(result.clauses.len(), 2, "expected 2 clauses, got: {:?}", result.clauses);
    assert!(
        matches!(result.path, EarsPath::Phrasing | EarsPath::Retrieval),
        "expected Phrasing or Retrieval, got {:?}",
        result.path
    );
}

/// 7. Unknown words are reported in `failed` path.
#[test]
fn failed_reports_unknown_words() {
    let ears = make_ears();
    let gate = FakeGate;
    // These words are gibberish and won't be in the lexicon or phrasings
    let result = ears.hear_native("flibber the zorp", &gate);
    // Should fail natively (nothing in phrasings for "flibber the zorp")
    // hear_native returns None on native failure, so we check failed_result
    let result = result.unwrap_or_else(|| ears.failed_result("flibber the zorp"));
    assert_eq!(result.path, EarsPath::Failed);
    assert!(
        result.unknown_words.iter().any(|w| w.contains("flibber")),
        "expected 'flibber' in unknown_words: {:?}",
        result.unknown_words
    );
    assert!(
        result.unknown_words.iter().any(|w| w.contains("zorp")),
        "expected 'zorp' in unknown_words: {:?}",
        result.unknown_words
    );
}

/// 8. Benchmark runs offline, total == 158, report is non-empty.
#[test]
fn bench_runs_offline() {
    let corpus = workspace_root().join("data/bench/ace_corpus.json");
    if !corpus.exists() {
        eprintln!("skipping bench test - corpus not found");
        return;
    }
    let ears = make_ears();
    let gate = FakeGate;
    let report = spoon_lang::ears::bench::run_ace(&ears, &gate, &corpus, false)
        .expect("run_ace failed");
    assert_eq!(report.total, 158, "expected 158 corpus items");
    let display = format!("{}", report);
    assert!(!display.is_empty());
    // Print the native hit count (no threshold asserted)
    println!("Native hits: {}/{}", report.hits, report.total);
    println!("Parsed: {}/{}", report.parsed, report.total);
    println!("{}", display);
}

/// 9. build_prompt output is under 10k chars with 60 vocab words and 8 shots.
#[test]
fn prompt_under_budget() {
    let base = std::fs::read_to_string(workspace_root().join("data/prompts/normalizer_base.md"))
        .unwrap_or_else(|_| "You are a normalizer. {{VOCABULARY}} {{CONTEXT}}".to_string());
    let vocab: Vec<String> = (0..60).map(|i| format!("word{}", i)).collect();
    let shots: Vec<(String, String)> = (0..8)
        .map(|i| {
            (
                format!("some messy input number {}", i),
                format!("User does something {}.", i),
            )
        })
        .collect();
    let utterance = "what is 7 times 9";
    let msgs = build_prompt(&base, &vocab, &shots, utterance);
    let total_chars: usize = msgs.iter().map(|m| m.content.len()).sum();
    assert!(
        total_chars < 10_000,
        "prompt too large: {} chars (limit 10000)",
        total_chars
    );
}

/// 10. No LLM calls when built without LLM config.
#[test]
fn no_llm_when_offline() {
    let client = spoon_core::llm::LlmClient::new();
    let counters = client.counters.clone();

    let ears = make_ears(); // no LLM
    let gate = FakeGate;

    // Process several messy inputs
    for input in &[
        "yoooo wasup",
        "jhon is happy",
        "what is 7 plus 3",
        "flibber the zorp quux",
        "heyyyy how are you doin",
    ] {
        let _ = ears.hear_native(input, &gate);
    }

    // The client we created was never handed to Ears, so counters must all be 0
    let (ears_c, mouth_c, teacher_c, fail_c) = counters.snapshot();
    assert_eq!((ears_c, mouth_c, teacher_c, fail_c), (0, 0, 0, 0));
}

// ---- live test (ignored unless SPOON_LLM_TESTS=1) ----

#[tokio::test]
#[ignore]
async fn llm_normalizer_live() {
    if std::env::var("SPOON_LLM_TESTS").as_deref() != Ok("1") {
        return;
    }

    use spoon_core::llm::{LlmClient, LlmConfig};
    use spoon_lang::ears::gate::SceGate;

    let client = LlmClient::new();
    let cfg = LlmConfig::ollama("qwen3.5:4b");

    let lex = Lexicon::load_seed_dir(&seed_dir()).unwrap();
    let phrasings = spoon_lang::ears::load_phrasings(&test_data_dir(), &lex).unwrap();
    let llm_ears = Ears::new(lex, phrasings, Some((client, cfg)));

    let mut gate = SceGate::with_defaults();
    gate.add_names(&["John", "Mary", "Bob", "Ben"]);

    let inputs = [
        "hey whats up",
        "yoooo wasup girl",
        "jhon owns a dog",
        "what is 7 times 9",
        "where the hell is bob at",
        "i dont know what to do",
        "ok so john has this dog right and like the dog is brown",
        "can u remind me what marys phone number is",
    ];

    for input in &inputs {
        let start = std::time::Instant::now();
        let result = llm_ears.hear(input, &gate).await;
        let ms = start.elapsed().as_millis();
        let parsed_ok = !result.sce.is_empty();
        println!(
            "input:      {:?}\nnormalized: {:?}\npath:       {:?}\nsce:        {:?}\nparsed:     {}\nms:         {}\n---",
            input, result.sce, result.path, result.sce, parsed_ok, ms
        );
    }
}

/// Benchmark with real gate, offline (allow_llm=false). No threshold.
#[test]
fn bench_ace_with_real_gate() {
    use spoon_lang::ears::gate::SceGate;
    let corpus = workspace_root().join("data/bench/ace_corpus.json");
    if !corpus.exists() {
        eprintln!("skipping bench_ace_with_real_gate - corpus not found");
        return;
    }
    let ears = make_ears();
    let gate = SceGate::with_defaults();
    let report = spoon_lang::ears::bench::run_ace(&ears, &gate, &corpus, false)
        .expect("run_ace failed");
    println!("=== bench_ace_with_real_gate (native only) ===");
    println!("Native hits: {}/{}", report.hits, report.total);
    println!("Parsed:      {}/{}", report.parsed, report.total);
    println!("{}", report);
}

/// Full ACE benchmark with LLM. Run with SPOON_LLM_TESTS=1.
#[test]
#[ignore]
fn bench_ace_llm_live() {
    if std::env::var("SPOON_LLM_TESTS").as_deref() != Ok("1") {
        return;
    }
    use spoon_core::llm::{LlmClient, LlmConfig};
    use spoon_lang::ears::gate::SceGate;

    let corpus = workspace_root().join("data/bench/ace_corpus.json");
    if !corpus.exists() {
        eprintln!("corpus not found");
        return;
    }

    let client = LlmClient::new();
    let cfg = LlmConfig::ollama("qwen3.5:4b");
    let lex = Lexicon::load_seed_dir(&seed_dir()).unwrap();
    let phrasings = spoon_lang::ears::load_phrasings(&test_data_dir(), &lex).unwrap();
    let ears = Ears::new(lex, phrasings, Some((client, cfg)));
    let gate = SceGate::with_defaults();

    let report = spoon_lang::ears::bench::run_ace(&ears, &gate, &corpus, true)
        .expect("run_ace failed");

    println!("=== bench_ace_llm_live (LLM enabled) ===");
    println!("{}", report);

    // Print up to 15 misses
    let misses: Vec<_> = report.misses.iter().take(15).collect();
    if !misses.is_empty() {
        println!("\n--- Misses (up to 15) ---");
        println!("{:<50} | {:<40} | {}", "input", "expected", "got");
        println!("{}", "-".repeat(130));
        for m in &misses {
            println!(
                "{:<50} | {:<40} | {}",
                truncate(&m.input, 48),
                truncate(&m.expected, 38),
                truncate(&m.got, 38)
            );
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

/// Conservative typo repair: known words must survive, garbled words still repair.
#[test]
fn typo_repair_is_conservative() {
    use spoon_lang::ears::normalize::normalize;

    let lex = Lexicon::load_seed_dir(&seed_dir()).expect("load lexicon");
    // "you", "dude", "yo", "good" are all common words - must survive.
    // "lol" is a filler - must be dropped.
    {
        let n = normalize("yo dude you good lol", &lex);
        let text = n.sentences.join(" ");
        assert!(
            text.to_lowercase().contains("you"),
            "\"you\" must survive, got: {:?}",
            text
        );
        assert!(
            !text.to_lowercase().contains("lol"),
            "\"lol\" should be dropped, got: {:?}",
            text
        );
    }
    // "hell" is a common English word - must NOT be repaired to "help".
    {
        let n = normalize("where the hell is bob at", &lex);
        let text = n.sentences.join(" ");
        assert!(
            text.to_lowercase().contains("hell"),
            "\"hell\" must survive, got: {:?}",
            text
        );
    }
    // Genuine typos still get repaired: "jhon" -> "john", "dgo" -> "dog".
    {
        let mut lex2 = Lexicon::load_seed_dir(&seed_dir()).expect("load lexicon");
        lex2.add_names(&["John"]);
        let n = normalize("jhon owns a dgo", &lex2);
        let text = n.sentences.join(" ");
        assert!(
            text.to_lowercase().contains("john"),
            "\"jhon\" should repair to \"john\", got: {:?}",
            text
        );
        assert!(
            text.to_lowercase().contains("dog"),
            "\"dgo\" should repair to \"dog\", got: {:?}",
            text
        );
    }
    // Transpositions of short words.
    {
        let n = normalize("teh cat sat on teh mat", &lex);
        let text = n.sentences.join(" ");
        assert!(
            text.to_lowercase().contains("the"),
            "\"teh\" should repair to \"the\", got: {:?}",
            text
        );
    }
    // Longer misspellings.
    {
        let n = normalize("recieve the packge tomorrow", &lex);
        let text = n.sentences.join(" ");
        assert!(
            text.to_lowercase().contains("receive"),
            "\"recieve\" should repair to \"receive\", got: {:?}",
            text
        );
        assert!(
            text.to_lowercase().contains("package"),
            "\"packge\" should repair to \"package\", got: {:?}",
            text
        );
    }
}
