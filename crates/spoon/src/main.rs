//! The Spoon binary.

mod build;
mod repl;
mod serve;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "spoon", about = "A persistent semantic cognitive system")]
struct Cli {
    /// Brain file. Defaults to ~/.spoon/spoon.db.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Use a throwaway in-memory brain.
    #[arg(long, global = true)]
    ephemeral: bool,
    /// Never consult a model. Everything still works, with less fluency.
    #[arg(long, global = true)]
    offline: bool,
    #[arg(long, global = true, default_value = "qwen3.5:4b")]
    ears_model: String,
    #[arg(long, global = true, default_value = "qwen3.5:4b")]
    mouth_model: String,
    #[arg(long, global = true, default_value = "qwen3.5:4b")]
    teacher_model: String,
    /// ask-writes (default), always-ask, or bypass.
    #[arg(long, global = true, default_value = "ask-writes")]
    permissions: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Talk to it.
    Repl,
    /// JSON lines on stdin and stdout.
    Stdio,
    /// OpenAI-compatible HTTP API plus the inspector.
    Serve {
        #[arg(long, default_value = "8787")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
    /// Run a corpus and report how it went.
    Bench {
        #[arg(default_value = "session")]
        suite: String,
    },
    /// What is this brain failing at?
    Doctor {
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Write a git-friendly seed.
    Export {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Load a seed.
    Import { file: PathBuf },
    /// What this brain knows.
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Repl => repl::run(&cli).await,
        Command::Stdio => repl::stdio(&cli).await,
        Command::Serve { port, ref host } => serve::run(&cli, host, port).await,
        Command::Bench { ref suite } => build::bench(&cli, suite).await,
        Command::Doctor { limit } => build::doctor(&cli, limit),
        Command::Export { ref out } => build::export(&cli, out.as_deref()),
        Command::Import { ref file } => build::import(&cli, file),
        Command::Status => build::status(&cli),
    }
}
