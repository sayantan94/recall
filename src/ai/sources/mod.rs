pub mod claude_code;
pub mod codex;

use anyhow::Result;

use super::models::{AiSession, Message, Source};

/// Everything a full parse of one transcript yields.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub messages: Vec<Message>,
    /// A name the user saved for this session inside the tool.
    pub custom_name: Option<String>,
    /// A title the tool generated for itself.
    pub generated_title: Option<String>,
}

/// A place AI conversations are stored on disk. Listing is cheap (metadata from
/// the first few lines); loading messages parses the whole transcript.
pub trait SessionSource {
    /// Sessions found on disk. A missing directory is not an error — it just
    /// means the tool isn't installed.
    fn list_sessions(&self) -> Result<Vec<AiSession>>;
    fn load_conversation(&self, session: &AiSession) -> Result<Conversation>;
}

pub fn source_for(source: Source) -> Box<dyn SessionSource> {
    match source {
        Source::Claude => Box::new(claude_code::ClaudeCodeSource::new()),
        Source::Codex => Box::new(codex::CodexSource::new()),
    }
}
