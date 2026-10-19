//! Realtime WebSocket gateway at `/gateway`.

use std::collections::HashSet;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, WebSocketUpgrade, close_code};
use axum::response::Response;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use sivryn_protocol::gateway::{ClientMessage, Event, ServerMessage};
use sivryn_protocol::{DeviceId, GATEWAY_VERSION, GuildId, UserId};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, interval, timeout};

use crate::AppState;
use crate::{auth, db};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// Connections that miss heartbeats for this long are closed.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(75);
const IDENTIFY_TIMEOUT: Duration = Duration::from_secs(10);

const CLOSE_GRACE: Duration = Duration::from_secs(2);

type Tx = SplitSink<WebSocket, WsMessage>;
type Rx = SplitStream<WebSocket>;

pub async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| async move {
        let (mut tx, mut rx) = socket.split();
        let result = run(&mut tx, &mut rx, state).await;
        if let Err(e) = &result {
            tracing::debug!(error = %e, "gateway connection failed");
        }
        close(tx, rx, result.is_ok()).await;
    })
}

/// Performs the WebSocket close handshake instead of dropping the TCP
/// connection. Dropping it while the client still has frames in flight makes
/// the OS reset the connection, and the client can lose the last messages we
/// sent (such as `InvalidSession`).
async fn close(mut tx: Tx, mut rx: Rx, clean: bool) {
    let frame = CloseFrame {
        code: if clean { close_code::NORMAL } else { close_code::POLICY },
        reason: if clean { "".into() } else { "protocol error".into() },
    };
    if tx.send(WsMessage::Close(Some(frame))).await.is_err() {
        return;
    }
    // Wait for the client's close frame (or EOF), discarding anything else.
    let _ = timeout(CLOSE_GRACE, async {
        while let Some(Ok(msg)) = rx.next().await {
            if matches!(msg, WsMessage::Close(_)) {
                break;
            }
        }
    })
    .await;
}

/// What a connection is allowed to see.
struct Session {
    user_id: UserId,
    device_id: Option<DeviceId>,
    guilds: HashSet<GuildId>,
}

impl Session {
    /// Updates membership from the event and decides whether to forward it.
    fn accept(&mut self, event: &Event) -> bool {
        match event {
            Event::GuildCreate { guild } if guild.owner_id == self.user_id => {
                self.guilds.insert(guild.id);
                true
            }
            Event::MemberJoin { guild_id, user } if user.id == self.user_id => {
                self.guilds.insert(*guild_id);
                true
            }
            Event::MemberLeave { guild_id, user_id } if *user_id == self.user_id => self.guilds.remove(guild_id),
            Event::GuildDelete { guild_id } => self.guilds.remove(guild_id),
            Event::MlsWelcome { welcome, .. } => self.device_id.is_some_and(|d| welcome.recipients.contains(&d)),
            // TODO: only forward presence of users sharing a guild.
            Event::PresenceUpdate { .. } => true,
            other => other.guild_id().is_some_and(|g| self.guilds.contains(&g)),
        }
    }
}

async fn send(socket: &mut (impl SinkExt<WsMessage> + Unpin), msg: &ServerMessage) -> anyhow::Result<()> {
    let text = serde_json::to_string(msg)?;
    socket.send(WsMessage::Text(text.into())).await.map_err(|_| anyhow::anyhow!("socket closed"))
}

async fn run(tx: &mut Tx, rx: &mut Rx, state: AppState) -> anyhow::Result<()> {
    send(
        tx,
        &ServerMessage::Hello {
            heartbeat_interval_ms: HEARTBEAT_INTERVAL.as_millis() as u64,
            version: GATEWAY_VERSION,
        },
    )
    .await?;

    // Subscribe before loading state so no event slips through in between.
    let mut events = state.events.subscribe();

    let (token, device_id) = match timeout(IDENTIFY_TIMEOUT, rx.next()).await {
        Ok(Some(Ok(WsMessage::Text(text)))) => match serde_json::from_str(text.as_str()) {
            Ok(ClientMessage::Identify { token, device_id }) => (token, device_id),
            _ => anyhow::bail!("first message must be identify"),
        },
        _ => anyhow::bail!("no identify received"),
    };

    let Some(user_id) = auth::user_for_token(&state.db, &token).await? else {
        let reason = "invalid or expired token".to_owned();
        return send(tx, &ServerMessage::InvalidSession { reason }).await;
    };
    if let Some(device) = device_id
        && db::require_own_device(&state.db, device, user_id).await.is_err()
    {
        let reason = "device does not belong to this account".to_owned();
        return send(tx, &ServerMessage::InvalidSession { reason }).await;
    }

    let user = db::user(&state.db, user_id).await?;
    let guilds = db::user_guilds(&state.db, user_id).await?;
    let mut session = Session { user_id, device_id, guilds: guilds.iter().map(|g| g.id).collect() };
    send(tx, &ServerMessage::Ready { user: user.into(), guilds }).await?;
    state.events.publish(Event::PresenceUpdate { user_id, online: true });

    let mut last_heartbeat = Instant::now();
    let mut watchdog = interval(Duration::from_secs(5));

    let result = loop {
        tokio::select! {
            incoming = rx.next() => {
                let Some(Ok(frame)) = incoming else { break Ok(()) };
                match frame {
                    WsMessage::Text(text) => match serde_json::from_str::<ClientMessage>(text.as_str()) {
                        Ok(ClientMessage::Heartbeat { seq }) => {
                            last_heartbeat = Instant::now();
                            send(tx, &ServerMessage::HeartbeatAck { seq }).await?;
                        }
                        Ok(ClientMessage::Identify { .. }) => break Err(anyhow::anyhow!("already identified")),
                        Err(e) => break Err(anyhow::anyhow!("invalid client message: {e}")),
                    },
                    WsMessage::Close(_) => break Ok(()),
                    _ => {}
                }
            }
            event = events.recv() => match event {
                Ok(event) => {
                    if session.accept(&event) {
                        send(tx, &ServerMessage::Event(event)).await?;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    // The client fell too far behind; make it reconnect and resync.
                    tracing::warn!(%user_id, skipped, "gateway client lagged");
                    break Ok(());
                }
                Err(RecvError::Closed) => break Ok(()),
            },
            _ = watchdog.tick() => {
                if last_heartbeat.elapsed() > HEARTBEAT_TIMEOUT {
                    break Ok(());
                }
            }
        }
    };

    // TODO: track connection counts per user so a second device staying online
    // doesn't get reported as offline.
    state.events.publish(Event::PresenceUpdate { user_id, online: false });
    result
}
