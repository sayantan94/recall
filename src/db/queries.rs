use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use super::models::{Command, SearchResult, Session, Summary, SummarySearchResult};

pub fn insert_session(conn: &Connection, session: &Session) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO sessions (id, start_time, end_time, terminal_app, initial_dir) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session.id, session.start_time, session.end_time, session.terminal_app, session.initial_dir],
    ).context("Failed to insert session")?;
    Ok(())
}

pub fn insert_command(conn: &Connection, cmd: &Command) -> Result<i64> {
    conn.execute(
        "INSERT INTO commands (session_id, command_text, timestamp, duration_ms, cwd, git_repo, git_branch, exit_code)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            cmd.session_id,
            cmd.command_text,
            cmd.timestamp,
            cmd.duration_ms,
            cmd.cwd,
            cmd.git_repo,
            cmd.git_branch,
            cmd.exit_code,
        ],
    )
    .context("Failed to insert command")?;
    Ok(conn.last_insert_rowid())
}

pub fn search_commands(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.session_id, c.command_text, c.timestamp, c.duration_ms, c.cwd,
                c.git_repo, c.git_branch, c.exit_code, rank
         FROM commands_fts f
         JOIN commands c ON c.id = f.rowid
         WHERE commands_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;

    let results = stmt
        .query_map(params![query, limit as i64], |row| {
            Ok(SearchResult {
                command: Command {
                    id: Some(row.get(0)?),
                    session_id: row.get(1)?,
                    command_text: row.get(2)?,
                    timestamp: row.get(3)?,
                    duration_ms: row.get(4)?,
                    cwd: row.get(5)?,
                    git_repo: row.get(6)?,
                    git_branch: row.get(7)?,
                    exit_code: row.get(8)?,
                },
                rank: row.get(9)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to collect search results")?;

    Ok(results)
}

pub fn search_summaries(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<SummarySearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.session_id, s.summary_text, s.tags, s.intent, s.created_at, rank
         FROM summaries_fts f
         JOIN summaries s ON s.id = f.rowid
         WHERE summaries_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;

    let results = stmt
        .query_map(params![query, limit as i64], |row| {
            Ok(SummarySearchResult {
                summary: Summary {
                    id: Some(row.get(0)?),
                    session_id: row.get(1)?,
                    summary_text: row.get(2)?,
                    tags: row.get(3)?,
                    intent: row.get(4)?,
                    created_at: row.get(5)?,
                },
                rank: row.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to collect summary search results")?;

    Ok(results)
}

pub fn get_commands_today(conn: &Connection) -> Result<Vec<Command>> {
    let today_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .unwrap()
        .timestamp_millis();

    let mut stmt = conn.prepare(
        "SELECT id, session_id, command_text, timestamp, duration_ms, cwd, git_repo, git_branch, exit_code
         FROM commands
         WHERE timestamp >= ?1
         ORDER BY timestamp ASC",
    )?;

    let results = stmt
        .query_map(params![today_start], |row| {
            Ok(Command {
                id: Some(row.get(0)?),
                session_id: row.get(1)?,
                command_text: row.get(2)?,
                timestamp: row.get(3)?,
                duration_ms: row.get(4)?,
                cwd: row.get(5)?,
                git_repo: row.get(6)?,
                git_branch: row.get(7)?,
                exit_code: row.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to get today's commands")?;

    Ok(results)
}

pub fn get_commands_on_date(conn: &Connection, date: &str) -> Result<Vec<Command>> {
    let parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("Invalid date format: {date}. Use YYYY-MM-DD"))?;

    let start = parsed
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .unwrap()
        .timestamp_millis();
    let end = parsed
        .succ_opt()
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .unwrap()
        .timestamp_millis();

    let mut stmt = conn.prepare(
        "SELECT id, session_id, command_text, timestamp, duration_ms, cwd, git_repo, git_branch, exit_code
         FROM commands
         WHERE timestamp >= ?1 AND timestamp < ?2
         ORDER BY timestamp ASC",
    )?;

    let results = stmt
        .query_map(params![start, end], |row| {
            Ok(Command {
                id: Some(row.get(0)?),
                session_id: row.get(1)?,
                command_text: row.get(2)?,
                timestamp: row.get(3)?,
                duration_ms: row.get(4)?,
                cwd: row.get(5)?,
                git_repo: row.get(6)?,
                git_branch: row.get(7)?,
                exit_code: row.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to get commands for date")?;

    Ok(results)
}

pub fn get_session_commands(conn: &Connection, session_id: &str) -> Result<Vec<Command>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, command_text, timestamp, duration_ms, cwd, git_repo, git_branch, exit_code
         FROM commands
         WHERE session_id = ?1
         ORDER BY timestamp ASC",
    )?;

    let results = stmt
        .query_map(params![session_id], |row| {
            Ok(Command {
                id: Some(row.get(0)?),
                session_id: row.get(1)?,
                command_text: row.get(2)?,
                timestamp: row.get(3)?,
                duration_ms: row.get(4)?,
                cwd: row.get(5)?,
                git_repo: row.get(6)?,
                git_branch: row.get(7)?,
                exit_code: row.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to get session commands")?;

    Ok(results)
}

pub fn get_sessions(
    conn: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(
        "SELECT id, start_time, end_time, terminal_app, initial_dir
         FROM sessions
         ORDER BY start_time DESC
         LIMIT ?1 OFFSET ?2",
    )?;

    let results = stmt
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok(Session {
                id: row.get(0)?,
                start_time: row.get(1)?,
                end_time: row.get(2)?,
                terminal_app: row.get(3)?,
                initial_dir: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to get sessions")?;

    Ok(results)
}

pub fn insert_summary(conn: &Connection, summary: &Summary) -> Result<i64> {
    conn.execute(
        "INSERT INTO summaries (session_id, summary_text, tags, intent, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            summary.session_id,
            summary.summary_text,
            summary.tags,
            summary.intent,
            summary.created_at,
        ],
    )
    .context("Failed to insert summary")?;
    Ok(conn.last_insert_rowid())
}

pub fn get_unsummarized_sessions(conn: &Connection, min_commands: usize) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT s.id
         FROM sessions s
         LEFT JOIN summaries su ON su.session_id = s.id
         WHERE su.id IS NULL
         GROUP BY s.id
         HAVING (SELECT COUNT(*) FROM commands c WHERE c.session_id = s.id) >= ?1",
    )?;

    let results = stmt
        .query_map(params![min_commands as i64], |row| row.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()
        .context("Failed to get unsummarized sessions")?;

    Ok(results)
}

pub fn get_all_commands(conn: &Connection, limit: usize) -> Result<Vec<Command>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, command_text, timestamp, duration_ms, cwd, git_repo, git_branch, exit_code
         FROM commands
         ORDER BY timestamp DESC
         LIMIT ?1",
    )?;

    let results = stmt
        .query_map(params![limit as i64], |row| {
            Ok(Command {
                id: Some(row.get(0)?),
                session_id: row.get(1)?,
                command_text: row.get(2)?,
                timestamp: row.get(3)?,
                duration_ms: row.get(4)?,
                cwd: row.get(5)?,
                git_repo: row.get(6)?,
                git_branch: row.get(7)?,
                exit_code: row.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to get all commands")?;

    Ok(results)
}
