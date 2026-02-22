use anyhow::Result;
use chrono::Utc;

use crate::capture::context;
use crate::config::settings::load_config;
use crate::db::models::{Command, Session};
use crate::db::queries;
use crate::db::schema::open_db;
use crate::privacy::filter::should_ignore;

pub fn handle_log(
    command: &str,
    exit_code: Option<i32>,
    start_ms: Option<i64>,
    cwd: Option<&str>,
    session_id: &str,
    terminal: Option<&str>,
) -> Result<()> {
    let config = load_config()?;

    // Check privacy filters
    if should_ignore(command, &config.privacy.ignore_patterns) {
        return Ok(());
    }

    // Check if paused
    if crate::config::settings::pause_file().exists() {
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
    };
    queries::insert_command(&conn, &cmd)?;

    Ok(())
}
