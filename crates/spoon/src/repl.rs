//! Talking to it: interactive, JSON lines, and the bench.

use std::io::{BufRead, Write};

use anyhow::Result;
use spoon_seat::Seat;

use crate::Cli;
use crate::build::assemble;

pub async fn run(cli: &Cli) -> Result<()> {
    let mut brain = assemble(cli).await?;
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
    let mut brain = assemble(cli).await?;
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

    let mut brain = assemble(cli).await?;
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
            println!("{:<4} {:<52} {:<22} (setup)", i + 1, short(&case.say, 50), short(&got, 20));
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
