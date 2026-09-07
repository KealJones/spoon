//! Talking to it: interactive, JSON lines, and the bench.

use std::io::{BufRead, Write};

use anyhow::Result;
use spoon_seat::Seat;

use crate::Cli;
use crate::build::{assemble, assemble_with_ears};

pub async fn run(cli: &Cli) -> Result<()> {
    let (mut brain, _ears_flag) = assemble(cli).await?;
    println!("spoon. ctrl-d to leave. :help for commands.");
    let stdin = std::io::stdin();
    loop {
        print!("> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            println!();
            return Ok(());
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match line {
            ":q" | ":quit" => return Ok(()),
            ":help" => {
                println!(":metrics  counters for the last turn");
                println!(":status   what this brain holds");
                println!(":q        leave");
                continue;
            }
            ":status" => {
                crate::build::status(cli)?;
                continue;
            }
            _ => {}
        }
        let show_metrics = line == ":metrics";
        let text = if show_metrics { "" } else { line };
        if show_metrics {
            println!(
                "ears {}  mouth {}  teacher {}",
                brain.seat_calls(Seat::Ears),
                brain.seat_calls(Seat::Mouth),
                brain.seat_calls(Seat::Teacher)
            );
            continue;
        }

        let result = brain.turn("repl", text).await?;
        println!("{}", result.reply);
        // Shown unprompted, because learning something is worth knowing about
        // even when it did not change this answer.
        for note in &result.episode.learning {
            println!("  + {note}");
        }
        if std::env::var("SPOON_DEBUG").is_ok() {
            let e = &result.episode;
            println!(
                "  [ears {:?} | steps {} | gaps {} | {}ms]",
                e.ears_path,
                e.steps.len(),
                e.gaps.len(),
                e.metrics.millis_total
            );
            for step in &e.steps {
                println!("  step: {}", brain.render(step));
            }
        }
    }
}

pub async fn stdio(cli: &Cli) -> Result<()> {
    let (mut brain, _ears_flag) = assemble(cli).await?;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: serde_json::Value = serde_json::from_str(&line)?;
        let session = request["session"].as_str().unwrap_or("stdio");
        let text = request["text"].as_str().unwrap_or_default();
        let result = brain.turn(session, text).await?;
        let out = serde_json::json!({
            "text": result.reply,
            "episode_id": result.episode.id,
            "ears_path": format!("{:?}", result.episode.ears_path),
            "gaps": result.episode.gaps.len(),
            "learning": result.episode.learning,
            "metrics": result.episode.metrics,
        });
        println!("{out}");
        std::io::stdout().flush()?;
    }
    Ok(())
}

/// Run a corpus and report how the ears did.
///
/// The headline number is how often the model was needed. It should fall as
/// Spoon learns; if it does not, the native path is not learning anything.
/// One case in a graded suite.
///
/// `expect` is the rendered result concept, not the reply. The mouth is a
/// language model, so grading its prose measures the mouth's mood; the result
/// concept is what the interior actually worked out, and it is the thing that
/// is either right or wrong.
#[derive(serde::Deserialize)]
struct Case {
    say: String,
    #[serde(default)]
    expect: Option<String>,
    /// Any one of these counts. For questions with more than one true answer.
    #[serde(default)]
    expect_any: Vec<String>,
    #[serde(default)]
    category: String,
    /// Run it, do not grade it.
    ///
    /// Some questions only mean something after something else was said.
    /// "who has a dog" needs "john has a dog" to have happened, and the
    /// setup turn is not itself the thing under test.
    #[serde(default)]
    setup: bool,
}

impl Case {
    fn accepts(&self, got: &str) -> bool {
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let got = norm(got);
        if let Some(want) = &self.expect
            && norm(want) == got
        {
            return true;
        }
        self.expect_any.iter().any(|w| norm(w) == got)
    }

    fn wanted(&self) -> String {
        match &self.expect {
            Some(e) => e.clone(),
            None => self.expect_any.join(" | "),
        }
    }
}

pub async fn bench(cli: &Cli, suite: &str) -> Result<()> {
    let path = format!("data/bench/{suite}.json");
    let raw =
        std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let corpus: serde_json::Value = serde_json::from_str(&raw)?;

    // Two shapes. `lines` is ungraded and only reports how a turn went, which
    // is all the older corpora can support. `cases` carries the answer, so the
    // suite can say whether Spoon was right. Ungraded suites were the reason
    // "it cannot answer anything" stayed invisible for so long: every number
    // the bench printed was about paths taken, not answers given.
    let lines: Vec<&str> = corpus["text-lines"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let cases: Vec<Case> = corpus["cases"]
        .as_array()
        .map(|a| serde_json::from_value(serde_json::Value::Array(a.clone())))
        .transpose()?
        .unwrap_or_default();
    let graded = !cases.is_empty();
    let cases: Vec<Case> = if graded {
        cases
    } else {
        lines
            .iter()
            .map(|l| Case {
                say: (*l).to_string(),
                expect: None,
                expect_any: Vec::new(),
                category: String::new(),
                setup: false,
            })
            .collect()
    };

    let (mut brain, _ears_flag) = assemble(cli).await?;
    let mut native = 0usize;
    let mut model = 0usize;
    let mut failed = 0usize;
    let mut gaps = 0usize;
    let mut interior_calls = 0u64;
    let mut right = 0usize;
    let mut by_category: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut wrong: Vec<(String, String, String)> = Vec::new();
    let started = std::time::Instant::now();

    println!("{:<4} {:<52} {:<22} want", "#", "utterance", "got");
    println!("{}", "-".repeat(110));
    for (i, case) in cases.iter().enumerate() {
        let result = brain.turn("bench", &case.say).await?;
        let e = &result.episode;
        match e.ears_path {
            spoon_brain::EarsPath::Native => native += 1,
            spoon_brain::EarsPath::Model => model += 1,
            spoon_brain::EarsPath::Failed => failed += 1,
        }
        gaps += e.gaps.len();
        interior_calls += e.metrics.interior_model_calls;

        let got = e
            .result
            .as_ref()
            .map(|c| brain.render(c))
            .unwrap_or_else(|| "-".to_string());
        let short = |s: &str, n: usize| {
            let t: String = s.chars().take(n).collect();
            if s.chars().count() > n {
                format!("{t}...")
            } else {
                t
            }
        };

        if graded && case.setup {
            println!(
                "{:<4} {:<52} {:<22} (setup)",
                i + 1,
                short(&case.say, 50),
                short(&got, 20)
            );
        } else if graded {
            let ok = case.accepts(&got);
            if ok {
                right += 1;
            } else {
                wrong.push((case.say.clone(), got.clone(), case.wanted()));
            }
            let slot = by_category.entry(case.category.clone()).or_default();
            slot.1 += 1;
            if ok {
                slot.0 += 1;
            }
            println!(
                "{:<4} {:<52} {:<22} {}",
                if ok {
                    (i + 1).to_string()
                } else {
                    format!("{}x", i + 1)
                },
                short(&case.say, 50),
                short(&got, 20),
                short(&case.wanted(), 24)
            );
        } else {
            println!(
                "{:<4} {:<52} {}",
                i + 1,
                short(&case.say, 50),
                short(&result.reply, 45)
            );
        }
    }

    let total = cases.iter().filter(|c| !c.setup).count().max(1);
    println!("\n{} utterances in {:?}", total, started.elapsed());
    if graded {
        println!(
            "correct: {right}/{total} ({:.0}%)",
            100.0 * right as f64 / total as f64
        );
        println!("\nby category:");
        for (name, (ok, n)) in &by_category {
            println!(
                "  {:<22} {:>3}/{:<3} {:>3.0}%",
                if name.is_empty() { "-" } else { name },
                ok,
                n,
                100.0 * *ok as f64 / *n.max(&1) as f64
            );
        }
        if !wrong.is_empty() {
            println!("\nwrong ({}):", wrong.len());
            for (say, got, want) in wrong.iter().take(40) {
                println!("  {say}\n      got  {got}\n      want {want}");
            }
        }
    }
    println!(
        "\nears: native {native} ({:.0}%)  model {model}  failed {failed}",
        100.0 * native as f64 / total as f64
    );
    println!("capability gaps encountered: {gaps}");
    println!("interior model calls: {interior_calls}");
    // The one number that is not a matter of degree.
    anyhow::ensure!(
        interior_calls == 0,
        "a model was consulted inside the interior, which the design forbids"
    );
    Ok(())
}

pub async fn bench_compare_ears(cli: &Cli, suite: &str) -> Result<()> {
    let path = format!("data/bench/{suite}.json");
    let raw =
        std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let corpus: serde_json::Value = serde_json::from_str(&raw)?;
    let cases: Vec<Case> = corpus["cases"]
        .as_array()
        .map(|a| serde_json::from_value(serde_json::Value::Array(a.clone())))
        .transpose()?
        .unwrap_or_default();

    if cases.is_empty() {
        anyhow::bail!("no graded cases in {path}");
    }

    let (mut brain_ab, ears_flag_ab) = assemble(cli).await?;
    ears_flag_ab.set(spoon_ears::EarsFormat::AngleBracket);

    let (mut brain_py, ears_flag_py) = assemble(cli).await?;
    ears_flag_py.set(spoon_ears::EarsFormat::PythonCall);

    let mut ab_right = 0usize;
    let mut py_right = 0usize;
    let mut both_right = 0usize;
    let mut both_wrong = 0usize;
    let mut ab_only = 0usize;
    let mut py_only = 0usize;
    let total = cases.iter().filter(|c| !c.setup).count();
    let started = std::time::Instant::now();

    println!(
        "{:<4} {:<40} {:<18} {:<18} {}",
        "#", "utterance", "angle-bracket", "python", "want"
    );
    println!("{}", "-".repeat(110));

    for (i, case) in cases.iter().enumerate() {
        let ab_result = brain_ab.turn("bench-ab", &case.say).await?;
        let py_result = brain_py.turn("bench-py", &case.say).await?;

        let ab_got = ab_result
            .episode
            .result
            .as_ref()
            .map(|c| brain_ab.render(c))
            .unwrap_or_else(|| "-".to_string());
        let py_got = py_result
            .episode
            .result
            .as_ref()
            .map(|c| brain_py.render(c))
            .unwrap_or_else(|| "-".to_string());

        if case.setup {
            println!("{:<4} {:<40} (setup)", i + 1, truncate(&case.say, 38));
            continue;
        }

        let ab_ok = case.accepts(&ab_got);
        let py_ok = case.accepts(&py_got);

        if ab_ok {
            ab_right += 1;
        }
        if py_ok {
            py_right += 1;
        }
        match (ab_ok, py_ok) {
            (true, true) => both_right += 1,
            (false, false) => both_wrong += 1,
            (true, false) => ab_only += 1,
            (false, true) => py_only += 1,
        }

        let marker = match (ab_ok, py_ok) {
            (true, true) => format!("{}", i + 1),
            (false, false) => format!("{}x", i + 1),
            (true, false) => format!("{}~", i + 1), // AB won
            (false, true) => format!("{}+", i + 1), // PY won
        };
        println!(
            "{:<4} {:<40} {:<18} {:<18} {}",
            marker,
            truncate(&case.say, 38),
            truncate(&ab_got, 16),
            truncate(&py_got, 16),
            truncate(&case.wanted(), 20),
        );
    }

    let elapsed = started.elapsed();
    println!("\n{total} cases in {elapsed:?}");
    println!(
        "angle-bracket: {ab_right}/{total} ({:.0}%)",
        100.0 * ab_right as f64 / total as f64
    );
    println!(
        "python:        {py_right}/{total} ({:.0}%)",
        100.0 * py_right as f64 / total as f64
    );
    println!("\nboth right: {both_right}  both wrong: {both_wrong}");
    println!("angle-bracket only: {ab_only}  python only: {py_only}");

    if py_only > 0 {
        println!("\n--- python wins ({py_only}) ---");
        // Re-run to show details would be too slow, the numbers above tell the story
    }
    if ab_only > 0 {
        println!("\n--- angle-bracket wins ({ab_only}) ---");
    }

    Ok(())
}

/// Run one suite through two ears models and compare.
///
/// Accuracy alone does not settle the choice, because the ears run on every
/// utterance and a model that is three points better and twenty times slower
/// is worse in a conversation. Latency is reported per case for that reason.
///
/// The two brains differ only in the ears model. Mouth and Teacher come from
/// the ordinary configuration, so a difference here is a difference in reading.
pub async fn bench_compare_models(
    cli: &Cli,
    suite: &str,
    spec: &str,
    limit: Option<usize>,
) -> Result<()> {
    let (left_model, right_model) = spec
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("--compare-models wants two models, as A,B"))?;
    let left_model = left_model.trim();
    let right_model = right_model.trim();
    if left_model.is_empty() || right_model.is_empty() {
        anyhow::bail!("--compare-models wants two models, as A,B");
    }

    let path = format!("data/bench/{suite}.json");
    let raw =
        std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let corpus: serde_json::Value = serde_json::from_str(&raw)?;
    let mut cases: Vec<Case> = corpus["cases"]
        .as_array()
        .map(|a| serde_json::from_value(serde_json::Value::Array(a.clone())))
        .transpose()?
        .unwrap_or_default();

    if cases.is_empty() {
        anyhow::bail!("no graded cases in {path}");
    }
    if let Some(n) = limit {
        cases.truncate(n);
    }

    if !cli.ephemeral {
        // Both brains open the same file, so a phrasing one of them learns is
        // on the native path for the other by the next case, and the numbers
        // stop being about the models.
        println!("note: both brains share the persistent brain, so what one");
        println!("      learns the other gets for free. Rerun with --ephemeral");
        println!("      to measure the models rather than the leakage.\n");
    }

    let (mut left, _) = assemble_with_ears(cli, Some(left_model)).await?;
    let (mut right, _) = assemble_with_ears(cli, Some(right_model)).await?;

    let mut left_right_count = 0usize;
    let mut right_right_count = 0usize;
    let mut both_right = 0usize;
    let mut both_wrong = 0usize;
    let mut left_only = 0usize;
    let mut right_only = 0usize;
    let mut left_millis = 0u128;
    let mut right_millis = 0u128;
    let mut left_model_calls = 0u64;
    let mut right_model_calls = 0u64;
    // Separated from a wrong answer on purpose. A seat that timed out or
    // returned unreadable output is a configuration problem, and counting it
    // as a misreading blames the model for something it may never have said.
    let mut left_failed = 0u64;
    let mut right_failed = 0u64;
    let total = cases.iter().filter(|c| !c.setup).count();
    let started = std::time::Instant::now();

    println!(
        "{:<5} {:<38} {:<16} {:<16} {}",
        "#",
        "utterance",
        truncate(left_model, 14),
        truncate(right_model, 14),
        "want"
    );
    println!("{}", "-".repeat(110));

    for (i, case) in cases.iter().enumerate() {
        let left_started = std::time::Instant::now();
        let left_result = left.turn("bench-left", &case.say).await?;
        let left_elapsed = left_started.elapsed().as_millis();

        let right_started = std::time::Instant::now();
        let right_result = right.turn("bench-right", &case.say).await?;
        let right_elapsed = right_started.elapsed().as_millis();

        match left_result.episode.ears_path {
            spoon_brain::EarsPath::Model => left_model_calls += 1,
            spoon_brain::EarsPath::Failed => left_failed += 1,
            spoon_brain::EarsPath::Native => {}
        }
        match right_result.episode.ears_path {
            spoon_brain::EarsPath::Model => right_model_calls += 1,
            spoon_brain::EarsPath::Failed => right_failed += 1,
            spoon_brain::EarsPath::Native => {}
        }

        let left_got = left_result
            .episode
            .result
            .as_ref()
            .map(|c| left.render(c))
            .unwrap_or_else(|| "-".to_string());
        let right_got = right_result
            .episode
            .result
            .as_ref()
            .map(|c| right.render(c))
            .unwrap_or_else(|| "-".to_string());

        if case.setup {
            println!("{:<5} {:<38} (setup)", i + 1, truncate(&case.say, 36));
            continue;
        }

        left_millis += left_elapsed;
        right_millis += right_elapsed;

        let left_ok = case.accepts(&left_got);
        let right_ok = case.accepts(&right_got);

        if left_ok {
            left_right_count += 1;
        }
        if right_ok {
            right_right_count += 1;
        }
        match (left_ok, right_ok) {
            (true, true) => both_right += 1,
            (false, false) => both_wrong += 1,
            (true, false) => left_only += 1,
            (false, true) => right_only += 1,
        }

        let marker = match (left_ok, right_ok) {
            (true, true) => format!("{}", i + 1),
            (false, false) => format!("{}x", i + 1),
            (true, false) => format!("{}~", i + 1),
            (false, true) => format!("{}+", i + 1),
        };
        println!(
            "{:<5} {:<38} {:<16} {:<16} {}",
            marker,
            truncate(&case.say, 36),
            truncate(&left_got, 14),
            truncate(&right_got, 14),
            truncate(&case.wanted(), 18),
        );
    }

    let elapsed = started.elapsed();
    let mean = |millis: u128| millis as f64 / total.max(1) as f64 / 1000.0;
    println!("\n{total} graded cases in {elapsed:?}");
    println!(
        "{:<16} {:>4}/{total} ({:>3.0}%)  {:>6.2}s per case  {:>4} ears calls  {:>3} ears failures",
        left_model,
        left_right_count,
        100.0 * left_right_count as f64 / total as f64,
        mean(left_millis),
        left_model_calls,
        left_failed,
    );
    println!(
        "{:<16} {:>4}/{total} ({:>3.0}%)  {:>6.2}s per case  {:>4} ears calls  {:>3} ears failures",
        right_model,
        right_right_count,
        100.0 * right_right_count as f64 / total as f64,
        mean(right_millis),
        right_model_calls,
        right_failed,
    );
    println!("\nboth right: {both_right}  both wrong: {both_wrong}");
    println!("{left_model} only: {left_only}  {right_model} only: {right_only}");

    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}...")
    } else {
        t
    }
}
