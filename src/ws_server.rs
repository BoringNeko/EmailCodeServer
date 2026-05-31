use crate::config::CodeInfo;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

type ClientTx = mpsc::UnboundedSender<String>;
type Clients = Arc<Mutex<HashMap<u64, ClientTx>>>;

pub async fn run(bind: String, api_keys: Vec<String>, mut code_rx: mpsc::Receiver<CodeInfo>) -> Result<()> {
    let listener = TcpListener::bind(&bind).await?;
    tracing::info!(addr = %bind, "WebSocket server listening");

    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let api_keys = Arc::new(api_keys);
    let mut next_id: u64 = 1;

    // Broadcast task: receives CodeInfo from mailbox monitors, sends to all clients
    let broadcast_clients = clients.clone();
    let _broadcast_handle = tokio::spawn(async move {
        while let Some(info) = code_rx.recv().await {
            let json = serde_json::json!({
                "type": "new_code",
                "code": info.code,
                "subject": info.subject,
                "from": info.from,
                "time": info.time,
                "account": info.account,
            })
            .to_string();

            let mut guard = broadcast_clients.lock().await;
            let mut dead_ids = Vec::new();
            for (id, tx) in guard.iter() {
                if tx.send(json.clone()).is_err() {
                    dead_ids.push(*id);
                }
            }
            for id in dead_ids {
                guard.remove(&id);
            }
        }
    });

    // Accept loop
    loop {
        let (stream, addr) = listener.accept().await?;
        let keys = api_keys.clone();
        let clients = clients.clone();
        let id = next_id;
        next_id += 1;

        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, addr, id, keys, clients).await {
                tracing::warn!(client_id = id, error = %e, "connection error");
            }
        });
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    addr: std::net::SocketAddr,
    id: u64,
    api_keys: Arc<Vec<String>>,
    clients: Clients,
) -> Result<()> {
    let ws_stream = accept_async(stream).await?;
    let (mut write, mut read) = ws_stream.split();

    // Wait for auth message (timeout 10s)
    let auth_msg = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read.next(),
    )
    .await;

    let authenticated = match auth_msg {
        Ok(Some(Ok(Message::Text(text)))) => {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                if data["type"].as_str() == Some("auth") {
                    let key = data["api_key"].as_str().unwrap_or("");
                    if api_keys.iter().any(|k| k == key) {
                        let resp = serde_json::json!({"type": "auth_ok"}).to_string();
                        write.send(Message::Text(resp.into())).await?;
                        true
                    } else {
                        let resp = serde_json::json!({
                            "type": "auth_fail",
                            "reason": "invalid api_key"
                        })
                        .to_string();
                        write.send(Message::Text(resp.into())).await?;
                        false
                    }
                } else {
                    let resp = serde_json::json!({
                        "type": "auth_fail",
                        "reason": "first message must be auth"
                    })
                    .to_string();
                    write.send(Message::Text(resp.into())).await?;
                    false
                }
            } else {
                false
            }
        }
        _ => false,
    };

    if !authenticated {
        return Ok(());
    }

    tracing::info!(client_id = id, %addr, "client authenticated");

    // Create channel for this client and register
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    clients.lock().await.insert(id, tx);

    // Read loop: handle ping from client, detect disconnect
    let read_handle = {
        let clients = clients.clone();
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                            if data["type"].as_str() == Some("ping") {
                                // handled in write loop via heartbeat check
                            }
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            // Client disconnected
            clients.lock().await.remove(&id);
            tracing::info!(client_id = id, "client disconnected");
        })
    };

    // Write loop: send queued messages + pong responses
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let pong = serde_json::json!({"type": "pong"}).to_string();
                if write.send(Message::Text(pong.into())).await.is_err() {
                    break;
                }
            }
            msg = rx.recv() => {
                match msg {
                    Some(text) => {
                        if write.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    // Cleanup
    read_handle.abort();
    clients.lock().await.remove(&id);

    Ok(())
}
