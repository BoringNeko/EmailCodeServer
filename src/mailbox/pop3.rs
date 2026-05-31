use crate::code_extractor;
use crate::config::{AccountConfig, CodeInfo, LlmFallbackConfig};
use anyhow::{anyhow, Result};
use async_native_tls::TlsConnector;
use chrono::Utc;
use regex::Regex;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

pub async fn watch(
    account: AccountConfig,
    tx: mpsc::Sender<CodeInfo>,
    patterns: Arc<Vec<Regex>>,
    llm: LlmFallbackConfig,
) -> Result<()> {
    let interval = Duration::from_secs(account.poll_interval_secs);
    let mut seen_uids = HashSet::new();

    loop {
        if let Err(e) = poll_once(&account, &tx, &patterns, &llm, &mut seen_uids).await {
            tracing::error!(account = %account.name, error = %e, "POP3 poll error");
        }
        tokio::time::sleep(interval).await;
    }
}

async fn poll_once(
    account: &AccountConfig,
    tx: &mpsc::Sender<CodeInfo>,
    patterns: &[Regex],
    llm: &LlmFallbackConfig,
    seen_uids: &mut HashSet<String>,
) -> Result<()> {
    let domain = account.host.clone();
    let addr = (domain.as_str(), account.port);

    let tcp = TcpStream::connect(addr).await?;

    let tls = TlsConnector::new();
    let tls_stream = tls.connect(&domain, tcp.compat()).await?;

    // Convert back to tokio traits for read/write
    let compat = tls_stream.compat();
    let (reader, mut writer) = tokio::io::split(compat);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Read server greeting
    read_line(&mut reader, &mut line).await?;
    if !line.starts_with("+OK") {
        return Err(anyhow!("POP3 server greeting error: {}", line.trim()));
    }

    // USER
    write_cmd(&mut writer, &format!("USER {}\r\n", account.email)).await?;
    read_line(&mut reader, &mut line).await?;
    if !line.starts_with("+OK") {
        return Err(anyhow!("POP3 USER error: {}", line.trim()));
    }

    // PASS
    write_cmd(&mut writer, &format!("PASS {}\r\n", account.password)).await?;
    read_line(&mut reader, &mut line).await?;
    if !line.starts_with("+OK") {
        return Err(anyhow!("POP3 PASS error: {}", line.trim()));
    }

    // UIDL
    write_cmd(&mut writer, "UIDL\r\n").await?;
    read_line(&mut reader, &mut line).await?;
    if !line.starts_with("+OK") {
        return Err(anyhow!("POP3 UIDL error: {}", line.trim()));
    }

    let mut new_msg_nums = Vec::new();
    loop {
        read_line(&mut reader, &mut line).await?;
        if line.starts_with('.') {
            if line == ".\r\n" {
                break;
            }
            line.remove(0);
        }
        let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
        if parts.len() == 2 {
            let msg_num: u32 = parts[0].parse().unwrap_or(0);
            let uidl = parts[1].to_string();
            if seen_uids.insert(uidl) {
                new_msg_nums.push(msg_num);
            }
        }
    }

    // RETR new messages
    for msg_num in new_msg_nums {
        match retr_message(&mut reader, &mut writer, msg_num).await {
            Ok(raw_email) => {
                if let Some(info) = parse_and_extract(&raw_email, patterns, llm, &account.name).await {
                    let _ = tx.send(info).await;
                }
            }
            Err(e) => {
                tracing::warn!(account = %account.name, msg_num, error = %e, "failed to RETR message");
            }
        }
    }

    // QUIT
    write_cmd(&mut writer, "QUIT\r\n").await?;
    Ok(())
}

async fn retr_message(
    reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    msg_num: u32,
) -> Result<String> {
    write_cmd(writer, &format!("RETR {}\r\n", msg_num)).await?;
    let mut line = String::new();
    read_line(reader, &mut line).await?;
    if !line.starts_with("+OK") {
        return Err(anyhow!("POP3 RETR error: {}", line.trim()));
    }

    let mut raw = String::new();
    loop {
        read_line(reader, &mut line).await?;
        if line == ".\r\n" {
            break;
        }
        if line.starts_with('.') {
            line.remove(0);
        }
        raw.push_str(&line);
    }
    Ok(raw)
}

async fn parse_and_extract(
    raw_email: &str,
    patterns: &[Regex],
    llm: &LlmFallbackConfig,
    account_name: &str,
) -> Option<CodeInfo> {
    let parsed = mailparse::parse_mail(raw_email.as_bytes()).ok()?;

    let subject = parsed
        .headers
        .iter()
        .find(|h| h.get_key().eq_ignore_ascii_case("Subject"))
        .map(|h| h.get_value())
        .unwrap_or_default();

    let from = parsed
        .headers
        .iter()
        .find(|h| h.get_key().eq_ignore_ascii_case("From"))
        .map(|h| h.get_value())
        .unwrap_or_default();

    let body = get_text_body(&parsed);

    let code = code_extractor::extract_with_llm(&subject, &body, patterns, llm).await?;

    let info = CodeInfo {
        code,
        subject,
        from,
        time: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        account: account_name.to_string(),
    };
    tracing::info!(account = %account_name, code = %info.code, "new verification code (POP3)");
    Some(info)
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

async fn read_line(
    reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
    buf: &mut String,
) -> Result<()> {
    buf.clear();
    reader.read_line(buf).await?;
    Ok(())
}

async fn write_cmd(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    cmd: &str,
) -> Result<()> {
    writer.write_all(cmd.as_bytes()).await?;
    Ok(())
}
