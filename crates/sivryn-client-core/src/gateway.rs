use std::time::Duration;

use futures::{SinkExt, StreamExt};
use sivryn_protocol::DeviceId;
use sivryn_protocol::gateway::{ClientMessage, ServerMessage};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{ApiClient, ClientError};

/// A live gateway connection. Server messages arrive on `events`; dropping
/// the receiver closes the connection.
pub struct GatewayConnection {
    pub events: mpsc::Receiver<ServerMessage>,
}

/// Connects, identifies and keeps the connection alive with heartbeats in a
/// background task. Reconnecting after a drop is up to the caller.
pub async fn connect_gateway(api: &ApiClient, device_id: Option<DeviceId>) -> Result<GatewayConnection, ClientError> {
    let token = api.token().ok_or(ClientError::NotLoggedIn)?.to_owned();
    let (socket, _) = tokio_tungstenite::connect_async(api.gateway_url())
        .await
        .map_err(|e| ClientError::InvalidUrl(format!("gateway: {e}")))?;
    let (mut sink, mut stream) = socket.split();
    let (tx, events) = mpsc::channel(256);

    tokio::spawn(async move {
        let send = |msg: &ClientMessage| {
            WsMessage::Text(serde_json::to_string(msg).expect("client messages serialize").into())
        };

        let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
        let mut seq = 0u64;
        if sink.send(send(&ClientMessage::Identify { token, device_id })).await.is_err() {
            return;
        }

        loop {
            tokio::select! {
                frame = stream.next() => {
                    let Some(Ok(frame)) = frame else { break };
                    let WsMessage::Text(text) = frame else { continue };
                    let msg = match serde_json::from_str::<ServerMessage>(text.as_str()) {
                        Ok(msg) => msg,
                        Err(e) => {
                            tracing::warn!(error = %e, "ignoring unknown gateway message");
                            continue;
                        }
                    };
                    if let ServerMessage::Hello { heartbeat_interval_ms, .. } = &msg {
                        heartbeat = tokio::time::interval(Duration::from_millis(*heartbeat_interval_ms));
                    }
                    if tx.send(msg).await.is_err() {
                        break;
                    }
                }
                _ = heartbeat.tick() => {
                    seq += 1;
                    if sink.send(send(&ClientMessage::Heartbeat { seq })).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
    });

    Ok(GatewayConnection { events })
}
