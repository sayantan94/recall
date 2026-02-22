use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub llm: LlmConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PrivacyConfig {
    #[serde(default = "default_ignore_patterns")]
    pub ignore_patterns: Vec<String>,
    #[serde(default = "default_redact_patterns")]
    pub redact_patterns: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Anthropic,
    Bedrock,
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::Anthropic
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub provider: LlmProvider,
    pub api_key: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    pub aws_region: Option<String>,
}

fn default_ignore_patterns() -> Vec<String> {
    vec![
        "export *KEY*".to_string(),
        "export *SECRET*".to_string(),
        "export *TOKEN*".to_string(),
        "export *PASSWORD*".to_string(),
        "*AWS_SECRET*".to_string(),
    ]
}

fn default_redact_patterns() -> Vec<String> {
    vec![]
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_base_url() -> String {
    "https://api.anthropic.com".to_string()
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            ignore_patterns: default_ignore_patterns(),
            redact_patterns: default_redact_patterns(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: LlmProvider::default(),
            api_key: None,
            model: default_model(),
            base_url: default_base_url(),
            aws_region: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            privacy: PrivacyConfig::default(),
            llm: LlmConfig::default(),
        }
    }
}

pub fn recall_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".recall")
}

pub fn db_path() -> PathBuf {
    recall_dir().join("recall.db")
}

pub fn config_path() -> PathBuf {
    recall_dir().join("config.toml")
}

pub fn pause_file() -> PathBuf {
    recall_dir().join(".paused")
}

pub fn env_file() -> PathBuf {
    recall_dir().join("env")
}

pub fn load_env_file() {
    let path = env_file();
    if !path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return,
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if std::env::var(key).is_err() {
                std::env::set_var(key, value);
            }
        }
    }
}

pub fn load_config() -> Result<Config> {
    load_env_file();
    let path = config_path();
    if !path.exists() {
        return Ok(Config::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config from {}", path.display()))?;
    let config: Config =
        toml::from_str(&content).with_context(|| "Failed to parse config.toml")?;
    Ok(config)
}

pub fn ensure_recall_dir() -> Result<()> {
    let dir = recall_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
    }
    Ok(())
}
