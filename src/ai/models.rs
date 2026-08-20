use serde::{Deserialize, Serialize};

/// An AI coding assistant whose sessions recall can index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Claude, Source::Codex];

    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Source::Claude => "Claude Code",
            Source::Codex => "Codex",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        match s.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Source::Claude),
            "codex" => Some(Source::Codex),
            _ => None,
        }
    }
}

/// One AI assistant conversation, as discovered on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSession {
    /// Source-qualified identity: `<source>:<native session id>`. Native ids are
    /// only unique within their own tool, so every stored reference carries both.
    pub uid: String,
    pub source: Source,
    pub session_id: String,
    /// Absolute directory the session belongs to (drives grouping and resume cwd).
    pub project: String,
    pub title: Option<String>,
    /// Milliseconds since epoch, matching the rest of recall's timestamps.
    pub started_at: i64,
    pub last_activity: i64,
    pub model: Option<String>,
    pub message_count: usize,
    pub file_path: String,
    pub file_mtime: i64,
    pub file_size: i64,
}

pub fn session_uid(source: Source, session_id: &str) -> String {
    format!("{}:{}", source.as_str(), session_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// Milliseconds since epoch, when the source records one.
    pub timestamp: Option<i64>,
    pub tool_names: Vec<String>,
}

/// A searchable slice of a conversation. Chunks, not whole sessions, are what
/// FTS5 indexes, so a long conversation stays findable by any part of it.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub chunk_id: String,
    pub session_uid: String,
    pub source: Source,
    pub project: String,
    pub title: Option<String>,
    pub timestamp: i64,
    pub text: String,
}

/// A session that matched a search, carrying its best-matching excerpt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSearchResult {
    pub session: AiSession,
    pub snippet: String,
    pub rank: f64,
}
