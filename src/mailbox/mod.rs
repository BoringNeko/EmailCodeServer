pub mod imap;
pub mod pop3;

use crate::config::{AccountConfig, CodeInfo};
use regex::Regex;
use std::sync::Arc;
use tokio::sync::mpsc;

pub async fn run(
    account: AccountConfig,
    tx: mpsc::Sender<CodeInfo>,
    patterns: Arc<Vec<Regex>>,
    llm: crate::config::LlmFallbackConfig,
) {
    let name = account.name.clone();
    tracing::info!(account = %name, protocol = ?account.protocol, "starting mailbox monitor");

    let result = match account.protocol {
        crate::config::Protocol::Imap => imap::watch(account, tx, patterns, llm).await,
        crate::config::Protocol::Pop3 => pop3::watch(account, tx, patterns, llm).await,
    };

    match result {
        Ok(()) => tracing::info!(account = %name, "monitor exited cleanly"),
        Err(e) => tracing::error!(account = %name, error = %e, "monitor exited with error"),
    }
}
