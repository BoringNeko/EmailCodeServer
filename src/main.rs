mod code_extractor;
mod config;
mod mailbox;
mod ws_server;

use config::Config;
use regex::Regex;
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.yaml".to_string());

    let config = load_config(&config_path);
    tracing::info!(path = %config_path, "config loaded");

    if config.accounts.is_empty() {
        tracing::warn!("no email accounts configured");
    }

    // Compile regex patterns (shared across all accounts)
    let patterns: Arc<Vec<Regex>> = Arc::new(
        config
            .extraction
            .patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect(),
    );

    if patterns.is_empty() {
        tracing::warn!("no valid regex patterns configured");
    }

    // Channel: mailbox monitors → WS server
    let (code_tx, code_rx) = mpsc::channel::<config::CodeInfo>(64);

    // Start WebSocket server
    let ws_bind = config.bind.clone();
    let ws_keys = config.api_keys.clone();
    let ws_handle = tokio::spawn(async move {
        if let Err(e) = ws_server::run(ws_bind, ws_keys, code_rx).await {
            tracing::error!(error = %e, "WebSocket server crashed");
        }
    });

    // Start mailbox monitors
    let mut monitor_handles = Vec::new();
    for account in config.accounts {
        let tx = code_tx.clone();
        let p = patterns.clone();
        let llm = config.extraction.llm_fallback.clone();
        monitor_handles.push(tokio::spawn(async move {
            mailbox::run(account, tx, p, llm).await;
        }));
    }

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("shutting down");

    // Abort all monitors
    for h in monitor_handles {
        h.abort();
    }
    ws_handle.abort();

    tracing::info!("shutdown complete");
}

fn load_config(path: &str) -> Config {
    let contents = std::fs::read_to_string(path).unwrap_or_else(|e| {
        tracing::error!(path = %path, error = %e, "failed to read config file");
        std::process::exit(1);
    });

    let mut config: Config = serde_yaml::from_str(&contents).unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to parse config");
        std::process::exit(1);
    });

    // Environment variable overrides
    if let Ok(bind) = std::env::var("EMAILSERVER_BIND") {
        config.bind = bind;
    }
    if let Ok(keys) = std::env::var("EMAILSERVER_API_KEYS") {
        config.api_keys = keys.split(',').map(|s| s.trim().to_string()).collect();
    }

    config
}
