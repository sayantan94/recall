use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "recall",
    version,
    about = "Search your shell history and your AI agent sessions, locally"
)]
pub struct Cli {
    /// Ask a natural language question about your history
    #[arg(trailing_var_arg = true, num_args = 0..)]
    pub question: Vec<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Index your agent sessions and report what else is worth setting up
    Setup,
    /// Initialize shell hook (e.g., `eval "$(recall init zsh)"`)
    Init {
        /// Shell type
        shell: String,
    },
    /// Log a command (called by the shell hook)
    #[command(hide = true)]
    Log {
        #[arg(long)]
        command: String,
        #[arg(long)]
        exit_code: Option<i32>,
        #[arg(long)]
        start: Option<i64>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        session: String,
        #[arg(long)]
        terminal: Option<String>,
        #[arg(long)]
        output_file: Option<String>,
    },
    /// Generate a new session ID
    #[command(hide = true)]
    SessionId,
    /// Search command history
    Search {
        /// Search query
        query: String,
        /// Filter by git repo name
        #[arg(long)]
        repo: Option<String>,
        /// Filter by directory
        #[arg(long)]
        dir: Option<String>,
        /// Show only failed commands
        #[arg(long)]
        failed: bool,
        /// Max results
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Show today's commands
    Today,
    /// Show commands on a specific date (YYYY-MM-DD)
    On {
        date: String,
    },
    /// Pause recording
    Pause,
    /// Resume recording
    Resume,
    /// Summarize unsummarized sessions using LLM
    Summarize,
    /// Search and resume your AI agent sessions (Claude Code, Codex)
    Agents {
        #[command(subcommand)]
        command: Option<AgentsCommand>,
    },
    /// Open the interactive TUI
    Ui,
    /// Open the web graph view
    Web {
        /// Port to serve on
        #[arg(long, default_value = "3141")]
        port: u16,
    },
}

#[derive(Subcommand)]
pub enum AgentsCommand {
    /// Scan Claude Code and Codex transcripts and update the index
    Index {
        /// Re-read every transcript, even unchanged ones
        #[arg(long)]
        force: bool,
        /// Index only one source
        #[arg(long)]
        source: Option<String>,
    },
    /// Search indexed agent sessions
    Search {
        /// Search query
        query: String,
        #[command(flatten)]
        filters: AgentFilters,
        /// Match substrings instead of whole words
        #[arg(long)]
        fuzzy: bool,
    },
    /// List indexed agent sessions, newest first
    List {
        #[command(flatten)]
        filters: AgentFilters,
    },
    /// Print the transcript of one session
    Show {
        /// Session id, source-qualified id, or a unique prefix
        session: String,
    },
    /// Reopen a session in the tool that created it (defaults to the most recent)
    Resume {
        /// Session id, source-qualified id, or a unique prefix
        session: Option<String>,
        /// Run in this directory instead of the session's own
        #[arg(long)]
        dir: Option<String>,
        /// Print the command instead of running it
        #[arg(long)]
        print: bool,
    },
    /// Show what is currently indexed
    Stats,
}

#[derive(Args)]
pub struct AgentFilters {
    /// Filter by source (claude or codex)
    #[arg(long)]
    pub source: Option<String>,
    /// Filter by project path substring
    #[arg(long)]
    pub project: Option<String>,
    /// Max results
    #[arg(long, default_value = "20")]
    pub limit: usize,
    /// Skip the automatic index refresh
    #[arg(long)]
    pub no_index: bool,
}
