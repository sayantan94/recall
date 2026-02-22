use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::settings::{LlmConfig, LlmProvider};

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    text: Option<String>,
}

pub async fn call_claude(
    config: &LlmConfig,
    system_context: &str,
    user_prompt: &str,
) -> Result<String> {
    match config.provider {
        LlmProvider::Anthropic => call_anthropic(config, system_context, user_prompt).await,
        LlmProvider::Bedrock => call_bedrock(config, system_context, user_prompt).await,
    }
}

async fn call_anthropic(
    config: &LlmConfig,
    system_context: &str,
    user_prompt: &str,
) -> Result<String> {
    let api_key = config
        .api_key
        .clone()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .context("No Anthropic API key found. Either:\n  - Set ANTHROPIC_API_KEY in ~/.recall/env\n  - Set llm.api_key in ~/.recall/config.toml\n  - Or switch to Bedrock: set llm.provider = \"bedrock\" in config.toml")?;

    let client = reqwest::Client::new();
    let url = format!("{}/v1/messages", config.base_url);

    let request = AnthropicRequest {
        model: config.model.clone(),
        max_tokens: 1024,
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: format!("{}\n\n{}", system_context, user_prompt),
        }],
    };

    let response = client
        .post(&url)
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Failed to call Anthropic API")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Anthropic API error ({}): {}", status, body);
    }

    let msg: AnthropicResponse = response
        .json()
        .await
        .context("Failed to parse Anthropic API response")?;

    let text = msg
        .content
        .into_iter()
        .filter_map(|b| b.text)
        .collect::<Vec<_>>()
        .join("");

    Ok(text)
}

async fn call_bedrock(
    config: &LlmConfig,
    system_context: &str,
    user_prompt: &str,
) -> Result<String> {
    use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message};

    let region = config
        .aws_region
        .as_deref()
        .unwrap_or("us-east-1");

    let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .load()
        .await;

    let client = aws_sdk_bedrockruntime::Client::new(&sdk_config);

    let user_content = format!("{}\n\n{}", system_context, user_prompt);

    let message = Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(user_content))
        .build()
        .context("Failed to build Bedrock message")?;

    let response = client
        .converse()
        .model_id(&config.model)
        .messages(message)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Bedrock API error: {}", e))?;

    let output = response
        .output()
        .ok_or_else(|| anyhow::anyhow!("No output in Bedrock response"))?;

    let reply = output
        .as_message()
        .map_err(|_| anyhow::anyhow!("Bedrock output is not a message"))?;

    let text = reply
        .content()
        .iter()
        .filter_map(|block| block.as_text().ok().map(|s| s.as_str()))
        .collect::<Vec<_>>()
        .join("");

    if text.is_empty() {
        bail!("Empty response from Bedrock");
    }

    Ok(text)
}
