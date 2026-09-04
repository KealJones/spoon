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
pub async fn bench(cli: &Cli, suite: &str) -> Result<()> {
    let path = format!("data/bench/{suite}.json");
    let raw =
        std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let corpus: serde_json::Value = serde_json::from_str(&raw)?;
    let lines: Vec<&str> = corpus["lines"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut brain = assemble(cli).await?;
    let mut native = 0usize;
    let mut model = 0usize;
    let mut failed = 0usize;
    let mut gaps = 0usize;
    let mut interior_calls = 0u64;
    let started = std::time::Instant::now();

    println!("{:<3} {:<58} reply", "#", "utterance");
    println!("{}", "-".repeat(110));
    for (i, line) in lines.iter().enumerate() {
        let result = brain.turn("bench", line).await?;
        let e = &result.episode;
        match e.ears_path {
            spoon_brain::EarsPath::Native => native += 1,
            spoon_brain::EarsPath::Model => model += 1,
            spoon_brain::EarsPath::Failed => failed += 1,
        }
        gaps += e.gaps.len();
        interior_calls += e.metrics.interior_model_calls;
        let short = |s: &str, n: usize| {
            let t: String = s.chars().take(n).collect();
            if s.chars().count() > n {
                format!("{t}...")
            } else {
                t
            }
        };
        println!(
            "{:<3} {:<58} {}",
            i + 1,
            short(line, 55),
            short(&result.reply, 45)
        );
    }

    let total = lines.len().max(1);
    println!("\n{} utterances in {:?}", total, started.elapsed());
    println!(
        "ears: native {native} ({:.0}%)  model {model}  failed {failed}",
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
