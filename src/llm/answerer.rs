use anyhow::Result;

use crate::config::settings::LlmConfig;
use crate::db::models::Command;
use crate::llm::client::call_claude;

/// Given a user question and candidate commands from search, ask the LLM to synthesize an answer.
pub async fn answer_question(
    config: &LlmConfig,
    question: &str,
    commands: &[Command],
) -> Result<String> {
    if commands.is_empty() {
        return Ok("No matching commands found in your history.".to_string());
    }

    let mut context = String::from(
        "You are a terminal history assistant. The user is asking about their command-line activity. \
         Below is a list of relevant commands from their history. Answer their question based on this data. \
         Be concise and specific. Format timestamps as human-readable.\n\n\
         Command history:\n",
    );

    for cmd in commands {
        let ts = chrono::DateTime::from_timestamp_millis(cmd.timestamp)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        context.push_str(&format!(
            "- [{}] `{}` (dir: {}, repo: {}, branch: {}, exit: {})\n",
            ts,
            cmd.command_text,
            cmd.cwd.as_deref().unwrap_or("?"),
            cmd.git_repo.as_deref().unwrap_or("-"),
            cmd.git_branch.as_deref().unwrap_or("-"),
            cmd.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string()),
        ));
    }

    call_claude(config, &context, question).await
}
