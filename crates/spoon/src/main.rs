//! The Spoon binary.

mod build;
mod config;
mod repl;
mod serve;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "spoon", about = "A persistent semantic cognitive system")]
struct Cli {
    /// Brain file. Defaults to ~/.spoon/spoon-v2.db.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Use a throwaway in-memory brain.
    #[arg(long, global = true)]
    ephemeral: bool,
    /// Never consult a model. Everything still works, with less fluency.
    #[arg(long, global = true)]
    offline: bool,
    /// Keep the ears and mouth, silence the Teacher.
    ///
    /// Teaching is what makes a run slow and what makes it change the brain.
    /// Turning it off measures the interior as it stands, which is the number
    /// you want when checking whether an edit helped.
    #[arg(long, global = true)]
    no_teaching: bool,
    /// Overrides ears.model in ~/.spoon/config.json.
    #[arg(long, global = true)]
    ears_model: Option<String>,
    /// Overrides mouth.model in ~/.spoon/config.json.
    #[arg(long, global = true)]
    mouth_model: Option<String>,
    /// Overrides teacher.model in ~/.spoon/config.json.
    #[arg(long, global = true)]
    teacher_model: Option<String>,
    /// ask-writes (default), always-ask, or bypass.
    #[arg(long, global = true)]
    permissions: Option<String>,
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
    /// Evaluate a concept expression directly, with no ears and no mouth.
    ///
    /// The interior on its own. Useful for seeing what Spoon can actually do
    /// separately from whether the model phrased the request well, which are
    /// different questions and get confused constantly.
    Eval {
        /// A concept expression, for example: sum<list<1, 2, 3>>
        expression: String,
    },
    /// Run a curriculum so a fresh brain starts with something.
    Teach {
        #[arg(long, default_value = "data/curriculum/basics.json")]
        file: PathBuf,
        /// Stop after this many lessons.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Name the shapes this brain keeps rebuilding.
    Consolidate {
        /// Report what would be named without storing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Which stage is producing the wrong answers.
    Blame {
        #[arg(long, default_value = "100")]
        limit: usize,
    },
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
        Command::Eval { ref expression } => build::eval(&cli, expression).await,
        Command::Teach { ref file, limit } => build::teach(&cli, file, limit).await,
        Command::Consolidate { dry_run } => build::consolidate(&cli, dry_run),
        Command::Blame { limit } => build::blame(&cli, limit),
    }
}
