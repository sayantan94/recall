use anyhow::Result;
use rusqlite::Connection;

use crate::db::models::{Command, SearchResult, SummarySearchResult};
use crate::db::queries;

pub struct SearchOptions {
    pub query: String,
    pub repo: Option<String>,
    pub dir: Option<String>,
    pub failed_only: bool,
    pub limit: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            repo: None,
            dir: None,
            failed_only: false,
            limit: 50,
        }
    }
}

/// Search commands using FTS5, with optional structured filters applied post-query.
pub fn search(conn: &Connection, opts: &SearchOptions) -> Result<Vec<SearchResult>> {
    let mut results = queries::search_commands(conn, &opts.query, opts.limit * 2)?;

    // Apply structured filters
    results.retain(|r| {
        if opts.failed_only && r.command.exit_code != Some(0) && r.command.exit_code.is_none() {
            // Keep commands with non-zero exit codes
        }
        if opts.failed_only {
            if let Some(code) = r.command.exit_code {
                if code == 0 {
                    return false;
                }
            } else {
                return false;
            }
        }
        if let Some(ref repo) = opts.repo {
            if r.command.git_repo.as_deref() != Some(repo.as_str()) {
                return false;
            }
        }
        if let Some(ref dir) = opts.dir {
            if let Some(ref cwd) = r.command.cwd {
                if !cwd.contains(dir.as_str()) {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    });

    results.truncate(opts.limit);
    Ok(results)
}

/// Search summaries using FTS5.
pub fn search_summaries(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SummarySearchResult>> {
    queries::search_summaries(conn, query, limit)
}

/// Get recent commands (for LLM context building).
pub fn get_recent_commands(conn: &Connection, limit: usize) -> Result<Vec<Command>> {
    queries::get_all_commands(conn, limit)
}
