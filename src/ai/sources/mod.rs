pub mod claude_code;
pub mod codex;

use anyhow::Result;

use super::models::{AiSession, Message, Source};

/// A place AI conversations are stored on disk. Listing is cheap (metadata from
/// the first few lines); loading messages parses the whole transcript.
pub trait SessionSource {
    /// Sessions found on disk. A missing directory is not an error — it just
    /// means the tool isn't installed.
    fn list_sessions(&self) -> Result<Vec<AiSession>>;
    fn load_messages(&self, session: &AiSession) -> Result<Vec<Message>>;
}

pub fn source_for(source: Source) -> Box<dyn SessionSource> {
    match source {
        Source::Claude => Box::new(claude_code::ClaudeCodeSource::new()),
        Source::Codex => Box::new(codex::CodexSource::new()),
    }
}
