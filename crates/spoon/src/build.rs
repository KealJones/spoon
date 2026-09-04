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

/// Whichever implementations the configuration ended up with.
type SeatTrio = (Box<dyn Ears>, Box<dyn Mouth>, Option<Box<dyn Teacher>>);

pub fn brain_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.db {
        return Ok(path.clone());
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    let dir = PathBuf::from(home).join(".spoon");
    std::fs::create_dir_all(&dir)?;
    // Deliberately not spoon.db: that is the v1 brain, whose schema this build
    // cannot read. Keeping a separate file means an existing v1 brain survives
    // untouched rather than being half-migrated into something neither system
    // can open.
    Ok(dir.join("spoon-v2.db"))
}

pub fn open_store(cli: &Cli) -> Result<Store> {
    if cli.ephemeral {
        return Ok(Store::open_in_memory()?);
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
pub async fn assemble(cli: &Cli) -> Result<Brain> {
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
        LlmClient::new(LlmConfig::ollama(&cli.ears_model), counters.clone())
            .reachable()
            .await
    };

    let (ears, mouth, teacher): SeatTrio = if online {
        (
            Box::new(ModelEars::new(LlmClient::new(
                LlmConfig::ollama(&cli.ears_model),
                counters.clone(),
            ))),
            Box::new(ModelMouth::new(
                LlmClient::new(LlmConfig::ollama(&cli.mouth_model), counters.clone()),
                table.clone(),
            )),
            Some(Box::new(ModelTeacher::new(
                LlmClient::new(LlmConfig::ollama(&cli.teacher_model), counters.clone()),
                table.clone(),
            ))),
        )
    } else {
        (
            Box::new(NativeEars::new()),
            Box::new(TemplateMouth::new(table.clone())),
            None,
        )
    };

    let config = BrainConfig {
        permission: permission(&cli.permissions),
        teaching: online,
        ..BrainConfig::default()
    };
    let seats = Seats {
        ears,
        mouth,
        teacher,
        counters,
    };
    Ok(Brain::new(store, registry, table, seats, config)?)
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
