//! Search facade over the indexed AI sessions.

use anyhow::Result;
use rusqlite::Connection;

use super::models::AiSearchResult;
use super::store::{self, Filter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// FTS5 full-text matching, ranked by BM25.
    Fts,
    /// Case-insensitive substring matching, for fragments FTS5 can't tokenize.
    Fuzzy,
}

impl Mode {
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Fts => "full-text",
            Mode::Fuzzy => "fuzzy",
        }
    }
}

/// Search indexed sessions. Full-text search falls back to fuzzy matching when
/// it finds nothing, so a half-remembered fragment still lands somewhere.
pub fn search(
    conn: &Connection,
    query: &str,
    filter: &Filter,
    mode: Mode,
) -> Result<(Vec<AiSearchResult>, Mode)> {
    match mode {
        Mode::Fuzzy => Ok((store::search_fuzzy(conn, query, filter)?, Mode::Fuzzy)),
        Mode::Fts => {
            let results = store::search(conn, query, filter)?;
            if results.is_empty() {
                let fuzzy = store::search_fuzzy(conn, query, filter)?;
                if !fuzzy.is_empty() {
                    return Ok((fuzzy, Mode::Fuzzy));
                }
            }
            Ok((results, Mode::Fts))
        }
    }
}
