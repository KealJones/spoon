use std::path::PathBuf;
use clap::{Parser, Subcommand};

/// Spoon: non-LLM conversational AI
#[derive(Parser, Debug)]
#[command(name = "spoon", about = "Spoon - non-LLM conversational AI")]
pub struct Cli {
    /// SQLite database path (default: ~/.spoon/spoon.db or $SPOON_DB)
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Use in-memory database (not persisted, overrides --db)
    #[arg(long, global = true)]
    pub ephemeral: bool,

    /// Disable all LLM calls
    #[arg(long, global = true)]
    pub offline: bool,

    /// Enable trace-level debug output
    #[arg(long, global = true)]
    pub debug: bool,

    /// Permission mode for effectful primitives (always-ask | ask-writes | bypass)
    #[arg(long, global = true, default_value = "ask-writes")]
    pub permissions: String,

    /// Ears LLM model name
    #[arg(long, global = true)]
    pub ears_model: Option<String>,

    /// Mouth LLM model name
    #[arg(long, global = true)]
    pub mouth_model: Option<String>,

    /// Teacher LLM model name
    #[arg(long, global = true)]
    pub teacher_model: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Interactive line-oriented REPL on stdin/stdout
    Repl,

    /// JSON-lines protocol on stdin/stdout
    Stdio,

    /// Serve OpenAI-compatible HTTP API
    Serve {
        #[arg(long, default_value = "8787")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },

    /// Export learned knowledge to a seed JSON file
    Export {
        /// Output file (stdout if omitted)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Seed name embedded in the JSON
        #[arg(long, default_value = "spoon-seed")]
        name: String,
    },

    /// Import a seed JSON file into the store
    Import {
        /// Path to the seed JSON file
        file: PathBuf,
    },

    /// Run a teacher curriculum against the persistent store (online only)
    Teach {
        /// How many lessons to ask the teacher for
        #[arg(long, default_value = "10")]
        lessons: usize,
        /// Comma-separated curriculum themes (defaults built in)
        #[arg(long, value_delimiter = ',')]
        themes: Vec<String>,
        /// Wall-clock cap for the whole run
        #[arg(long, default_value = "30")]
        max_minutes: u64,
        /// Print the curriculum and exit without teaching
        #[arg(long)]
        dry_run: bool,
    },

    /// Run a benchmark corpus through the Brain and record the results
    Bench {
        /// Corpus name: convo20 | ace | babi | demo
        corpus: String,
    },

    /// Diagnose failure patterns from stored episodes
    Doctor {
        /// Look only at episodes since this window (e.g. 7d, 24h, 2w)
        #[arg(long)]
        since: Option<String>,
        /// Maximum number of episodes to analyse
        #[arg(long, default_value = "10000")]
        limit: usize,
        /// Write JSON to stdout instead of human-readable text
        #[arg(long)]
        json: bool,
        /// Show all builds instead of defaulting to the current build
        #[arg(long)]
        all: bool,
        /// Show only episodes from a specific build id
        #[arg(long)]
        build: Option<String>,
    },
}
