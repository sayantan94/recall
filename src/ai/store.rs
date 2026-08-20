//! SQL for the AI session tables. Kept in one place so the indexer, the CLI and
//! the TUI all see the same data the same way.

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, Row};
use std::collections::HashMap;

use super::models::{AiSearchResult, AiSession, Chunk, Source};

/// The filters every listing and search shares.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub source: Option<Source>,
    /// Matched as a substring of the session's project path.
    pub project: Option<String>,
    pub limit: usize,
}

impl Filter {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }

    /// SQL predicate plus its bound values, for splicing into a WHERE clause.
    fn clauses(&self, alias: &str) -> (String, Vec<SqlValue>) {
        let mut sql = String::new();
        let mut values = Vec::new();

        if let Some(source) = self.source {
            sql.push_str(&format!(" AND {}.source = ?", alias));
            values.push(SqlValue::Text(source.as_str().to_string()));
        }
        if let Some(ref project) = self.project {
            sql.push_str(&format!(" AND {}.project LIKE ?", alias));
            values.push(SqlValue::Text(format!("%{}%", project)));
        }

        (sql, values)
    }
}

const SESSION_COLUMNS: &str = "uid, source, session_id, project, title, started_at, \
     last_activity, model, message_count, file_path, file_mtime, file_size, custom_name";

fn session_from_row(row: &Row, offset: usize) -> rusqlite::Result<AiSession> {
    let source: String = row.get(offset + 1)?;
    Ok(AiSession {
        uid: row.get(offset)?,
        source: Source::parse(&source).unwrap_or(Source::Claude),
        session_id: row.get(offset + 2)?,
        project: row.get(offset + 3)?,
        title: row.get(offset + 4)?,
        started_at: row.get(offset + 5)?,
        last_activity: row.get(offset + 6)?,
        model: row.get(offset + 7)?,
        message_count: row.get::<_, i64>(offset + 8)? as usize,
        file_path: row.get(offset + 9)?,
        file_mtime: row.get(offset + 10)?,
        file_size: row.get(offset + 11)?,
        custom_name: row.get(offset + 12)?,
    })
}

/// Build an FTS5 MATCH expression that treats every token as a literal and
/// requires all of them, so paths and flags like `--resume` can't be read as
/// query operators.
pub fn fts_query(raw: &str) -> String {
    raw.split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM recall_meta WHERE key = ?1")?;
    let mut rows = stmt.query_map(params![key], |row| row.get::<_, String>(0))?;
    match rows.next() {
        Some(value) => Ok(Some(value?)),
        None => Ok(None),
    }
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO recall_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn upsert_session(conn: &Connection, session: &AiSession, indexed_at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO ai_sessions (uid, source, session_id, project, title, started_at,
                                  last_activity, model, message_count, file_path,
                                  file_mtime, file_size, indexed_at, custom_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(uid) DO UPDATE SET
            project = excluded.project,
            title = excluded.title,
            custom_name = excluded.custom_name,
            started_at = excluded.started_at,
            last_activity = excluded.last_activity,
            model = excluded.model,
            message_count = excluded.message_count,
            file_path = excluded.file_path,
            file_mtime = excluded.file_mtime,
            file_size = excluded.file_size,
            indexed_at = excluded.indexed_at",
        params![
            session.uid,
            session.source.as_str(),
            session.session_id,
            session.project,
            session.title,
            session.started_at,
            session.last_activity,
            session.model,
            session.message_count as i64,
            session.file_path,
            session.file_mtime,
            session.file_size,
            indexed_at,
            session.custom_name,
        ],
    )
    .context("Failed to upsert AI session")?;
    Ok(())
}

pub fn insert_chunk(conn: &Connection, chunk: &Chunk) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO ai_chunks (chunk_id, session_uid, source, project, title, timestamp, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            chunk.chunk_id,
            chunk.session_uid,
            chunk.source.as_str(),
            chunk.project,
            chunk.title,
            chunk.timestamp,
            chunk.text,
        ],
    )
    .context("Failed to insert AI chunk")?;
    Ok(())
}

pub fn delete_chunks(conn: &Connection, session_uid: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM ai_chunks WHERE session_uid = ?1",
        params![session_uid],
    )?;
    Ok(())
}

pub fn delete_session(conn: &Connection, session_uid: &str) -> Result<()> {
    delete_chunks(conn, session_uid)?;
    conn.execute("DELETE FROM ai_sessions WHERE uid = ?1", params![session_uid])?;
    Ok(())
}

/// `uid -> (file_mtime, file_size)` for everything already indexed. The indexer
/// compares this against what's on disk to decide what needs re-reading.
pub fn indexed_fingerprints(conn: &Connection) -> Result<HashMap<String, (i64, i64)>> {
    let mut stmt = conn.prepare("SELECT uid, file_mtime, file_size FROM ai_sessions")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?)))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (uid, fingerprint) = row?;
        map.insert(uid, fingerprint);
    }
    Ok(map)
}

pub fn uids_for_source(conn: &Connection, source: Source) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT uid FROM ai_sessions WHERE source = ?1")?;
    let rows = stmt.query_map(params![source.as_str()], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn list_sessions(conn: &Connection, filter: &Filter) -> Result<Vec<AiSession>> {
    let (where_sql, mut values) = filter.clauses("s");
    let sql = format!(
        "SELECT {} FROM ai_sessions s WHERE 1=1{} ORDER BY last_activity DESC LIMIT ?",
        SESSION_COLUMNS, where_sql
    );
    values.push(SqlValue::Integer(filter.limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), |row| session_from_row(row, 0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn get_session(conn: &Connection, uid: &str) -> Result<Option<AiSession>> {
    let sql = format!("SELECT {} FROM ai_sessions WHERE uid = ?1", SESSION_COLUMNS);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![uid], |row| session_from_row(row, 0))?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Resolve a user-typed reference: a full uid (`claude:abc…`), a full native
/// session id, or a unique prefix of either.
pub fn resolve_session(conn: &Connection, reference: &str) -> Result<Vec<AiSession>> {
    if let Some(session) = get_session(conn, reference)? {
        return Ok(vec![session]);
    }

    let sql = format!(
        "SELECT {} FROM ai_sessions
         WHERE uid = ?1 OR session_id = ?1 OR uid LIKE ?2 OR session_id LIKE ?2
         ORDER BY last_activity DESC",
        SESSION_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let pattern = format!("{}%", reference);
    let rows = stmt.query_map(params![reference, pattern], |row| session_from_row(row, 0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Full-text search over indexed chunks, collapsed to one hit per session with
/// its best-matching excerpt.
pub fn search(conn: &Connection, query: &str, filter: &Filter) -> Result<Vec<AiSearchResult>> {
    let match_expr = fts_query(query);
    if match_expr.is_empty() {
        return Ok(Vec::new());
    }

    let (where_sql, mut values) = filter.clauses("s");
    // Over-fetch chunks: many hits collapse into the same session.
    let chunk_limit = (filter.limit * 8).max(50) as i64;

    let columns = SESSION_COLUMNS
        .split(", ")
        .map(|col| format!("s.{}", col.trim()))
        .collect::<Vec<_>>()
        .join(", ");

    // The FTS table is left unaliased: snippet() and rank must name it directly.
    let sql = format!(
        "SELECT {}, snippet(ai_chunks_fts, 0, '', '', '…', 26), ai_chunks_fts.rank
         FROM ai_chunks_fts
         JOIN ai_chunks c ON c.id = ai_chunks_fts.rowid
         JOIN ai_sessions s ON s.uid = c.session_uid
         WHERE ai_chunks_fts MATCH ?{}
         ORDER BY ai_chunks_fts.rank
         LIMIT ?",
        columns, where_sql
    );

    // Bound values follow the order the placeholders appear in: MATCH, filters, limit.
    values.insert(0, SqlValue::Text(match_expr));
    values.push(SqlValue::Integer(chunk_limit));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), |row| {
        Ok(AiSearchResult {
            session: session_from_row(row, 0)?,
            snippet: row.get(13)?,
            rank: row.get(14)?,
        })
    })?;

    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();
    for row in rows {
        let result = row?;
        // Rows arrive best-rank-first, so the first hit per session is its best.
        if seen.insert(result.session.uid.clone()) {
            results.push(result);
            if results.len() >= filter.limit {
                break;
            }
        }
    }

    Ok(results)
}

/// Case-insensitive substring search, for when you remember a fragment rather
/// than a word — FTS5 only matches whole tokens.
pub fn search_fuzzy(conn: &Connection, query: &str, filter: &Filter) -> Result<Vec<AiSearchResult>> {
    let needle = query.trim();
    if needle.is_empty() {
        return Ok(Vec::new());
    }

    let (where_sql, mut values) = filter.clauses("s");
    let columns = SESSION_COLUMNS
        .split(", ")
        .map(|col| format!("s.{}", col.trim()))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "SELECT {}, c.text
         FROM ai_chunks c
         JOIN ai_sessions s ON s.uid = c.session_uid
         WHERE c.text LIKE ? ESCAPE '\\'{}
         ORDER BY c.timestamp DESC
         LIMIT ?",
        columns, where_sql
    );

    values.insert(0, SqlValue::Text(format!("%{}%", escape_like(needle))));
    values.push(SqlValue::Integer((filter.limit * 8).max(50) as i64));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), |row| {
        let text: String = row.get(13)?;
        Ok(AiSearchResult {
            session: session_from_row(row, 0)?,
            snippet: excerpt_around(&text, needle),
            rank: 0.0,
        })
    })?;

    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();
    for row in rows {
        let result = row?;
        if seen.insert(result.session.uid.clone()) {
            results.push(result);
            if results.len() >= filter.limit {
                break;
            }
        }
    }

    Ok(results)
}

fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A short window of text centred on the match, so results read like FTS5 snippets.
fn excerpt_around(text: &str, needle: &str) -> String {
    const WINDOW: usize = 90;
    let lower_text = text.to_lowercase();
    let position = lower_text.find(&needle.to_lowercase()).unwrap_or(0);

    let chars: Vec<char> = text.chars().collect();
    // `position` is a byte offset; convert it to a char offset.
    let char_position = text[..position].chars().count();
    let start = char_position.saturating_sub(WINDOW / 2);
    let end = (char_position + needle.chars().count() + WINDOW / 2).min(chars.len());

    let mut excerpt = String::new();
    if start > 0 {
        excerpt.push('…');
    }
    excerpt.extend(chars[start..end].iter());
    if end < chars.len() {
        excerpt.push('…');
    }
    excerpt.replace('\n', " ")
}

pub fn session_chunks(conn: &Connection, session_uid: &str) -> Result<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        "SELECT chunk_id, session_uid, source, project, title, timestamp, text
         FROM ai_chunks WHERE session_uid = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![session_uid], |row| {
        let source: String = row.get(2)?;
        Ok(Chunk {
            chunk_id: row.get(0)?,
            session_uid: row.get(1)?,
            source: Source::parse(&source).unwrap_or(Source::Claude),
            project: row.get(3)?,
            title: row.get(4)?,
            timestamp: row.get(5)?,
            text: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub sessions: usize,
    pub chunks: usize,
    pub per_source: Vec<(Source, usize)>,
    pub projects: usize,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let sessions: i64 = conn.query_row("SELECT COUNT(*) FROM ai_sessions", [], |r| r.get(0))?;
    let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM ai_chunks", [], |r| r.get(0))?;
    let projects: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT project) FROM ai_sessions",
        [],
        |r| r.get(0),
    )?;

    let mut per_source = Vec::new();
    for source in Source::ALL {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_sessions WHERE source = ?1",
            params![source.as_str()],
            |r| r.get(0),
        )?;
        per_source.push((source, count as usize));
    }

    Ok(Stats {
        sessions: sessions as usize,
        chunks: chunks as usize,
        per_source,
        projects: projects as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::session_uid;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::schema::initialize_db(&conn).unwrap();
        conn
    }

    fn sample(source: Source, id: &str, project: &str, activity: i64) -> AiSession {
        AiSession {
            uid: session_uid(source, id),
            source,
            session_id: id.into(),
            project: project.into(),
            title: Some(format!("title for {}", id)),
            started_at: activity,
            last_activity: activity,
            model: Some("test-model".into()),
            message_count: 2,
            file_path: format!("/tmp/{}.jsonl", id),
            file_mtime: activity,
            file_size: 128,
            custom_name: None,
        }
    }

    fn chunk_for(session: &AiSession, index: usize, text: &str) -> Chunk {
        Chunk {
            chunk_id: format!("{}:{}", session.uid, index),
            session_uid: session.uid.clone(),
            source: session.source,
            project: session.project.clone(),
            title: session.title.clone(),
            timestamp: session.started_at,
            text: text.into(),
        }
    }

    #[test]
    fn meta_round_trips_and_overwrites() {
        let conn = test_db();
        assert_eq!(get_meta(&conn, "index_format").unwrap(), None);

        set_meta(&conn, "index_format", "2").unwrap();
        assert_eq!(get_meta(&conn, "index_format").unwrap().as_deref(), Some("2"));

        set_meta(&conn, "index_format", "3").unwrap();
        assert_eq!(get_meta(&conn, "index_format").unwrap().as_deref(), Some("3"));
    }

    #[test]
    fn fts_query_quotes_every_token() {
        assert_eq!(fts_query("recall --resume"), "\"recall\" AND \"--resume\"");
        assert_eq!(fts_query("  "), "");
    }

    #[test]
    fn upsert_then_read_back() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/repos/one", 1000);
        upsert_session(&conn, &session, 1).unwrap();

        let loaded = get_session(&conn, &session.uid).unwrap().unwrap();
        assert_eq!(loaded.session_id, "abc");
        assert_eq!(loaded.source, Source::Claude);
        assert_eq!(loaded.message_count, 2);
    }

    #[test]
    fn upsert_updates_rather_than_duplicates() {
        let conn = test_db();
        let mut session = sample(Source::Codex, "xyz", "/repos/two", 1000);
        upsert_session(&conn, &session, 1).unwrap();
        session.message_count = 9;
        session.last_activity = 5000;
        upsert_session(&conn, &session, 2).unwrap();

        assert_eq!(stats(&conn).unwrap().sessions, 1);
        let loaded = get_session(&conn, &session.uid).unwrap().unwrap();
        assert_eq!(loaded.message_count, 9);
        assert_eq!(loaded.last_activity, 5000);
    }

    #[test]
    fn a_saved_name_round_trips_and_can_be_cleared() {
        let conn = test_db();
        let mut session = sample(Source::Claude, "abc", "/repos/one", 1000);
        session.custom_name = Some("appliedIn".into());
        session.title = Some("appliedIn".into());
        upsert_session(&conn, &session, 1).unwrap();
        assert_eq!(
            get_session(&conn, &session.uid).unwrap().unwrap().custom_name.as_deref(),
            Some("appliedIn")
        );

        // Renaming it away in the tool must clear the stored name too.
        session.custom_name = None;
        session.title = Some("the opening prompt".into());
        upsert_session(&conn, &session, 2).unwrap();

        let reloaded = get_session(&conn, &session.uid).unwrap().unwrap();
        assert_eq!(reloaded.custom_name, None, "the old name does not linger");
        assert_eq!(reloaded.title.as_deref(), Some("the opening prompt"));
    }

    #[test]
    fn search_finds_chunk_text_and_collapses_by_session() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/repos/one", 1000);
        upsert_session(&conn, &session, 1).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 0, "USER: fix the flaky retry logic")).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 1, "ASSISTANT: the retry logic now backs off")).unwrap();

        let results = search(&conn, "retry", &Filter::with_limit(10)).unwrap();
        assert_eq!(results.len(), 1, "both chunks belong to one session");
        assert_eq!(results[0].session.uid, session.uid);
        assert!(results[0].snippet.to_lowercase().contains("retry"));
    }

    #[test]
    fn search_requires_all_tokens() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/repos/one", 1000);
        upsert_session(&conn, &session, 1).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 0, "docker compose build")).unwrap();

        assert_eq!(search(&conn, "docker compose", &Filter::with_limit(10)).unwrap().len(), 1);
        assert!(search(&conn, "docker kubernetes", &Filter::with_limit(10)).unwrap().is_empty());
    }

    #[test]
    fn search_honours_source_and_project_filters() {
        let conn = test_db();
        let claude = sample(Source::Claude, "a", "/repos/alpha", 1000);
        let codex = sample(Source::Codex, "b", "/repos/beta", 2000);
        upsert_session(&conn, &claude, 1).unwrap();
        upsert_session(&conn, &codex, 1).unwrap();
        insert_chunk(&conn, &chunk_for(&claude, 0, "shared keyword here")).unwrap();
        insert_chunk(&conn, &chunk_for(&codex, 0, "shared keyword here")).unwrap();

        let by_source = search(
            &conn,
            "keyword",
            &Filter {
                source: Some(Source::Codex),
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_source.len(), 1);
        assert_eq!(by_source[0].session.source, Source::Codex);

        let by_project = search(
            &conn,
            "keyword",
            &Filter {
                project: Some("alpha".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_project.len(), 1);
        assert_eq!(by_project[0].session.project, "/repos/alpha");
    }

    #[test]
    fn deleting_a_session_removes_it_from_the_index() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/repos/one", 1000);
        upsert_session(&conn, &session, 1).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 0, "unique-token-xyz")).unwrap();
        assert_eq!(search(&conn, "unique-token-xyz", &Filter::with_limit(5)).unwrap().len(), 1);

        delete_session(&conn, &session.uid).unwrap();
        assert!(search(&conn, "unique-token-xyz", &Filter::with_limit(5)).unwrap().is_empty());
        assert_eq!(stats(&conn).unwrap().sessions, 0);
    }

    #[test]
    fn replacing_chunks_drops_stale_text_from_the_index() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/repos/one", 1000);
        upsert_session(&conn, &session, 1).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 0, "old-token")).unwrap();

        delete_chunks(&conn, &session.uid).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 0, "new-token")).unwrap();

        assert!(search(&conn, "old-token", &Filter::with_limit(5)).unwrap().is_empty());
        assert_eq!(search(&conn, "new-token", &Filter::with_limit(5)).unwrap().len(), 1);
    }

    #[test]
    fn fuzzy_search_matches_partial_words() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/repos/one", 1000);
        upsert_session(&conn, &session, 1).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 0, "we refactored the SessionIndexer today")).unwrap();

        // FTS5 tokenizes on word boundaries, so a mid-word fragment misses.
        assert!(search(&conn, "ionIndex", &Filter::with_limit(5)).unwrap().is_empty());
        let fuzzy = search_fuzzy(&conn, "ionIndex", &Filter::with_limit(5)).unwrap();
        assert_eq!(fuzzy.len(), 1);
        assert!(fuzzy[0].snippet.contains("SessionIndexer"));
    }

    #[test]
    fn fuzzy_search_treats_wildcards_literally() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/repos/one", 1000);
        upsert_session(&conn, &session, 1).unwrap();
        insert_chunk(&conn, &chunk_for(&session, 0, "plain text without wildcards")).unwrap();

        assert!(search_fuzzy(&conn, "%", &Filter::with_limit(5)).unwrap().is_empty());
    }

    #[test]
    fn list_sessions_is_newest_first() {
        let conn = test_db();
        upsert_session(&conn, &sample(Source::Claude, "old", "/p", 1000), 1).unwrap();
        upsert_session(&conn, &sample(Source::Codex, "new", "/p", 9000), 1).unwrap();

        let listed = list_sessions(&conn, &Filter::with_limit(10)).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].session_id, "new");
    }

    #[test]
    fn resolve_session_accepts_uid_native_id_and_prefix() {
        let conn = test_db();
        let session = sample(Source::Claude, "abcdef123", "/p", 1000);
        upsert_session(&conn, &session, 1).unwrap();

        assert_eq!(resolve_session(&conn, "claude:abcdef123").unwrap().len(), 1);
        assert_eq!(resolve_session(&conn, "abcdef123").unwrap().len(), 1);
        assert_eq!(resolve_session(&conn, "abcd").unwrap().len(), 1);
        assert!(resolve_session(&conn, "nope").unwrap().is_empty());
    }

    #[test]
    fn fingerprints_report_what_is_indexed() {
        let conn = test_db();
        let session = sample(Source::Claude, "abc", "/p", 1000);
        upsert_session(&conn, &session, 1).unwrap();

        let prints = indexed_fingerprints(&conn).unwrap();
        assert_eq!(prints.get(&session.uid), Some(&(1000, 128)));
    }
}
