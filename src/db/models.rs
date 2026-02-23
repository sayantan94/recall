use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub start_time: i64,
    pub end_time: Option<i64>,
    pub terminal_app: Option<String>,
    pub initial_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: Option<i64>,
    pub session_id: String,
    pub command_text: String,
    pub timestamp: i64,
    pub duration_ms: Option<i64>,
    pub cwd: Option<String>,
    pub git_repo: Option<String>,
    pub git_branch: Option<String>,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub id: Option<i64>,
    pub session_id: String,
    pub summary_text: String,
    pub tags: Option<String>,
    pub intent: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub command: Command,
    pub rank: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarySearchResult {
    pub summary: Summary,
    pub rank: f64,
}
