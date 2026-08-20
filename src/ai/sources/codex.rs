use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::ai::models::{session_uid, AiSession, Message, Role, Source};
use crate::ai::sources::SessionSource;
use crate::ai::{parse_rfc3339_millis, sessions_dir_codex};

pub struct CodexSource {
    sessions_dir: PathBuf,
}

impl CodexSource {
    pub fn new() -> Self {
        Self {
            sessions_dir: sessions_dir_codex(),
        }
    }
}

#[derive(Deserialize)]
struct CodexEvent {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    event_type: String,
    payload: serde_json::Value,
}

#[derive(Deserialize)]
struct SessionMeta {
    id: Option<String>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct ResponseItem {
    role: Option<String>,
    content: Option<Vec<ContentBlock>>,
}

#[derive(Deserialize)]
struct TurnContext {
    model: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: Option<String>,
    text: Option<String>,
    name: Option<String>,
}

/// Codex writes one JSONL per session under `~/.codex/sessions/YYYY/MM/DD/`.
fn find_session_files(sessions_dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(sessions_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect()
}

/// Codex replays project instructions and sandbox rules as ordinary turns.
/// They are identical across every session in a repo, so indexing them buries
/// the real conversation and makes every title read "# AGENTS.md instructions".
fn is_injected_context(text: &str) -> bool {
    const PREFIXES: [&str; 7] = [
        "<environment_context>",
        "<plugins_instructions>",
        "<recommended_plugins>",
        "<user_instructions>",
        "<permissions instructions>",
        "<turn_aborted>",
        "# AGENTS.md instructions for ",
    ];
    PREFIXES.iter().any(|prefix| text.starts_with(prefix))
}

fn extract_content(blocks: &[ContentBlock]) -> (String, Vec<String>) {
    let mut texts = Vec::new();
    let mut tool_names = Vec::new();

    for block in blocks {
        match block.block_type.as_deref() {
            Some("input_text") | Some("output_text") => {
                if let Some(text) = &block.text {
                    if !is_injected_context(text) {
                        texts.push(text.as_str());
                    }
                }
            }
            Some("function_call") => {
                if let Some(name) = &block.name {
                    tool_names.push(name.clone());
                }
            }
            _ => {}
        }
    }

    (texts.join("\n"), tool_names)
}

impl SessionSource for CodexSource {
    fn list_sessions(&self) -> Result<Vec<AiSession>> {
        if !self.sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();

        for file_path in find_session_files(&self.sessions_dir) {
            let file = fs::File::open(&file_path)?;

            let mut session_id = None;
            let mut cwd = None;
            let mut started_at = None;
            let mut model = None;
            let mut title = None;

            for line in BufReader::new(file).lines().take(30) {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                let event: CodexEvent = match serde_json::from_str(&line) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                match event.event_type.as_str() {
                    "session_meta" => {
                        if let Ok(meta) = serde_json::from_value::<SessionMeta>(event.payload) {
                            session_id = meta.id;
                            cwd = meta.cwd;
                        }
                        if started_at.is_none() {
                            started_at =
                                event.timestamp.as_deref().and_then(parse_rfc3339_millis);
                        }
                    }
                    "turn_context" => {
                        if model.is_none() {
                            if let Ok(ctx) = serde_json::from_value::<TurnContext>(event.payload) {
                                model = ctx.model;
                            }
                        }
                    }
                    "response_item" => {
                        if title.is_some() {
                            continue;
                        }
                        let item: ResponseItem = match serde_json::from_value(event.payload) {
                            Ok(i) => i,
                            Err(_) => continue,
                        };
                        if item.role.as_deref() != Some("user") {
                            continue;
                        }
                        if let Some(blocks) = &item.content {
                            let (text, _) = extract_content(blocks);
                            let text = text.trim();
                            if !text.is_empty() {
                                title = Some(text.chars().take(120).collect::<String>());
                            }
                        }
                    }
                    _ => {}
                }
            }

            let session_id = session_id.unwrap_or_else(|| {
                file_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            });

            let metadata = fs::metadata(&file_path)?;
            let file_mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            let started_at = started_at.unwrap_or(file_mtime);

            sessions.push(AiSession {
                uid: session_uid(Source::Codex, &session_id),
                source: Source::Codex,
                session_id,
                project: cwd.unwrap_or_else(|| "unknown".to_string()),
                title,
                started_at,
                last_activity: file_mtime.max(started_at),
                model,
                message_count: 0,
                file_path: file_path.to_string_lossy().to_string(),
                file_mtime,
                file_size: metadata.len() as i64,
            });
        }

        Ok(sessions)
    }

    fn load_messages(&self, session: &AiSession) -> Result<Vec<Message>> {
        let file = fs::File::open(&session.file_path)
            .with_context(|| format!("Failed to open {}", session.file_path))?;
        let mut messages = Vec::new();

        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }

            let event: CodexEvent = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if event.event_type != "response_item" {
                continue;
            }

            let timestamp = event.timestamp.as_deref().and_then(parse_rfc3339_millis);

            let item: ResponseItem = match serde_json::from_value(event.payload) {
                Ok(i) => i,
                Err(_) => continue,
            };

            let role = match item.role.as_deref() {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                // `developer` carries system instructions, not conversation
                _ => continue,
            };

            let blocks = match item.content {
                Some(b) => b,
                None => continue,
            };

            let (text, tool_names) = extract_content(&blocks);
            if text.trim().is_empty() {
                continue;
            }

            messages.push(Message {
                role,
                text,
                timestamp,
                tool_names,
            });
        }

        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_meta() {
        let json = r#"{"timestamp":"2026-03-29T21:05:52.143Z","type":"session_meta","payload":{"id":"019d3b6a","cwd":"/Users/m/repos/test"}}"#;
        let event: CodexEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, "session_meta");
        let meta: SessionMeta = serde_json::from_value(event.payload).unwrap();
        assert_eq!(meta.id.as_deref(), Some("019d3b6a"));
        assert_eq!(meta.cwd.as_deref(), Some("/Users/m/repos/test"));
    }

    #[test]
    fn extracts_user_and_assistant_text() {
        let user = r#"{"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"Hello!"}]}}"#;
        let event: CodexEvent = serde_json::from_str(user).unwrap();
        let item: ResponseItem = serde_json::from_value(event.payload).unwrap();
        let (text, tools) = extract_content(&item.content.unwrap());
        assert_eq!(text, "Hello!");
        assert!(tools.is_empty());
    }

    #[test]
    fn skips_environment_context() {
        let blocks = vec![ContentBlock {
            block_type: Some("input_text".into()),
            text: Some("<environment_context>\n<cwd>/foo</cwd>\n</environment_context>".into()),
            name: None,
        }];
        let (text, _) = extract_content(&blocks);
        assert!(text.is_empty());
    }

    #[test]
    fn skips_replayed_agents_md_instructions() {
        let blocks = vec![ContentBlock {
            block_type: Some("input_text".into()),
            text: Some("# AGENTS.md instructions for /repos/thing\n\n<INSTRUCTIONS>".into()),
            name: None,
        }];
        let (text, _) = extract_content(&blocks);
        assert!(text.is_empty());
    }

    #[test]
    fn skips_the_replayed_plugin_catalogue() {
        let blocks = vec![ContentBlock {
            block_type: Some("input_text".into()),
            text: Some("<recommended_plugins> Here is a list of plugins".into()),
            name: None,
        }];
        let (text, _) = extract_content(&blocks);
        assert!(text.is_empty());
    }

    #[test]
    fn keeps_real_prompts_that_merely_mention_agents_md() {
        let blocks = vec![ContentBlock {
            block_type: Some("input_text".into()),
            text: Some("update AGENTS.md with the new lint rule".into()),
            name: None,
        }];
        let (text, _) = extract_content(&blocks);
        assert_eq!(text, "update AGENTS.md with the new lint rule");
    }

    #[test]
    fn collects_function_call_names() {
        let blocks = vec![
            ContentBlock {
                block_type: Some("output_text".into()),
                text: Some("Let me check.".into()),
                name: None,
            },
            ContentBlock {
                block_type: Some("function_call".into()),
                text: None,
                name: Some("shell".into()),
            },
        ];
        let (text, tools) = extract_content(&blocks);
        assert_eq!(text, "Let me check.");
        assert_eq!(tools, vec!["shell"]);
    }

    #[test]
    fn reads_model_from_turn_context() {
        let json = r#"{"type":"turn_context","payload":{"model":"gpt-5.4","cwd":"/tmp"}}"#;
        let event: CodexEvent = serde_json::from_str(json).unwrap();
        let ctx: TurnContext = serde_json::from_value(event.payload).unwrap();
        assert_eq!(ctx.model.as_deref(), Some("gpt-5.4"));
    }
}
