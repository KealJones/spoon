pub mod bench;
pub mod cli;
pub mod repl;
pub mod seedio;
pub mod server;
pub mod stdio;
pub mod teach;

use clap::Parser;
use spoon_core::kernel::PermissionMode;
use spoon_mind::brain::{Brain, BrainConfig, default_db_path};
use tracing_subscriber::EnvFilter;

use cli::{Cli, Command};

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Tracing: RUST_LOG env overrides; default to info or trace based on --debug.
    let default_level = if cli.debug { "spoon=trace,info" } else { "spoon=info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| EnvFilter::new(default_level)),
        )
        .init();

    let permission_mode = match cli.permissions.as_str() {
        "always-ask" => PermissionMode::AlwaysAsk,
        "bypass" => PermissionMode::Bypass,
        _ => PermissionMode::AskWrites,
    };

    let defaults = BrainConfig::default();
    let cfg = BrainConfig {
        db_path: if cli.ephemeral { None } else { cli.db.clone().or_else(|| Some(default_db_path())) },
        offline: cli.offline,
        debug: cli.debug,
        ears_model: cli.ears_model.clone().unwrap_or(defaults.ears_model),
        mouth_model: cli.mouth_model.clone().unwrap_or(defaults.mouth_model),
        teacher_model: cli.teacher_model.clone().unwrap_or(defaults.teacher_model),
        permission_mode,
        data_dir: None,
    };

    match cli.command {
        Command::Repl => {
            let brain = Brain::open(cfg).await?;
            repl::run(brain).await
        }
        Command::Stdio => {
            let brain = Brain::open(cfg).await?;
            stdio::run(brain).await
        }
        Command::Serve { host, port } => {
            let brain = Brain::open(cfg).await?;
            tracing::info!("serving on http://{host}:{port}");
            server::serve(brain, &host, port).await
        }
        Command::Export { out, name } => {
            seedio::export(cfg.db_path.as_deref(), &name, out.as_deref()).await
        }
        Command::Import { file } => {
            seedio::import(cfg.db_path.as_deref(), &file).await
        }
        Command::Teach { lessons, themes, max_minutes, dry_run } => {
            if cfg.offline {
                anyhow::bail!("spoon teach needs the teacher seat: drop --offline");
            }
            let brain = Brain::open(cfg).await?;
            teach::run(brain, teach::TeachArgs { lessons, themes, max_minutes, dry_run }).await
        }
        Command::Bench { corpus } => {
            let brain = Brain::open(cfg).await?;
            bench::run(brain, &corpus).await
        }
    }
}
