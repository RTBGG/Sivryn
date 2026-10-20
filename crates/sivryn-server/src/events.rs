//! Event bus that fans gateway events out to every connected client.
//!
//! A single instance uses an in-process broadcast channel. When `SIVRYN_NATS_URL`
//! is configured (and the `nats` feature is on), events are published to NATS
//! and every instance forwards what it receives into its local channel, so
//! clients on any instance see every event.

use sivryn_protocol::gateway::{Audience, Envelope, Event};
use tokio::sync::broadcast;

const LOCAL_CAPACITY: usize = 4096;
#[cfg(feature = "nats")]
const SUBJECT: &str = "sivryn.events";

#[derive(Clone)]
pub struct EventBus {
    local: broadcast::Sender<Envelope>,
    #[cfg(feature = "nats")]
    nats: Option<async_nats::Client>,
}

impl EventBus {
    pub fn in_process() -> Self {
        let (local, _) = broadcast::channel(LOCAL_CAPACITY);
        Self {
            local,
            #[cfg(feature = "nats")]
            nats: None,
        }
    }

    #[cfg(feature = "nats")]
    pub async fn nats(url: &str) -> anyhow::Result<Self> {
        use futures::StreamExt;

        let client = async_nats::connect(url).await?;
        let mut subscriber = client.subscribe(SUBJECT).await?;
        let (local, _) = broadcast::channel(LOCAL_CAPACITY);

        let forward = local.clone();
        tokio::spawn(async move {
            while let Some(msg) = subscriber.next().await {
                match serde_json::from_slice::<Envelope>(&msg.payload) {
                    Ok(event) => {
                        // No receivers is fine: nobody is connected to this instance.
                        let _ = forward.send(event);
                    }
                    Err(e) => tracing::warn!(error = %e, "dropping malformed event from NATS"),
                }
            }
            tracing::error!("NATS subscription ended; cross-instance events stopped");
        });

        Ok(Self { local, nats: Some(client) })
    }

    /// Sends `event` to everyone in `to` (on every instance, with NATS).
    pub fn publish(&self, to: Audience, event: Event) {
        let event = Envelope { to, event };
        #[cfg(feature = "nats")]
        if let Some(client) = &self.nats {
            let client = client.clone();
            tokio::spawn(async move {
                let payload = match serde_json::to_vec(&event) {
                    Ok(p) => p,
                    Err(e) => return tracing::error!(error = %e, "failed to serialize event"),
                };
                if let Err(e) = client.publish(SUBJECT, payload.into()).await {
                    tracing::error!(error = %e, "failed to publish event to NATS");
                }
            });
            return;
        }

        let _ = self.local.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.local.subscribe()
    }
}
