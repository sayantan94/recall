use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "recall", about = "Record and search your terminal history")]
pub struct Cli {
    /// Ask a natural language question about your history
    #[arg(trailing_var_arg = true, num_args = 0..)]
    pub question: Vec<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
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
    /// Open the interactive TUI
    Ui,
    /// Open the web graph view
    Web {
        /// Port to serve on
        #[arg(long, default_value = "3141")]
        port: u16,
    },
}
