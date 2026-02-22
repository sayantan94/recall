use anyhow::{Context, Result};
use rusqlite::Connection;

pub fn initialize_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            start_time INTEGER NOT NULL,
            end_time INTEGER,
            terminal_app TEXT,
            initial_dir TEXT
        );

        CREATE TABLE IF NOT EXISTS commands (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            command_text TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            duration_ms INTEGER,
            cwd TEXT,
            git_repo TEXT,
            git_branch TEXT,
            exit_code INTEGER,
            FOREIGN KEY (session_id) REFERENCES sessions(id)
        );

        CREATE TABLE IF NOT EXISTS summaries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            summary_text TEXT NOT NULL,
            tags TEXT,
            intent TEXT,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id)
        );

        CREATE INDEX IF NOT EXISTS idx_commands_session ON commands(session_id);
        CREATE INDEX IF NOT EXISTS idx_commands_timestamp ON commands(timestamp);
        CREATE INDEX IF NOT EXISTS idx_commands_exit_code ON commands(exit_code);
        CREATE INDEX IF NOT EXISTS idx_commands_git_repo ON commands(git_repo);
        ",
    )
    .context("Failed to create base tables")?;

    // Create FTS5 tables (these don't support IF NOT EXISTS, so check first)
    let has_commands_fts: bool = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='commands_fts'")?
        .exists([])?;

    if !has_commands_fts {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE commands_fts USING fts5(
                command_text, cwd, git_repo, git_branch,
                content='commands', content_rowid='id'
            );

            CREATE TRIGGER commands_ai AFTER INSERT ON commands BEGIN
                INSERT INTO commands_fts(rowid, command_text, cwd, git_repo, git_branch)
                VALUES (new.id, new.command_text, new.cwd, new.git_repo, new.git_branch);
            END;
            ",
        )
        .context("Failed to create commands FTS table")?;
    }

    let has_summaries_fts: bool = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='summaries_fts'")?
        .exists([])?;

    if !has_summaries_fts {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE summaries_fts USING fts5(
                summary_text, tags,
                content='summaries', content_rowid='id'
            );

            CREATE TRIGGER summaries_ai AFTER INSERT ON summaries BEGIN
                INSERT INTO summaries_fts(rowid, summary_text, tags)
                VALUES (new.id, new.summary_text, new.tags);
            END;
            ",
        )
        .context("Failed to create summaries FTS table")?;
    }

    Ok(())
}

pub fn open_db() -> Result<Connection> {
    let db_path = crate::config::settings::db_path();
    crate::config::settings::ensure_recall_dir()?;
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open database at {}", db_path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    initialize_db(&conn)?;
    Ok(conn)
}
