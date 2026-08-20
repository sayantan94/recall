//! Indexing and search for AI coding assistant sessions (Claude Code, Codex).
//!
//! Transcripts stay where their tool wrote them; recall reads them, splits each
//! conversation into chunks, and indexes those chunks in the same SQLite +
//! FTS5 database that holds shell history — so one tool answers both "what did
//! I run" and "what did I talk about".

pub mod chunker;
pub mod commands;
pub mod indexer;
pub mod models;
pub mod resume;
pub mod search;
pub mod sources;
pub mod store;

use chrono::DateTime;
use std::path::PathBuf;

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn projects_dir_claude() -> PathBuf {
    home().join(".claude").join("projects")
}

pub fn sessions_dir_codex() -> PathBuf {
    home().join(".codex").join("sessions")
}

/// Parse an RFC 3339 timestamp into milliseconds since epoch, matching how the
/// rest of recall stores time.
pub fn parse_rfc3339_millis(ts: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_with_fractional_seconds() {
        assert_eq!(
            parse_rfc3339_millis("1970-01-01T00:00:01.500Z"),
            Some(1500)
        );
    }

    #[test]
    fn rejects_garbage_timestamps() {
        assert_eq!(parse_rfc3339_millis("not a time"), None);
    }
}
