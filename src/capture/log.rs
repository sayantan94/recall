use anyhow::Result;
use chrono::Utc;
use regex::Regex;

use crate::capture::context;
use crate::config::settings::load_config;
use crate::db::models::{Command, Session};
use crate::db::queries;
use crate::db::schema::open_db;
use crate::privacy::filter::should_ignore;

const MAX_OUTPUT_BYTES: usize = 10 * 1024; // 10KB

/// Read a temp file, strip ANSI escape codes, truncate to 10KB, and delete the file.
fn read_output_file(path: &str) -> Option<String> {
    let content = std::fs::read(path).ok()?;
    // Always try to clean up the temp file
    let _ = std::fs::remove_file(path);

    if content.is_empty() {
        return None;
    }

    // Truncate raw bytes to limit before string conversion
    let truncated = if content.len() > MAX_OUTPUT_BYTES {
        &content[..MAX_OUTPUT_BYTES]
    } else {
        &content[..]
    };

    // Convert to string lossy (handles non-UTF8 gracefully)
    let text = String::from_utf8_lossy(truncated);

    // Strip ANSI escape codes
    let ansi_re = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    let cleaned = ansi_re.replace_all(&text, "").to_string();

    if cleaned.trim().is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

pub fn handle_log(
    command: &str,
    exit_code: Option<i32>,
    start_ms: Option<i64>,
    cwd: Option<&str>,
    session_id: &str,
    terminal: Option<&str>,
    output_file: Option<&str>,
) -> Result<()> {
    let config = load_config()?;

    // Check privacy filters
    if should_ignore(command, &config.privacy.ignore_patterns) {
        // Still clean up the temp file if present
        if let Some(path) = output_file {
            let _ = std::fs::remove_file(path);
        }
        return Ok(());
    }

    // Check if paused
    if crate::config::settings::pause_file().exists() {
        if let Some(path) = output_file {
            let _ = std::fs::remove_file(path);
        }
        return Ok(());
    }

    let now = Utc::now().timestamp_millis();
    let duration_ms = start_ms.map(|s| now - s);

    // Detect git context
    let (git_repo, git_branch) = if let Some(dir) = cwd {
        (context::detect_git_repo(dir), context::detect_git_branch(dir))
    } else {
        (None, None)
    };

    // Read and process output file
    let output = output_file.and_then(read_output_file);

    let conn = open_db()?;

    // Ensure session exists
    let session = Session {
        id: session_id.to_string(),
        start_time: start_ms.unwrap_or(now),
        end_time: None,
        terminal_app: terminal.map(|s| s.to_string()),
        initial_dir: cwd.map(|s| s.to_string()),
    };
    queries::insert_session(&conn, &session)?;

    // Insert command
    let cmd = Command {
        id: None,
        session_id: session_id.to_string(),
        command_text: command.to_string(),
        timestamp: start_ms.unwrap_or(now),
        duration_ms,
        cwd: cwd.map(|s| s.to_string()),
        git_repo,
        git_branch,
        exit_code,
        output,
    };
    queries::insert_command(&conn, &cmd)?;

    Ok(())
}
