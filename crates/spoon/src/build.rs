//! Assembling a brain, and the commands that do not need a conversation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use spoon_brain::{Brain, BrainConfig, Seats};
use spoon_ears::{ModelEars, NativeEars};
use spoon_eval::PermissionMode;
use spoon_mouth::{ModelMouth, TemplateMouth};
use spoon_seat::{Ears, LlmClient, LlmConfig, Mouth, SeatCounters, Teacher};
use spoon_store::Store;
use spoon_teach::ModelTeacher;

use crate::Cli;
use crate::config::Config;

const DEFAULT_MODEL: &str = "qwen3.5:4b";
const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1";

/// How to reach one seat: flag, then config file, then built-in default.
///
/// Resolved per seat rather than once for the process, because the seats want
/// different things. The ears run on every utterance and a slow one is felt
/// immediately, while the Teacher runs on a miss and is the only seat where a
/// large or remote model earns its latency.
fn seat_config(
    flag: &Option<String>,
    configured: Option<&crate::config::Seat>,
    default_model: &str,
) -> Result<LlmConfig> {
    let model = flag
        .as_deref()
        .or_else(|| configured.and_then(|s| s.model.as_deref()))
        .unwrap_or(default_model);

    let provider = configured
        .and_then(|s| s.provider.as_deref())
        .unwrap_or("ollama");

    let mut config = match provider {
        "ollama" => LlmConfig::ollama(model),
        "openai" => {
            // The variable is named in the config; the key itself never is.
            // An absent key is not an error: a local llama.cpp or vLLM server
            // speaks this protocol and wants no auth at all.
            let key = configured
                .and_then(|s| s.api_key_env.as_deref())
                .or(Some("OPENAI_API_KEY"))
                .and_then(|name| std::env::var(name).ok())
                .filter(|k| !k.is_empty());
            let url = configured
                .and_then(|s| s.base_url.as_deref())
                .unwrap_or(DEFAULT_OPENAI_URL);
            LlmConfig::openai(url, model, key)
        }
        other => anyhow::bail!(
            "unknown provider {other:?} in ~/.spoon/config.json; expected \"ollama\" or \"openai\""
        ),
    };

    if let Some(url) = configured.and_then(|s| s.base_url.as_deref()) {
        config.base_url = url.to_string();
    }
    if let Some(secs) = configured.and_then(|s| s.timeout_secs) {
        config = config.with_timeout(std::time::Duration::from_secs(secs));
    }
    Ok(config)
}

/// Whichever implementations the configuration ended up with.
type SeatTrio = (Box<dyn Ears>, Box<dyn Mouth>, Option<Box<dyn Teacher>>);

pub fn brain_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.db {
        return Ok(path.clone());
    }
    // Config file path, if set.
    if let Ok(settings) = Config::load() {
        if let Some(db) = settings.database.as_ref().and_then(|d| d.path.as_ref()) {
            return Ok(db.clone());
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    let dir = PathBuf::from(home).join(".spoon");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("spoon-v2.db"))
}

pub fn open_store(cli: &Cli) -> Result<Store> {
    if cli.ephemeral {
        return Ok(Store::open_in_memory()?);
    }
    // A throwaway brain for anyone testing, so nobody reaches for the real one
    // and deletes it. SPOON_SCRATCH is set by the test scripts; it is not a
    // thing a user ever needs to know about.
    if let Ok(path) = std::env::var("SPOON_SCRATCH") {
        return Ok(Store::open(std::path::Path::new(&path))?);
    }
    let path = brain_path(cli)?;
    Ok(Store::open(&path)?)
}

fn permission(mode: &str) -> PermissionMode {
    match mode {
        "always-ask" => PermissionMode::AlwaysAsk,
        "bypass" => PermissionMode::Bypass,
        _ => PermissionMode::AskWrites,
    }
}

/// Build a brain, seeding a fresh one so it can actually do something.
pub async fn assemble(cli: &Cli) -> Result<(Brain, spoon_ears::EarsFormatFlag)> {
    assemble_with_ears(cli, None).await
}

/// Build a brain with the ears pointed at a specific model.
///
/// The override exists for the comparison bench, which needs two brains that
/// differ in exactly one thing. It takes precedence over both the flag and the
/// config file, since it is the more specific request.
pub async fn assemble_with_ears(
    cli: &Cli,
    ears_override: Option<&str>,
) -> Result<(Brain, spoon_ears::EarsFormatFlag)> {
    let settings = Config::load()?;
    let ears = seat_config(
        &ears_override
            .map(str::to_string)
            .or_else(|| cli.ears_model.clone()),
        settings.ears.as_ref(),
        DEFAULT_MODEL,
    )?;
    let mouth = seat_config(&cli.mouth_model, settings.mouth.as_ref(), DEFAULT_MODEL)?;
    let mut teacher = seat_config(&cli.teacher_model, settings.teacher.as_ref(), DEFAULT_MODEL)?;
    if settings
        .teacher
        .as_ref()
        .and_then(|s| s.timeout_secs)
        .is_none()
    {
        // Ears and mouth stay snappy. The Teacher is the large model, and a
        // lesson is several lines, so twenty seconds is a truncated thought.
        teacher = teacher.with_timeout(std::time::Duration::from_secs(180));
    }
    let permissions = cli
        .permissions
        .as_deref()
        .or_else(|| settings.permission_mode())
        .unwrap_or("ask-writes")
        .to_string();

    let store = open_store(cli)?;
    let registry = spoon_natives::bootstrap();

    // A brand new brain has no realizations, so every call would look like
    // data. Seeding is idempotent, so an existing brain just gets refreshed.
    spoon_natives::seed_bootstrap(&store, &registry)?;
    spoon_infer::seed_meta_rules(&store)?;

    let counters = Arc::new(SeatCounters::default());
    let table = Arc::new(store.load_symbol_table()?);

    let online = if cli.offline {
        false
    } else {
        LlmClient::new(ears.clone(), counters.clone())
            .reachable()
            .await
    };

    let teaching = online && !cli.no_teaching;
    let ears_format_flag =
        spoon_ears::EarsFormatFlag::new(!std::env::var("SPOON_EARS_ANGLES").is_ok());
    let (ears, mouth, teacher): SeatTrio = if online {
        (
            Box::new(
                ModelEars::new(LlmClient::new(ears.clone(), counters.clone()))
                    .with_format_flag(ears_format_flag.clone()),
            ),
            Box::new(ModelMouth::new(
                LlmClient::new(mouth, counters.clone()),
                table.clone(),
            )),
            teaching.then(|| {
                Box::new(ModelTeacher::new(
                    LlmClient::new(teacher, counters.clone()),
                    table.clone(),
                )) as Box<dyn Teacher>
            }),
        )
    } else {
        (
            Box::new(NativeEars::new()),
            Box::new(TemplateMouth::new(table.clone())),
            None,
        )
    };

    let config = BrainConfig {
        permission: permission(&permissions),
        teaching,
        ..BrainConfig::default()
    };
    let seats = Seats {
        ears,
        mouth,
        teacher,
        counters,
    };
    Ok((
        Brain::new(store, registry, table, seats, config)?,
        ears_format_flag,
    ))
}

pub fn status(cli: &Cli) -> Result<()> {
    let store = open_store(cli)?;
    println!(
        "brain:        {}",
        if cli.ephemeral {
            "(ephemeral)".into()
        } else {
            brain_path(cli)?.display().to_string()
        }
    );
    println!("schema:       v{}", store.schema_version()?);
    println!("concepts:     {}", store.count_concepts()?);
    println!("realizations: {}", store.all_realizations()?.len());
    println!("episodes:     {}", store.count_episodes()?);
    println!("symbols:      {}", store.all_symbols()?.len());

    // What the ears have learned, and whether it is holding up. A count alone
    // hides the failure that matters: phrasings piling up while the ones being
    // used are the wrong ones.
    let pairs = store.all_pairs(usize::MAX)?;
    let tried: Vec<_> = pairs
        .iter()
        .filter(|p| p.successes + p.failures > 0)
        .collect();
    let wins: u32 = tried.iter().map(|p| p.successes).sum();
    let losses: u32 = tried.iter().map(|p| p.failures).sum();
    println!("phrasings:    {}", pairs.len());
    if !tried.is_empty() {
        println!(
            "  used:       {} of them, {wins} worked and {losses} did not",
            tried.len()
        );
        let weak = tried.iter().filter(|p| p.standing() < 0.5).count();
        println!("  below half: {weak}");
    }

    let learned = store
        .all_realizations()?
        .into_iter()
        .filter(|r| !matches!(r.provenance, spoon_concept::Provenance::Bootstrap))
        .count();
    println!("learned:      {learned} realizations beyond the bootstrap set");
    Ok(())
}

pub fn clean(cli: &Cli) -> Result<()> {
    let store = open_store(cli)?;
    let report = store.purge_stale_pairs()?;
    let retired = store.purge_stale_realizations()?;
    println!(
        "phrasings: removed {} stale, kept {}",
        report.removed, report.kept
    );
    println!("realizations: retired {retired} stale");
    if !report.examples.is_empty() {
        println!("examples of removed phrasings:");
        for ex in &report.examples {
            println!("  {ex}");
        }
    }
    if report.removed == 0 && retired == 0 {
        println!("brain is clean");
    }
    Ok(())
}

pub fn export(cli: &Cli, out: Option<&Path>) -> Result<()> {
    let store = open_store(cli)?;
    let seed = store.export_seed("spoon")?;
    let json = serde_json::to_string_pretty(&seed)?;
    match out {
        Some(path) => {
            std::fs::write(path, json)?;
            println!("wrote {}", path.display());
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub fn import(cli: &Cli, file: &Path) -> Result<()> {
    let store = open_store(cli)?;
    let seed = serde_json::from_str(&std::fs::read_to_string(file)?)?;
    let stats = store.import_seed(&seed)?;
    println!("imported {stats:?}");
    Ok(())
}

/// Mine the episodes for what this brain is failing at.
///
/// Clusters by symptom rather than by input, because input shape is unboundedly
/// variable by design and keying on it yields one cluster per episode, which
/// identifies nothing.
pub fn doctor(cli: &Cli, limit: usize) -> Result<()> {
    let store = open_store(cli)?;
    let episodes = store.recent_episodes(limit, None)?;
    if episodes.is_empty() {
        println!("no episodes yet");
        return Ok(());
    }
    let mut ears_failed = 0usize;
    let mut corrected = 0usize;
    let mut gaps: std::collections::HashMap<String, usize> = Default::default();
    let mut failed_realizations: std::collections::HashMap<String, usize> = Default::default();
    let mut model_ears = 0usize;

    for raw in &episodes {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        if v["ears_path"] == "Failed" {
            ears_failed += 1;
        }
        if v["ears_path"] == "Model" {
            model_ears += 1;
        }
        if v["correction"].is_string() {
            corrected += 1;
        }
        if let Some(list) = v["gaps"].as_array() {
            for gap in list {
                *gaps
                    .entry(gap.to_string().chars().take(60).collect())
                    .or_default() += 1;
            }
        }
        if let Some(list) = v["realizations"].as_array() {
            for entry in list {
                if entry[1] == false
                    && let Some(name) = entry[0].as_str()
                {
                    *failed_realizations.entry(name.to_string()).or_default() += 1;
                }
            }
        }
    }

    println!("episodes examined: {}", episodes.len());
    println!("ears fell back to the model: {model_ears}");
    println!("ears failed outright:        {ears_failed}");
    println!("user corrected the answer:   {corrected}");

    let mut gaps: Vec<_> = gaps.into_iter().collect();
    gaps.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    if !gaps.is_empty() {
        println!("\nmost common capability gaps:");
        for (gap, n) in gaps.iter().take(10) {
            println!("  {n:>4}  {gap}");
        }
    }
    let mut failures: Vec<_> = failed_realizations.into_iter().collect();
    failures.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    if !failures.is_empty() {
        println!("\nrealizations that failed most:");
        for (name, n) in failures.iter().take(10) {
            println!("  {n:>4}  {name}");
        }
    }
    Ok(())
}

pub async fn bench(cli: &Cli, suite: &str) -> Result<()> {
    crate::repl::bench(cli, suite).await
}

/// Run a curriculum against a brain.
///
/// A fresh brain knows its natives and nothing about the world, so the first
/// real conversation spends itself teaching vocabulary. A curriculum front-loads
/// that: ordinary turns, run in order, with everything they establish persisted
/// exactly as if a user had typed them. There is no separate teaching mode,
/// because a lesson that took a different path than a conversation would not be
/// teaching the thing that gets used.
pub async fn teach(cli: &Cli, file: &Path, limit: Option<usize>) -> Result<()> {
    let raw = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", file.display()))?;
    let curriculum: serde_json::Value = serde_json::from_str(&raw)?;
    let lessons: Vec<&str> = curriculum["lessons"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let lessons: Vec<&str> = match limit {
        Some(n) => lessons.into_iter().take(n).collect(),
        None => lessons,
    };

    let (mut brain, _ears_flag) = assemble(cli).await?;
    let before = open_store(cli)?.count_concepts()?;
    println!("teaching {} lessons", lessons.len());
    for (i, lesson) in lessons.iter().enumerate() {
        let result = brain.turn("teach", lesson).await?;
        let short: String = result.reply.chars().take(60).collect();
        println!("{:>3}  {:<52}  {}", i + 1, truncate(lesson, 50), short);
    }
    let after = open_store(cli)?.count_concepts()?;
    println!("\nconcepts: {before} -> {after}");
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

/// Name the shapes this brain keeps rebuilding.
pub fn consolidate(cli: &Cli, dry_run: bool) -> Result<()> {
    let store = open_store(cli)?;
    let bodies: Vec<spoon_concept::Concept> = store
        .all_realizations()?
        .iter()
        .filter_map(|r| match &r.spec {
            spoon_concept::RealizationSpec::Composed { body } => Some(body.clone()),
            _ => None,
        })
        .collect();

    if bodies.len() < 3 {
        println!(
            "only {} learned bodies; nothing to compress yet",
            bodies.len()
        );
        return Ok(());
    }

    let config = spoon_learn::ConsolidateConfig::default();
    if dry_run {
        let found = spoon_learn::consolidate(&bodies, config);
        println!("{} abstraction(s) would be named:", found.len());
        for a in &found {
            println!(
                "  {:<28} {} uses, utility {:.1}",
                a.name, a.instances, a.utility
            );
        }
        return Ok(());
    }

    let named = spoon_learn::consolidate_store(&store, config)?;
    println!("named {} abstraction(s):", named.len());
    for a in &named {
        println!(
            "  {:<28} {} uses, utility {:.1}",
            a.name, a.instances, a.utility
        );
    }
    Ok(())
}

/// Which stage is producing the wrong answers.
pub fn blame(cli: &Cli, limit: usize) -> Result<()> {
    let store = open_store(cli)?;
    let episodes: Vec<spoon_brain::Episode> = store
        .recent_episodes(limit, None)?
        .iter()
        .filter_map(|json| serde_json::from_str(json).ok())
        .collect();

    let unsatisfying = episodes.iter().filter(|e| e.looks_unsatisfying()).count();
    println!(
        "{} episodes, {unsatisfying} of them unsatisfying",
        episodes.len()
    );
    if unsatisfying == 0 {
        println!("nothing to apportion");
        return Ok(());
    }

    println!("\nblame by stage:");
    for (stage, weight) in spoon_brain::assign_all(&episodes) {
        let bar = "#".repeat((weight * 4.0).round() as usize);
        println!("  {:<16} {:>5.1}  {bar}", stage.as_str(), weight);
    }

    println!("\nworst turns:");
    for episode in episodes.iter().filter(|e| e.looks_unsatisfying()).take(8) {
        let blame = spoon_brain::assign_blame(episode);
        let suspect = blame
            .prime_suspect()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| "unclear".to_string());
        println!("  {:<12} {}", suspect, truncate(&episode.user_text, 60));
    }
    Ok(())
}

/// Evaluate a concept expression with no ears and no mouth in the way.
pub async fn eval(cli: &Cli, expression: &str) -> Result<()> {
    let store = open_store(cli)?;
    let registry = spoon_natives::bootstrap();
    spoon_natives::seed_bootstrap(&store, &registry)?;
    spoon_infer::seed_meta_rules(&store)?;
    let table = store.load_symbol_table()?;
    for name in registry.names() {
        table.intern(name.as_str());
    }

    let concept = spoon_concept::parse(expression, &table)
        .map_err(|e| anyhow::anyhow!("cannot parse {expression:?}: {e}"))?;

    let mut evaluator = spoon_eval::Evaluator::new(&store, &registry).with_permission(permission(
        cli.permissions.as_deref().unwrap_or("ask-writes"),
    ));
    let outcome = evaluator.evaluate(&concept);
    let trace = evaluator.trace();

    match &outcome {
        spoon_eval::Outcome::Value(v) => {
            println!("{}", spoon_concept::render(v, &table));
        }
        spoon_eval::Outcome::Stuck { concept, gaps } => {
            println!("stuck: {}", spoon_concept::render(concept, &table));
            for gap in gaps {
                println!(
                    "  no realization for {}",
                    spoon_concept::render(&gap.concept, &table)
                );
            }
        }
        spoon_eval::Outcome::NeedsPermission {
            effect,
            realization,
            ..
        } => {
            println!(
                "needs permission: {realization} wants {} access",
                effect.as_str()
            );
            println!("rerun with --permissions bypass to allow it");
        }
        other => println!("{other:?}"),
    }

    if std::env::var("SPOON_DEBUG").is_ok() {
        println!("\n{} nodes, {}ms", trace.nodes_used, trace.millis);
        for step in &trace.steps {
            println!(
                "  {:<40} {}",
                truncate(&spoon_concept::render(&step.concept, &table), 38),
                step.realization.as_deref().unwrap_or("(data)")
            );
        }
    }
    let _ = evaluator.commit_evidence();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Seat as SeatConfig;
    use spoon_seat::Transport;

    fn seat(provider: &str, model: &str) -> SeatConfig {
        SeatConfig {
            provider: Some(provider.to_string()),
            model: Some(model.to_string()),
            ..SeatConfig::default()
        }
    }

    #[test]
    fn no_configuration_falls_back_to_the_default_model() {
        let config = seat_config(&None, None, DEFAULT_MODEL).unwrap();
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.transport, Transport::Ollama);
    }

    #[test]
    fn config_file_beats_the_default() {
        let seat = seat("ollama", "qwen3.8:27b");
        let config = seat_config(&None, Some(&seat), DEFAULT_MODEL).unwrap();
        assert_eq!(config.model, "qwen3.8:27b");
    }

    #[test]
    fn flag_beats_the_config_file() {
        let seat = seat("ollama", "qwen3.8:27b");
        let flag = Some("qwen3.5:0.8b".to_string());
        let config = seat_config(&flag, Some(&seat), DEFAULT_MODEL).unwrap();
        assert_eq!(config.model, "qwen3.5:0.8b");
    }

    /// The whole point of reading `provider`: a seat can now be somewhere else.
    #[test]
    fn openai_provider_selects_the_openai_transport() {
        let seat = seat("openai", "gpt-4o");
        let config = seat_config(&None, Some(&seat), DEFAULT_MODEL).unwrap();
        assert_eq!(config.transport, Transport::OpenAi);
        assert_eq!(config.base_url, DEFAULT_OPENAI_URL);
    }

    #[test]
    fn base_url_and_timeout_override_the_provider_defaults() {
        let seat = SeatConfig {
            provider: Some("openai".to_string()),
            model: Some("local".to_string()),
            base_url: Some("http://localhost:8080/v1".to_string()),
            api_key_env: None,
            timeout_secs: Some(90),
        };
        let config = seat_config(&None, Some(&seat), DEFAULT_MODEL).unwrap();
        assert_eq!(config.base_url, "http://localhost:8080/v1");
        assert_eq!(config.timeout, std::time::Duration::from_secs(90));
    }

    /// A typo in the provider used to be silently ignored, which is how the
    /// teacher ran on the wrong model for a week.
    #[test]
    fn an_unknown_provider_is_an_error_rather_than_a_default() {
        let seat = seat("anthropic", "claude");
        let error = seat_config(&None, Some(&seat), DEFAULT_MODEL).unwrap_err();
        assert!(error.to_string().contains("unknown provider"));
    }
}
