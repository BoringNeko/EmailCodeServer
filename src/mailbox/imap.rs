use crate::code_extractor;
use crate::config::{AccountConfig, CodeInfo, LlmFallbackConfig};
use anyhow::Result;
use async_imap::extensions::idle::IdleResponse;
use async_imap::Client;
use async_native_tls::TlsConnector;
use chrono::Utc;
use futures_util::StreamExt;
use regex::Regex;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::compat::TokioAsyncReadCompatExt;

type ImapStream = async_native_tls::TlsStream<tokio_util::compat::Compat<TcpStream>>;

pub async fn watch(
    account: AccountConfig,
    tx: mpsc::Sender<CodeInfo>,
    patterns: Arc<Vec<Regex>>,
    llm: LlmFallbackConfig,
) -> Result<()> {
    let mut last_uid: u32 = 0;
    let mut retry_delay = 1u64;
    let max_delay: u64 = 60;

    loop {
        match run_idle_loop(&account, &tx, &patterns, &llm, &mut last_uid).await {
            Ok(()) => {
                tracing::info!(account = %account.name, "IMAP idle loop ended, reconnecting");
                retry_delay = 1;
            }
            Err(e) => {
                tracing::error!(account = %account.name, error = %e, "IMAP error, reconnecting in {}s", retry_delay);
                tokio::time::sleep(Duration::from_secs(retry_delay)).await;
                retry_delay = (retry_delay * 2).min(max_delay);
            }
        }
    }
}

async fn run_idle_loop(
    account: &AccountConfig,
    tx: &mpsc::Sender<CodeInfo>,
    patterns: &[Regex],
    llm: &LlmFallbackConfig,
    last_uid: &mut u32,
) -> Result<()> {
    let domain = account.host.clone();
    let addr = (domain.as_str(), account.port);

    let tls = TlsConnector::new();
    let tcp = TcpStream::connect(addr).await?;
    let tls_stream = tls.connect(&domain, tcp.compat()).await?;

    let client = Client::new(tls_stream);
    let mut session = client
        .login(&account.email, &account.password)
        .await
        .map_err(|(e, _)| anyhow::anyhow!("IMAP login failed: {e}"))?;

    session.select("INBOX").await?;

    if *last_uid == 0 {
        let uids = session.uid_search("ALL").await?;
        *last_uid = uids.iter().max().copied().unwrap_or(0);
    }

    tracing::info!(account = %account.name, last_uid = *last_uid, "IMAP connected, entering IDLE loop");

    loop {
        let mut idle_handle = session.idle();
        idle_handle.init().await?;

        let (wait_future, _stop) = idle_handle.wait();
        let idle_result = tokio::time::timeout(
            Duration::from_secs(account.idle_timeout_minutes as u64 * 60),
            wait_future,
        )
        .await;

        // Always get session back via done()
        session = match idle_handle.done().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(account = %account.name, error = %e, "failed to DONE IDLE, will reconnect");
                return Ok(());
            }
        };

        match idle_result {
            Ok(Ok(IdleResponse::NewData(_))) => {
                // New data available, check for new messages
                match session.uid_search(format!("{}:*", *last_uid + 1)).await {
                    Ok(new_uids) => {
                        if !new_uids.is_empty() {
                            tracing::info!(account = %account.name, new_uids = ?new_uids, "new messages detected");
                        }
                        for uid in new_uids {
                            if let Err(e) =
                                fetch_and_process(&mut session, uid, tx, patterns, llm, &account.name)
                                    .await
                            {
                                tracing::warn!(account = %account.name, uid, error = %e, "failed to process message");
                            }
                            *last_uid = uid;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(account = %account.name, error = %e, "UID search failed");
                    }
                }
            }
            Ok(Ok(IdleResponse::Timeout)) | Ok(Ok(IdleResponse::ManualInterrupt)) => {
                // Normal timeout/interrupt, just re-enter IDLE
            }
            Ok(Err(e)) => {
                return Err(anyhow::anyhow!("IDLE wait error: {e}"));
            }
            Err(_elapsed) => {
                // Our timeout elapsed — re-enter IDLE (session already recovered via done())
                tracing::debug!(account = %account.name, "IDLE timeout, re-entering");
            }
        }
    }
}

async fn fetch_and_process(
    session: &mut async_imap::Session<ImapStream>,
    uid: u32,
    tx: &mpsc::Sender<CodeInfo>,
    patterns: &[Regex],
    llm: &LlmFallbackConfig,
    account_name: &str,
) -> Result<()> {
    let mut stream = session
        .uid_fetch(uid.to_string(), "(ENVELOPE BODY[])")
        .await?;

    while let Some(fetch_result) = stream.next().await {
        let fetch = fetch_result?;

        let envelope = fetch.envelope();

        let from = envelope
            .and_then(|e| e.from.as_ref())
            .and_then(|addrs| addrs.first())
            .and_then(|addr| addr.mailbox.as_ref())
            .map(|mbox| String::from_utf8_lossy(mbox).to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let subject = envelope
            .and_then(|e| e.subject.as_ref())
            .map(|s| String::from_utf8_lossy(s).to_string())
            .unwrap_or_default();

        // BODY[] returns the full RFC822 message; parse with mailparse
        let raw = fetch.body().unwrap_or(b"");
        let body_text = if let Ok(parsed) = mailparse::parse_mail(raw) {
            get_text_body(&parsed)
        } else {
            String::from_utf8_lossy(raw).to_string()
        };

        tracing::debug!(
            account = %account_name,
            uid,
            subject = %subject,
            body_len = body_text.len(),
            "processing message"
        );

        let code = code_extractor::extract_with_llm(&subject, &body_text, patterns, llm).await;

        if let Some(code) = code {
            let info = CodeInfo {
                code,
                subject,
                from,
                time: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                account: account_name.to_string(),
            };
            tracing::info!(account = %account_name, code = %info.code, "new verification code");
            let _ = tx.send(info).await;
        } else {
            tracing::debug!(
                account = %account_name,
                uid,
                subject = %subject,
                body_preview = %&body_text[..body_text.len().min(100)],
                "no verification code found"
            );
        }
    }

    Ok(())
}

fn get_text_body(parsed: &mailparse::ParsedMail) -> String {
    if parsed.ctype.mimetype == "text/plain" {
        return parsed.get_body().unwrap_or_default();
    }
    for part in &parsed.subparts {
        let body = get_text_body(part);
        if !body.is_empty() {
            return body;
        }
    }
    String::new()
}
