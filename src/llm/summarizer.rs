use anyhow::Result;

use crate::config::settings::LlmConfig;
use crate::db::models::Command;
use crate::llm::client::call_claude;

/// Summarize a session's commands into a summary, tags, and intent.
/// Returns (summary_text, tags_json, intent).
pub async fn summarize_session(
    config: &LlmConfig,
    commands: &[Command],
) -> Result<(String, String, String)> {
    if commands.is_empty() {
        return Ok((
            "Empty session".to_string(),
            "[]".to_string(),
            "unknown".to_string(),
        ));
    }

    let mut context = String::from(
        "You are a terminal activity summarizer. Given the following sequence of shell commands from a single session, \
         provide:\n\
         1. A concise summary (1-2 sentences) of what the user was doing\n\
         2. A JSON array of relevant tags (e.g. [\"git\", \"rust\", \"debugging\"])\n\
         3. A one-word intent (e.g. \"development\", \"deployment\", \"debugging\", \"configuration\")\n\n\
         Respond in exactly this format:\n\
         SUMMARY: <your summary>\n\
         TAGS: <json array>\n\
         INTENT: <one word>\n\n\
         Commands:\n",
    );

    for cmd in commands {
        let ts = chrono::DateTime::from_timestamp_millis(cmd.timestamp)
            .map(|dt| dt.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "?".to_string());

        context.push_str(&format!(
            "  [{}] {} (exit: {})\n",
            ts,
            cmd.command_text,
            cmd.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string()),
        ));

        if let Some(ref output) = cmd.output {
            let truncated: String = output.lines().take(10).collect::<Vec<_>>().join("\n");
            context.push_str(&format!("    output: {}\n", truncated));
        }
    }

    let response = call_claude(config, &context, "Summarize this session.").await?;

    // Parse the response
    let mut summary = String::new();
    let mut tags = "[]".to_string();
    let mut intent = "unknown".to_string();

    for line in response.lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix("SUMMARY:") {
            summary = s.trim().to_string();
        } else if let Some(t) = line.strip_prefix("TAGS:") {
            tags = t.trim().to_string();
        } else if let Some(i) = line.strip_prefix("INTENT:") {
            intent = i.trim().to_string();
        }
    }

    if summary.is_empty() {
        summary = response.lines().next().unwrap_or("Session activity").to_string();
    }

    Ok((summary, tags, intent))
}
