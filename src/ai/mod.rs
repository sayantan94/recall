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

/// Where Claude Code keeps its transcripts.
///
/// `RECALL_CLAUDE_DIR` overrides it, so tests and sandboxed runs can point at a
/// fixture directory instead of the real one.
pub fn projects_dir_claude() -> PathBuf {
    override_dir("RECALL_CLAUDE_DIR")
        .unwrap_or_else(|| home().join(".claude").join("projects"))
}

/// Where Codex keeps its transcripts. `RECALL_CODEX_DIR` overrides it.
pub fn sessions_dir_codex() -> PathBuf {
    override_dir("RECALL_CODEX_DIR").unwrap_or_else(|| home().join(".codex").join("sessions"))
}

fn override_dir(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
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

    #[test]
    fn transcript_directories_can_be_overridden() {
        // Safety: the variable is set and read on this thread only.
        unsafe { std::env::set_var("RECALL_CLAUDE_DIR", "/tmp/recall-fixture") };
        assert_eq!(projects_dir_claude(), PathBuf::from("/tmp/recall-fixture"));
        unsafe { std::env::remove_var("RECALL_CLAUDE_DIR") };
        assert!(projects_dir_claude().ends_with(".claude/projects"));
    }
}
