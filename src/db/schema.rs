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

    // Migrate: add output column if it doesn't exist
    let has_output_col: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('commands') WHERE name='output'")?
        .exists([])?;

    if !has_output_col {
        conn.execute_batch("ALTER TABLE commands ADD COLUMN output TEXT;")
            .context("Failed to add output column")?;
    }

    // Create FTS5 tables (these don't support IF NOT EXISTS, so check first)
    let has_commands_fts: bool = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='commands_fts'")?
        .exists([])?;

    if !has_commands_fts {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE commands_fts USING fts5(
                command_text, cwd, git_repo, git_branch, output,
                content='commands', content_rowid='id'
            );

            CREATE TRIGGER commands_ai AFTER INSERT ON commands BEGIN
                INSERT INTO commands_fts(rowid, command_text, cwd, git_repo, git_branch, output)
                VALUES (new.id, new.command_text, new.cwd, new.git_repo, new.git_branch, new.output);
            END;
            ",
        )
        .context("Failed to create commands FTS table")?;
    } else {
        // Rebuild FTS to include output column if the existing FTS doesn't have it
        let fts_has_output: bool = conn
            .prepare(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='commands_fts' AND sql LIKE '%output%'",
            )?
            .exists([])?;

        if !fts_has_output {
            conn.execute_batch(
                "
                DROP TRIGGER IF EXISTS commands_ai;
                DROP TABLE IF EXISTS commands_fts;

                CREATE VIRTUAL TABLE commands_fts USING fts5(
                    command_text, cwd, git_repo, git_branch, output,
                    content='commands', content_rowid='id'
                );

                CREATE TRIGGER commands_ai AFTER INSERT ON commands BEGIN
                    INSERT INTO commands_fts(rowid, command_text, cwd, git_repo, git_branch, output)
                    VALUES (new.id, new.command_text, new.cwd, new.git_repo, new.git_branch, new.output);
                END;

                INSERT INTO commands_fts(rowid, command_text, cwd, git_repo, git_branch, output)
                    SELECT id, command_text, cwd, git_repo, git_branch, output FROM commands;
                ",
            )
            .context("Failed to rebuild commands FTS table with output")?;
        }
    }

    initialize_ai_tables(conn)?;

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

/// Tables for indexed AI assistant sessions (Claude Code, Codex).
///
/// Chunks — not whole sessions — are what FTS5 indexes, so a long conversation
/// stays findable by any part of it. The FTS table is external-content over
/// `ai_chunks` and needs delete/update triggers as well as insert, because
/// re-indexing a changed transcript replaces its chunks.
fn initialize_ai_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS ai_sessions (
            uid TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            session_id TEXT NOT NULL,
            project TEXT NOT NULL,
            title TEXT,
            started_at INTEGER NOT NULL,
            last_activity INTEGER NOT NULL,
            model TEXT,
            message_count INTEGER NOT NULL DEFAULT 0,
            file_path TEXT NOT NULL,
            file_mtime INTEGER NOT NULL,
            file_size INTEGER NOT NULL,
            indexed_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ai_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chunk_id TEXT NOT NULL UNIQUE,
            session_uid TEXT NOT NULL,
            source TEXT NOT NULL,
            project TEXT NOT NULL,
            title TEXT,
            timestamp INTEGER NOT NULL,
            text TEXT NOT NULL,
            FOREIGN KEY (session_uid) REFERENCES ai_sessions(uid) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_ai_sessions_activity ON ai_sessions(last_activity);
        CREATE INDEX IF NOT EXISTS idx_ai_sessions_project ON ai_sessions(project);
        CREATE INDEX IF NOT EXISTS idx_ai_sessions_source ON ai_sessions(source);
        CREATE INDEX IF NOT EXISTS idx_ai_chunks_session ON ai_chunks(session_uid);

        CREATE TABLE IF NOT EXISTS recall_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
    .context("Failed to create AI session tables")?;

    let has_ai_chunks_fts: bool = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='ai_chunks_fts'")?
        .exists([])?;

    if !has_ai_chunks_fts {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE ai_chunks_fts USING fts5(
                text, title, project,
                content='ai_chunks', content_rowid='id'
            );

            CREATE TRIGGER ai_chunks_ai AFTER INSERT ON ai_chunks BEGIN
                INSERT INTO ai_chunks_fts(rowid, text, title, project)
                VALUES (new.id, new.text, new.title, new.project);
            END;

            CREATE TRIGGER ai_chunks_ad AFTER DELETE ON ai_chunks BEGIN
                INSERT INTO ai_chunks_fts(ai_chunks_fts, rowid, text, title, project)
                VALUES ('delete', old.id, old.text, old.title, old.project);
            END;

            CREATE TRIGGER ai_chunks_au AFTER UPDATE ON ai_chunks BEGIN
                INSERT INTO ai_chunks_fts(ai_chunks_fts, rowid, text, title, project)
                VALUES ('delete', old.id, old.text, old.title, old.project);
                INSERT INTO ai_chunks_fts(rowid, text, title, project)
                VALUES (new.id, new.text, new.title, new.project);
            END;
            ",
        )
        .context("Failed to create AI chunk FTS table")?;
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
