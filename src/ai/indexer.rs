//! Reconciled incremental indexing.
//!
//! Every run compares what is on disk against what is in the database: new and
//! changed transcripts are re-read, unchanged ones are skipped, and sessions
//! whose files have disappeared are dropped from the index.

use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::HashSet;

use super::chunker::chunk_session;
use super::models::{AiSession, Source};
use super::sources::source_for;
use super::store;

/// Bumped whenever parsing or chunking changes what a transcript turns into.
/// An index written by an older format is rebuilt from scratch rather than
/// left holding text this build would never produce — for example the injected
/// Codex context and recall's own headless prompts, which earlier versions
/// indexed and this one filters out.
pub const INDEX_FORMAT: u32 = 2;

const INDEX_FORMAT_KEY: &str = "index_format";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub chunks: usize,
    /// Set when a stale index was discarded and rebuilt.
    pub rebuilt: bool,
    /// Transcripts that could not be parsed, with the reason.
    pub failed: Vec<(String, String)>,
}

impl IndexReport {
    pub fn merge(&mut self, other: IndexReport) {
        self.added += other.added;
        self.updated += other.updated;
        self.unchanged += other.unchanged;
        self.removed += other.removed;
        self.chunks += other.chunks;
        self.rebuilt |= other.rebuilt;
        self.failed.extend(other.failed);
    }
}

/// Index every supported source. `force` re-reads transcripts even when their
/// size and mtime are unchanged.
///
/// An index left behind by an older build is rebuilt automatically, so
/// upgrading never leaves stale text behind to be found in searches.
pub fn index_all(conn: &Connection, force: bool) -> Result<IndexReport> {
    let stale = index_is_stale(conn)?;
    let mut report = IndexReport {
        rebuilt: stale,
        ..Default::default()
    };

    for source in Source::ALL {
        report.merge(index_source(conn, source, force || stale)?);
    }

    store::set_meta(conn, INDEX_FORMAT_KEY, &INDEX_FORMAT.to_string())?;
    report.rebuilt = stale;
    Ok(report)
}

/// True when the index on disk was written by a build that parsed transcripts
/// differently. A brand-new index is not stale — there is nothing to discard.
fn index_is_stale(conn: &Connection) -> Result<bool> {
    if store::stats(conn)?.sessions == 0 {
        return Ok(false);
    }

    let recorded = store::get_meta(conn, INDEX_FORMAT_KEY)?
        .and_then(|value| value.parse::<u32>().ok())
        // No marker at all means it predates version tracking.
        .unwrap_or(0);

    Ok(recorded != INDEX_FORMAT)
}

pub fn index_source(conn: &Connection, source: Source, force: bool) -> Result<IndexReport> {
    let handler = source_for(source);
    let on_disk = handler.list_sessions()?;
    let fingerprints = store::indexed_fingerprints(conn)?;
    let indexed_at = Utc::now().timestamp_millis();

    let mut report = IndexReport::default();
    let mut seen: HashSet<String> = HashSet::new();

    for mut session in on_disk {
        seen.insert(session.uid.clone());

        let known = fingerprints.get(&session.uid);
        let is_new = known.is_none();
        if !force {
            if let Some(&(mtime, size)) = known {
                if mtime == session.file_mtime && size == session.file_size {
                    report.unchanged += 1;
                    continue;
                }
            }
        }

        let messages = match handler.load_messages(&session) {
            Ok(messages) => messages,
            Err(error) => {
                report.failed.push((session.file_path.clone(), error.to_string()));
                continue;
            }
        };

        if is_recall_generated(&messages) {
            // recall's own headless runs are not conversations worth finding.
            // Delete rather than skip, so transcripts recorded before recall
            // started passing --no-session-persistence get cleaned out too.
            if !is_new {
                store::delete_session(conn, &session.uid)?;
                report.removed += 1;
            }
            continue;
        }

        enrich(&mut session, &messages);
        let chunks = chunk_session(&session, &messages);

        store::upsert_session(conn, &session, indexed_at)?;
        store::delete_chunks(conn, &session.uid)?;
        for chunk in &chunks {
            store::insert_chunk(conn, chunk)?;
        }

        report.chunks += chunks.len();
        if is_new {
            report.added += 1;
        } else {
            report.updated += 1;
        }
    }

    // Drop sessions this source no longer has on disk.
    for uid in store::uids_for_source(conn, source)? {
        if !seen.contains(&uid) {
            store::delete_session(conn, &uid)?;
            report.removed += 1;
        }
    }

    Ok(report)
}

/// True when this transcript is a headless run recall itself started.
fn is_recall_generated(messages: &[super::models::Message]) -> bool {
    messages
        .iter()
        .find(|m| m.role == super::models::Role::User)
        .is_some_and(|m| crate::ai::sources::claude_code::is_recall_own_prompt(&m.text))
}

/// Fill in what only a full parse can tell us: how many messages the session
/// holds, when it was last active, and a title when the tool didn't record one.
fn enrich(session: &mut AiSession, messages: &[super::models::Message]) {
    session.message_count = messages.len();

    if let Some(ts) = messages.iter().rev().find_map(|m| m.timestamp) {
        session.last_activity = session.last_activity.max(ts);
    }
    if let Some(ts) = messages.iter().find_map(|m| m.timestamp) {
        session.started_at = ts;
    }

    if session.title.as_ref().is_none_or(|t| t.trim().is_empty()) {
        session.title = messages
            .iter()
            .find(|m| m.role == super::models::Role::User)
            .map(|m| first_line_summary(&m.text));
    }
}

/// A one-line title taken from the opening prompt.
fn first_line_summary(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let summary: String = line.chars().take(120).collect();
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::{session_uid, Message, Role};

    fn msg(role: Role, text: &str, ts: Option<i64>) -> Message {
        Message {
            role,
            text: text.into(),
            timestamp: ts,
            tool_names: vec![],
        }
    }

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::schema::initialize_db(&conn).unwrap();
        conn
    }

    fn seed_one_session(conn: &rusqlite::Connection) {
        store::upsert_session(conn, &session(), 0).unwrap();
    }

    fn session() -> AiSession {
        AiSession {
            uid: session_uid(Source::Claude, "s1"),
            source: Source::Claude,
            session_id: "s1".into(),
            project: "/p".into(),
            title: None,
            started_at: 500,
            last_activity: 500,
            model: None,
            message_count: 0,
            file_path: "/tmp/s1.jsonl".into(),
            file_mtime: 500,
            file_size: 10,
        }
    }

    #[test]
    fn enrich_counts_messages_and_extends_activity() {
        let mut s = session();
        let messages = vec![
            msg(Role::User, "first", Some(1000)),
            msg(Role::Assistant, "reply", Some(2000)),
        ];
        enrich(&mut s, &messages);
        assert_eq!(s.message_count, 2);
        assert_eq!(s.started_at, 1000);
        assert_eq!(s.last_activity, 2000);
    }

    #[test]
    fn enrich_titles_from_the_first_user_message() {
        let mut s = session();
        let messages = vec![msg(Role::User, "\n  fix the parser\nmore detail", None)];
        enrich(&mut s, &messages);
        assert_eq!(s.title.as_deref(), Some("fix the parser"));
    }

    #[test]
    fn enrich_keeps_an_existing_title() {
        let mut s = session();
        s.title = Some("tool supplied".into());
        enrich(&mut s, &[msg(Role::User, "something else", None)]);
        assert_eq!(s.title.as_deref(), Some("tool supplied"));
    }

    #[test]
    fn enrich_never_moves_activity_backwards() {
        let mut s = session();
        s.last_activity = 9000;
        enrich(&mut s, &[msg(Role::User, "old", Some(1000))]);
        assert_eq!(s.last_activity, 9000);
    }

    #[test]
    fn recall_own_headless_runs_are_skipped() {
        let own = vec![msg(
            Role::User,
            "You are a terminal history assistant. The user is asking...",
            None,
        )];
        assert!(is_recall_generated(&own));

        let real = vec![msg(Role::User, "fix the flaky test", None)];
        assert!(!is_recall_generated(&real));
        assert!(!is_recall_generated(&[]));
    }

    #[test]
    fn a_fresh_index_is_not_stale() {
        let conn = test_conn();
        assert!(!index_is_stale(&conn).unwrap(), "nothing indexed yet");
    }

    #[test]
    fn an_index_from_an_older_build_is_stale() {
        let conn = test_conn();
        seed_one_session(&conn);

        // No marker: written before version tracking existed.
        assert!(index_is_stale(&conn).unwrap());

        store::set_meta(&conn, INDEX_FORMAT_KEY, "1").unwrap();
        assert!(index_is_stale(&conn).unwrap(), "an older format is stale");

        store::set_meta(&conn, INDEX_FORMAT_KEY, &INDEX_FORMAT.to_string()).unwrap();
        assert!(!index_is_stale(&conn).unwrap(), "the current format is not");
    }

    #[test]
    fn report_merge_sums_counters() {
        let mut a = IndexReport {
            added: 1,
            updated: 2,
            unchanged: 3,
            removed: 4,
            chunks: 5,
            rebuilt: false,
            failed: vec![("x".into(), "boom".into())],
        };
        a.merge(IndexReport {
            added: 1,
            removed: 1,
            chunks: 2,
            ..Default::default()
        });
        assert_eq!(a.added, 2);
        assert_eq!(a.removed, 5);
        assert_eq!(a.chunks, 7);
        assert_eq!(a.failed.len(), 1);
    }
}
