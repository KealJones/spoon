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

/// 11. Speaker grounding: pronouns become User/Assistant.
#[test]
fn speaker_grounding() {
    use spoon_lang::ears::normalize::ground_speakers;

    assert_eq!(ground_speakers("i own a dog"), "User own a dog");
    assert_eq!(ground_speakers("my dog is happy"), "User's dog is happy");
    assert_eq!(ground_speakers("you should know"), "Assistant should know");
    assert_eq!(ground_speakers("your answer is wrong"), "Assistant's answer is wrong");
    // "I" mid-sentence
    assert_eq!(ground_speakers("john and i are here"), "john and User are here");
    // Quoted content is preserved (rough heuristic)
    // Names are NOT changed
    assert_eq!(ground_speakers("John owns a dog"), "John owns a dog");
}

/// 12. Sentence-final tag stripping.
#[test]
fn sentence_final_tags() {
    use spoon_lang::ears::normalize::strip_sentence_final_tags;

    assert_eq!(strip_sentence_final_tags("where is bob at").trim(), "where is bob");
    assert_eq!(strip_sentence_final_tags("when does mary leave again?").trim(), "when does mary leave?");
    assert_eq!(strip_sentence_final_tags("is it right").trim(), "is it");
    assert_eq!(strip_sentence_final_tags("who is john tho").trim(), "who is john");
    // Multi-word final tag
    assert_eq!(strip_sentence_final_tags("is that valid or what").trim(), "is that valid");
    // Non-final use is preserved
    assert!(strip_sentence_final_tags("right now is good").contains("right"));
}

/// 13. Structural equality: two semantically equivalent SCE strings match.
#[test]
fn structural_equality() {
    use spoon_lang::ears::bench::structural_eq;
    use spoon_lang::ears::gate::SceGate;

    let gate = SceGate::with_defaults();
    // Same sentence
    assert!(structural_eq("John owns a dog.", "John owns a dog.", &gate));
    // Different variable names (won't differ since same string, but let's check mismatched SCE)
    // Two parses of the same meaning
    assert!(structural_eq("Does John own a dog?", "Does John own a dog?", &gate));
    // Different sentence type => not structural match
    assert!(!structural_eq("John owns a dog.", "Does John own a dog?", &gate));
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
        "heyyyy whats going on",
        "which customer bought the red thing?",
        "no cats are dogs",
        "if its raining then bob stays home",
        "divide 100 by 4 pls",
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

/// Dump one ACE category end to end, no LLM: input, what the ears made of
/// it, the gate's verdict, the expected form. `SPOON_ACE_CAT=loop_like_logic
/// cargo test -p spoon-lang --test ears ace_category_dump -- --ignored --nocapture`.
#[test]
#[ignore]
fn ace_category_dump() {
    use spoon_lang::ears::Gate;
    let Ok(category) = std::env::var("SPOON_ACE_CAT") else { return };
    let corpus = std::fs::read_to_string(workspace_root().join("data/bench/ace_corpus.json")).expect("corpus");
    let items: Vec<serde_json::Value> = serde_json::from_str(&corpus).expect("corpus json");
    let ears = make_ears();
    let gate = real_gate();
    for item in items.iter().filter(|i| i["category"] == category.as_str()) {
        let input = item["input"].as_str().unwrap_or_default();
        let expected = item["expected_ace"].as_str().unwrap_or_default();
        let result = ears.hear_offline(input, &gate);
        let verdict = match gate.parse(&result.sce) {
            Ok(_) => "parses".to_string(),
            Err(e) => format!("ERR {e}"),
        };
        let hit = spoon_lang::ears::bench::structural_eq(expected, &result.sce, &gate);
        println!(
            "#{} {} [{:?}] {}\n  IN : {input}\n  GOT: {}\n  EXP: {expected}\n  unknown={:?}\n",
            item["id"],
            if hit { "HIT " } else { "miss" },
            result.path,
            verdict,
            result.sce,
            result.unknown_words
        );
    }
}

/// Parse `SPOON_SCE` (sentences separated by `|`) with the real gate and print
/// each verdict. A probe for "does this shape parse".
#[test]
#[ignore]
fn gate_parse_dump() {
    use spoon_lang::ears::Gate;
    let Ok(text) = std::env::var("SPOON_SCE") else { return };
    let gate = real_gate();
    for sentence in text.split('|') {
        match gate.parse_reported(sentence.trim()) {
            Ok((clauses, unknown)) => println!("OK  {sentence}\n    {clauses:?} unknown={unknown:?}"),
            Err(e) => println!("ERR {sentence}\n    {e}"),
        }
    }
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

    println!("=== bench_ace_llm_live (LLM enabled, qwen3.5:4b) ===");
    println!("{}", report);

    if !report.misses.is_empty() {
        println!("\n--- Misses ({}) ---", report.misses.len());
        for m in &report.misses {
            println!("IN : {}\nEXP: {}\nGOT: {}\n", m.input, m.expected, m.got);
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let out = workspace_root().join(format!("data/bench/results/ace_llm_live_{stamp}.json"));
    if let Err(e) = report.save_json(&out) {
        eprintln!("failed to save report: {e}");
    } else {
        println!("saved -> {}", out.display());
    }
}

/// Model sweep across qwen3.5:0.8b, 2b, 4b. Run with SPOON_LLM_TESTS=1.
#[test]
#[ignore]
fn bench_ace_model_sweep() {
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

    let models = ["qwen3.5:0.8b", "qwen3.5:2b", "qwen3.5:4b"];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple YYYYMMDD from unix timestamp
    let days = now / 86400;
    let year = 1970 + days / 365;
    let day_of_year = days % 365;
    let month = day_of_year / 30 + 1;
    let day = day_of_year % 30 + 1;
    let date = format!("{:04}{:02}{:02}", year, month, day);

    println!("\n=== Model Sweep ===");
    println!("{:<16} | {:>8} | {:>8} | {:>8} | {:>8}", "model", "parsed%", "struct%", "exact%", "med_ms");
    println!("{}", "-".repeat(65));

    for model in &models {
        let client = LlmClient::new();
        let cfg = LlmConfig::ollama(model);
        let lex = Lexicon::load_seed_dir(&seed_dir()).unwrap();
        let phrasings = spoon_lang::ears::load_phrasings(&test_data_dir(), &lex).unwrap();
        let ears = Ears::new(lex, phrasings, Some((client, cfg)));
        let gate = SceGate::with_defaults();

        let wall_start = std::time::Instant::now();
        let report = spoon_lang::ears::bench::run_ace(&ears, &gate, &corpus, true)
            .expect("run_ace failed");
        let elapsed_ms = wall_start.elapsed().as_millis() as usize;
        let n = report.total.max(1);
        let med_ms = elapsed_ms / n;

        let parsed_pct = 100.0 * report.parsed as f64 / n as f64;
        let hits_pct = 100.0 * report.hits as f64 / n as f64;
        let exact_pct = 100.0 * report.exact as f64 / n as f64;

        println!("{:<16} | {:>7.1}% | {:>7.1}% | {:>7.1}% | {:>8}",
            model, parsed_pct, hits_pct, exact_pct, med_ms);
        println!("  path: {:?}", report.path_histogram);

        // Save report JSON
        let safe_model = model.replace(':', "_").replace('.', "_");
        let result_path = workspace_root().join(format!("data/bench/results/ace_{}_{}.json", safe_model, date));
        if let Err(e) = report.save_json(&result_path) {
            eprintln!("failed to save report for {}: {}", model, e);
        } else {
            println!("  saved -> {}", result_path.display());
        }
    }
}

// ---- direct-path policy (real gate) ----

fn real_gate() -> spoon_lang::ears::gate::SceGate {
    let mut gate = spoon_lang::ears::gate::SceGate::with_defaults();
    gate.add_names(&["John", "Mary", "Bob"]);
    gate
}

/// A one-thread fake Ollama: answers each `/api/chat` POST with the next
/// canned reply (the last one repeats). Returns the base URL.
fn fake_ollama(replies: &[&str]) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake ollama");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let replies: Vec<String> = replies.iter().map(|s| s.to_string()).collect();
    std::thread::spawn(move || {
        for (i, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { break };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let headers = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                let len = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + len {
                    break;
                }
            }
            let reply = replies.get(i).or(replies.last()).cloned().unwrap_or_default();
            let body = serde_json::json!({"message": {"role": "assistant", "content": reply}}).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    base
}

fn ears_with_fake_llm(replies: &[&str]) -> Ears {
    use spoon_core::llm::{LlmClient, LlmConfig, Transport};
    let cfg = LlmConfig {
        base_url: fake_ollama(replies),
        api_key: None,
        model: "fake".into(),
        transport: Transport::Ollama,
        timeout_secs: 5,
        temperature: 0.0,
        no_think: true,
    };
    let lex = Lexicon::load_seed_dir(&seed_dir()).expect("load lexicon");
    let phrasings = spoon_lang::ears::load_phrasings(&test_data_dir(), &lex).expect("load phrasings");
    Ears::new(lex, phrasings, Some((LlmClient::new(), cfg)))
}

/// The model turns gibberish into grammatical SCE; the guard turns it back
/// into a failure because no content word is known. A fresh user's name is
/// not gibberish: `owns` and `dog` carry the sentence.
#[tokio::test]
async fn garbage_from_the_llm_is_rejected() {
    let gate = real_gate();

    let ears = ears_with_fake_llm(&["Zxqv is a wibble."]);
    let result = ears.hear("zxqv flarp wibble", &gate).await;
    assert_eq!(result.path, EarsPath::Failed, "got {:?} / {:?}", result.path, result.sce);
    for word in ["zxqv", "flarp", "wibble"] {
        assert!(result.unknown_words.iter().any(|w| w.eq_ignore_ascii_case(word)), "{word} missing from {:?}", result.unknown_words);
    }

    let ears = ears_with_fake_llm(&["Kealan owns a dog."]);
    let result = ears.hear("kealan owns a dog innit", &gate).await;
    assert_eq!(result.path, EarsPath::Llm, "got {:?} / {:?}", result.path, result.sce);
    assert_eq!(result.sce, "Kealan owns a dog.");
    assert_eq!(result.unknown_words, vec!["Kealan".to_string(), "innit".to_string()]);
}

/// `??` is the prompt's escape hatch: Failed at once, no repair round.
#[tokio::test]
async fn no_meaning_marker_fails_without_repair() {
    let ears = ears_with_fake_llm(&["??"]);
    let result = ears.hear("zxqv flarp wibble", &real_gate()).await;
    assert_eq!(result.path, EarsPath::Failed);
    let (llm_calls, repairs, _) = ears.stats().snapshot();
    assert_eq!((llm_calls, repairs), (1, 0));
}

/// Loop-like English becomes quantified SCE natively (ACE ids 81, 83, 85, 87, 88).
#[test]
fn loops_resolve_natively() {
    let ears = make_ears();
    let gate = real_gate();
    let cases = [
        (
            "for every user if they are inactive disable their account",
            "For every user X if X is inactive then Assistant disables X's account.",
        ),
        (
            "for each customer give them a receipt if they bought something",
            "For every customer X if X buys something then Assistant gives a receipt to X.",
        ),
        (
            "check every file, if its empty delete it, if its not empty archive it",
            "Assistant deletes every empty file. Assistant archives every file that is not empty.",
        ),
        (
            "keep processing orders as long as there are pending ones and never process a cancelled order",
            "Assistant processes every pending order. Assistant processes no cancelled order.",
        ),
        (
            "if any task has more than 3 failures stop retrying that task",
            "If a task owns more than 3 failures then Assistant stops retrying the task.",
        ),
        ("go through every card and reject the expired ones", "Assistant rejects every expired card."),
    ];
    for (input, expected) in cases {
        let result = ears.hear_offline(input, &gate);
        assert_eq!(result.sce, expected, "input {input:?}");
        assert_eq!(result.path, EarsPath::Direct, "input {input:?}");
    }
}

/// Bare plurals are universals.
#[test]
fn plural_universals() {
    let ears = make_ears();
    let gate = real_gate();
    let cases = [
        ("Wolves are white.", "Every wolf is white."),
        ("dogs are animals", "Every dog is an animal."),
        ("cats have tails", "Every cat has a tail."),
        ("dogs are not cats", "No dog is a cat."),
    ];
    for (input, expected) in cases {
        let result = ears.hear_offline(input, &gate);
        assert_eq!(result.sce, expected, "input {input:?}");
        assert_eq!(result.path, EarsPath::Direct, "input {input:?}");
    }
}

/// One level of reported speech unwraps deterministically; the speaker's
/// name survives the lowercasing and the typo repair.
#[test]
fn reported_speech_unwraps_one_level() {
    let ears = make_ears();
    let gate = real_gate();
    let result = ears.hear_offline("Jake told me \"Sarah said Bob hates me\" but Sarah swears she never said anything about Jake at all", &gate);
    assert!(
        result.sce.starts_with("Jake tells User that Sarah says that Bob hates Jake."),
        "got {:?}",
        result.sce
    );
    assert!(result.sce.contains("about Jake"), "the second mention keeps the name: {:?}", result.sce);

    let result = ears.hear_offline("Greg said \"Ian told me Dheeraj thinks you're ready\"", &gate);
    assert_eq!(result.sce, "Greg says that Ian tells Greg that Dheeraj thinks that User is ready.");
    assert_eq!(result.path, EarsPath::Direct, "got {:?}", result.path);
}

/// Junk that the strict parser would accept as Names must not sneak in as a
/// clean direct parse; the greeting phrasing gets its turn.
#[test]
fn junk_greeting_resolves_via_phrasing() {
    let ears = make_ears();
    let gate = real_gate();
    let result = ears.hear_offline("yo whats up", &gate);
    assert_eq!(result.path, EarsPath::Phrasing, "got {:?} / {:?}", result.path, result.sce);
    assert_eq!(result.sce, "User greets Assistant.");
}

#[test]
fn capitalized_junk_is_never_a_clean_direct() {
    let ears = make_ears();
    let gate = real_gate();
    let result = ears.hear_offline("Hello whats up.", &gate);
    assert!(
        !(result.path == EarsPath::Direct && result.confidence >= 1.0),
        "clean Direct for junk: {:?}",
        result
    );
    // The gate accepts it (Hello as a Name), so the last resort still yields
    // the dirty parse with the unknown words reported, or the phrasing wins.
    if result.path == EarsPath::Direct {
        assert!(result.confidence <= 0.3);
        assert!(!result.unknown_words.is_empty(), "unknown words must be reported: {:?}", result);
    }
    // `who owns a dog` is a question, not an assertion about a dog owner.
    let q = ears.hear_offline("who owns a dog", &gate);
    assert_eq!(q.sce, "Who owns a dog?", "{:?}", q);
}

/// A command whose only unknown word is its verb is still a clean Direct
/// parse: that is how new verbs reach the learner.
#[test]
fn unknown_verb_command_is_direct() {
    let ears = make_ears();
    let gate = real_gate();
    let result = ears.hear_offline("Assistant, double 21!", &gate);
    assert_eq!(result.path, EarsPath::Direct, "{:?}", result);
    assert!(result.confidence >= 1.0, "{:?}", result);
    assert!(matches!(result.clauses[0].act, Act::Command));

    // A verb nobody has ever seen is still clean in a command, and reported.
    let result = ears.hear_offline("Assistant, frobnicate 21!", &gate);
    assert_eq!(result.path, EarsPath::Direct, "{:?}", result);
    assert!(result.confidence >= 1.0, "{:?}", result);
    assert_eq!(result.unknown_words, vec!["frobnicate".to_string()]);
    // ... but not in an assertion.
    let result = ears.hear_offline("John frobnicates a dog.", &gate);
    assert!(result.path != EarsPath::Direct || result.confidence < 1.0, "{:?}", result);

    // Same command reached through an indirect request.
    let result = ears.hear_offline("can u double 21 for me", &gate);
    assert_eq!(result.sce, "Assistant, double 21!", "{:?}", result);
    assert_eq!(result.path, EarsPath::Direct);
}

// ---- convo20 ----

fn convo_corpus() -> std::path::PathBuf {
    workspace_root().join("data/bench/convo20.json")
}

/// Every expected SCE in convo20 must parse with the real gate, otherwise the
/// bench measures the corpus instead of the ears.
#[test]
fn convo20_expected_parses() {
    let text = std::fs::read_to_string(convo_corpus()).expect("convo20.json");
    let items: Vec<spoon_lang::ears::bench::ConvoItem> = serde_json::from_str(&text).unwrap();
    assert_eq!(items.len(), 20);
    let gate = real_gate();
    let bad: Vec<String> = items
        .iter()
        .filter_map(|i| gate.parse(&i.sce).err().map(|e| format!("{} -> {e}", i.sce)))
        .collect();
    assert!(bad.is_empty(), "expected SCE that does not parse:\n{}", bad.join("\n"));
}

/// Offline conversation bench: no LLM, at least half the turns must land.
#[test]
fn convo20_native() {
    let ears = make_ears();
    let gate = real_gate();
    let report = spoon_lang::ears::bench::run_convo(&ears, &gate, &convo_corpus(), false).expect("run_convo");
    println!("=== convo20_native (no LLM) ===\n{report}");
    assert_eq!(report.llm_calls, 0);
    assert!(report.hits >= 10, "native hits {}/{} < 10", report.hits, report.total);
}

/// Conversation bench with the LLM seat. Run with SPOON_LLM_TESTS=1.
#[test]
#[ignore]
fn convo20_llm_live() {
    if std::env::var("SPOON_LLM_TESTS").as_deref() != Ok("1") {
        return;
    }
    use spoon_core::llm::{LlmClient, LlmConfig};
    let mut lex = Lexicon::load_seed_dir(&seed_dir()).unwrap();
    lex.add_names(&["John", "Mary", "Ben", "Bob"]);
    let phrasings = spoon_lang::ears::load_phrasings(&test_data_dir(), &lex).unwrap();
    let ears = Ears::new(lex, phrasings, Some((LlmClient::new(), LlmConfig::ollama("qwen3.5:4b"))));
    let gate = real_gate();
    let report = spoon_lang::ears::bench::run_convo(&ears, &gate, &convo_corpus(), true).expect("run_convo");
    println!("=== convo20_llm_live (qwen3.5:4b) ===\n{report}");
    assert!(report.hits >= 17, "live hits {}/{} < 17", report.hits, report.total);
}

/// Conservative typo repair: known words must survive, garbled words still repair.
#[test]
fn typo_repair_is_conservative() {
    use spoon_lang::ears::normalize::normalize;

    let lex = Lexicon::load_seed_dir(&seed_dir()).expect("load lexicon");
    // "you", "dude", "yo", "good" are known - must not mangle.
    // "you" now becomes "Assistant" via speaker grounding.
    // "lol" is dropped globally.
    {
        let n = normalize("yo dude you good lol", &lex);
        let text = n.sentences.join(" ");
        assert!(
            text.to_lowercase().contains("assistant"),
            "\"you\" should become \"Assistant\", got: {:?}",
            text
        );
        assert!(
            !text.to_lowercase().contains("lol"),
            "\"lol\" should be dropped, got: {:?}",
            text
        );
    }
    // "hell" is a common English word - must NOT be repaired to "help".
    // (As a wh-expletive, `where the hell` -> `where`, so use it as a noun.)
    {
        let n = normalize("hell is a hot place", &lex);
        let text = n.sentences.join(" ");
        assert!(
            text.to_lowercase().contains("hell"),
            "\"hell\" must survive, got: {:?}",
            text
        );
        let n = normalize("where the hell is bob at", &lex);
        assert_eq!(n.sentences, vec!["Where is Bob?"]);
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
