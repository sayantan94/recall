use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::ai::models::{session_uid, AiSession, Message, Role, Source};
use crate::ai::sources::{Conversation, SessionSource};
use crate::ai::{parse_rfc3339_millis, projects_dir_claude};

pub struct ClaudeCodeSource {
    projects_dir: PathBuf,
}

impl ClaudeCodeSource {
    pub fn new() -> Self {
        Self {
            projects_dir: projects_dir_claude(),
        }
    }
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    message: Option<RawMessage>,
    /// Set on `custom-title` lines: the name the user saved for the session.
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    /// Set on `ai-title` lines: the title Claude Code generated.
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
}

#[derive(Deserialize)]
struct RawMessage {
    content: Option<serde_json::Value>,
    model: Option<String>,
}

/// Claude Code encodes a project path by replacing every `/` with `-`, which is
/// lossy for directories that contain a dash. Resolve it by walking the real
/// filesystem, preferring the longest segment join that exists on disk.
fn decode_project_path(encoded: &str) -> String {
    if !encoded.starts_with('-') {
        return encoded.to_string();
    }

    let segments: Vec<&str> = encoded[1..].split('-').collect();
    let mut path = String::from("/");
    let mut i = 0;

    while i < segments.len() {
        let candidate = format!("{}{}", path, segments[i]);
        if Path::new(&candidate).exists() {
            path = format!("{}/", candidate);
            i += 1;
            continue;
        }

        let mut found = false;
        for j in (i + 1..segments.len()).rev() {
            let joined = segments[i..=j].join("-");
            let candidate = format!("{}{}", path, joined);
            if Path::new(&candidate).exists() {
                path = format!("{}/", candidate);
                i = j + 1;
                found = true;
                break;
            }
        }

        if !found {
            path = format!("{}{}", path, segments[i..].join("-"));
            break;
        }
    }

    path.trim_end_matches('/').to_string()
}

fn extract_text_from_content(content: &serde_json::Value) -> (String, Vec<String>) {
    let mut texts = Vec::new();
    let mut tool_names = Vec::new();

    match content {
        serde_json::Value::String(s) => texts.push(s.clone()),
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                let obj = match block.as_object() {
                    Some(o) => o,
                    None => continue,
                };
                match obj.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
                            texts.push(text.to_string());
                        }
                    }
                    Some("tool_use") => {
                        if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                            tool_names.push(name.to_string());
                        }
                    }
                    // thinking / tool_result blocks are noise for search
                    _ => {}
                }
            }
        }
        _ => {}
    }

    (clean_message_text(&texts.join("\n")), tool_names)
}

/// Strip the internal XML-ish tags Claude Code injects into transcripts, so the
/// index holds what the human and the assistant actually said.
fn clean_message_text(text: &str) -> String {
    const INTERNAL_TAGS: [&str; 8] = [
        "local-command-caveat",
        "local-command-stdout",
        "command-name",
        "command-message",
        "command-args",
        "system-reminder",
        "user-prompt-submit-hook",
        "antml:thinking",
    ];

    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    while let Some(start) = remaining.find('<') {
        result.push_str(&remaining[..start]);

        let end = match remaining[start..].find('>') {
            Some(e) => e,
            None => {
                result.push_str(&remaining[start..]);
                return finish_cleanup(result);
            }
        };

        let tag_name = remaining[start + 1..start + end]
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_start_matches('/');

        if INTERNAL_TAGS.contains(&tag_name) {
            let close_tag = format!("</{}>", tag_name);
            remaining = match remaining.find(&close_tag) {
                Some(pos) => &remaining[pos + close_tag.len()..],
                None => &remaining[start + end + 1..],
            };
        } else {
            result.push_str(&remaining[start..start + end + 1]);
            remaining = &remaining[start + end + 1..];
        }
    }

    result.push_str(remaining);
    finish_cleanup(result)
}

fn finish_cleanup(text: String) -> String {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("[Request interrupted by user")
                && !trimmed.starts_with("[Response interrupted by")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// recall drives `claude -p` for its own natural-language answers, and Claude
/// Code records those runs as ordinary sessions. Indexing them would fill the
/// index with recall's own prompts, so they are skipped.
pub fn is_recall_own_prompt(text: &str) -> bool {
    const PREFIXES: [&str; 2] = [
        "You are a terminal history assistant.",
        "You are a terminal activity summarizer.",
    ];
    PREFIXES.iter().any(|prefix| text.trim_start().starts_with(prefix))
}

/// Transcripts live at `~/.claude/projects/<encoded-path>/<session-id>.jsonl`.
/// Subagent transcripts are skipped: they duplicate the parent conversation.
fn find_session_files(projects_dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(projects_dir)
        .min_depth(2)
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_path_buf())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "jsonl")
                && !p.to_str().is_some_and(|s| s.contains("subagents"))
        })
        .collect()
}

impl SessionSource for ClaudeCodeSource {
    fn list_sessions(&self) -> Result<Vec<AiSession>> {
        if !self.projects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();

        for file_path in find_session_files(&self.projects_dir) {
            let encoded_project = file_path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let project = decode_project_path(encoded_project);

            let session_id = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            let metadata = fs::metadata(&file_path)?;
            let file_mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            let file = fs::File::open(&file_path)?;
            let mut started_at = None;
            let mut model = None;
            let mut cwd = None;

            for line in BufReader::new(file).lines().take(10) {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                let entry: RawEntry = match serde_json::from_str(&line) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if entry.entry_type.as_deref() != Some("user")
                    && entry.entry_type.as_deref() != Some("assistant")
                {
                    continue;
                }
                if started_at.is_none() {
                    started_at = entry.timestamp.as_deref().and_then(parse_rfc3339_millis);
                }
                if cwd.is_none() {
                    cwd = entry.cwd.clone();
                }
                if model.is_none() {
                    model = entry.message.as_ref().and_then(|m| m.model.clone());
                }
            }

            let started_at = started_at.unwrap_or(file_mtime);

            sessions.push(AiSession {
                uid: session_uid(Source::Claude, &session_id),
                source: Source::Claude,
                session_id,
                project: cwd.unwrap_or(project),
                // Titles come from the full parse: the cheap metadata pass
                // cannot see rename lines, which are appended anywhere.
                title: None,
                custom_name: None,
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

    fn load_conversation(&self, session: &AiSession) -> Result<Conversation> {
        let file = fs::File::open(&session.file_path)
            .with_context(|| format!("Failed to open {}", session.file_path))?;
        let mut conversation = Conversation::default();

        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.is_empty() {
                continue;
            }

            let entry: RawEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Renames are appended, so the last one recorded wins.
            match entry.entry_type.as_deref() {
                Some("custom-title") => {
                    conversation.custom_name =
                        entry.custom_title.filter(|name| !name.trim().is_empty());
                    continue;
                }
                Some("ai-title") => {
                    conversation.generated_title =
                        entry.ai_title.filter(|title| !title.trim().is_empty());
                    continue;
                }
                _ => {}
            }

            let role = match entry.entry_type.as_deref() {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => continue,
            };

            let content = match entry.message.as_ref().and_then(|m| m.content.as_ref()) {
                Some(c) => c,
                None => continue,
            };

            let (text, tool_names) = extract_text_from_content(content);
            if text.trim().is_empty() {
                continue;
            }

            conversation.messages.push(Message {
                role,
                text,
                timestamp: entry.timestamp.as_deref().and_then(parse_rfc3339_millis),
                tool_names,
            });
        }

        Ok(conversation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_project_path_falls_back_when_nothing_exists() {
        assert_eq!(
            decode_project_path("-nope-does-not-exist"),
            "/nope-does-not-exist"
        );
    }

    #[test]
    fn decode_project_path_resolves_real_dirs() {
        // /usr/local exists on macOS and most Linux boxes.
        assert_eq!(decode_project_path("-usr-local"), "/usr/local");
    }

    #[test]
    fn decode_project_path_passes_through_unencoded() {
        assert_eq!(decode_project_path("plain"), "plain");
    }

    #[test]
    fn clean_message_text_strips_internal_tags() {
        let raw = "before<system-reminder>hidden</system-reminder>after";
        assert_eq!(clean_message_text(raw), "beforeafter");
    }

    #[test]
    fn clean_message_text_keeps_unknown_tags() {
        let raw = "use <Foo> in code";
        assert_eq!(clean_message_text(raw), "use <Foo> in code");
    }

    #[test]
    fn clean_message_text_drops_interruption_markers() {
        let raw = "kept\n[Request interrupted by user for tool use]\nalso kept";
        assert_eq!(clean_message_text(raw), "kept\nalso kept");
    }

    #[test]
    fn extract_text_collects_text_and_tools() {
        let content = serde_json::json!([
            {"type": "text", "text": "Reading it now."},
            {"type": "tool_use", "name": "Read"},
            {"type": "thinking", "thinking": "ignored"},
        ]);
        let (text, tools) = extract_text_from_content(&content);
        assert_eq!(text, "Reading it now.");
        assert_eq!(tools, vec!["Read"]);
    }

    #[test]
    fn extract_text_handles_plain_string_content() {
        let content = serde_json::json!("just a string");
        let (text, tools) = extract_text_from_content(&content);
        assert_eq!(text, "just a string");
        assert!(tools.is_empty());
    }
}
