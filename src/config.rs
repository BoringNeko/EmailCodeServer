use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub extraction: ExtractionConfig,
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    #[serde(default = "default_patterns")]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub llm_fallback: LlmFallbackConfig,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            patterns: default_patterns(),
            llm_fallback: LlmFallbackConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFallbackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
}

impl Default for LlmFallbackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_llm_provider(),
            api_key: String::new(),
            model: default_llm_model(),
            base_url: default_llm_base_url(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    #[serde(default = "default_imap_port")]
    pub port: u16,
    #[serde(default = "default_true")]
    pub tls: bool,
    pub email: String,
    pub password: String,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_minutes: u32,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Imap,
    Pop3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeInfo {
    pub code: String,
    pub subject: String,
    pub from: String,
    pub time: String,
    #[serde(default)]
    pub account: String,
}

fn default_bind() -> String {
    "0.0.0.0:8080".into()
}

fn default_patterns() -> Vec<String> {
    vec![
        r"\b\d{6}\b".into(),
        r"\b\d{4}-\d{4}\b".into(),
        r"\b\d{8}\b".into(),
    ]
}

fn default_llm_provider() -> String {
    "openai".into()
}

fn default_llm_model() -> String {
    "gpt-4.1-mini".into()
}

fn default_llm_base_url() -> String {
    "https://api.openai.com/v1".into()
}

fn default_imap_port() -> u16 {
    993
}

fn default_true() -> bool {
    true
}

fn default_idle_timeout() -> u32 {
    29
}

fn default_poll_interval() -> u64 {
    60
}
