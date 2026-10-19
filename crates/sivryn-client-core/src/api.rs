use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sivryn_protocol::api::*;
use sivryn_protocol::model::{Channel, Message, User};
use sivryn_protocol::{ChannelId, DeviceId, GuildId, UserId};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("server returned {status}: {message}")]
    Api { status: StatusCode, code: String, message: String },
    #[error("not logged in")]
    NotLoggedIn,
    #[error("invalid instance URL: {0}")]
    InvalidUrl(String),
}

impl ClientError {
    /// The session token was rejected (expired or logged out elsewhere).
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ClientError::Api { status, .. } if *status == StatusCode::UNAUTHORIZED)
    }

    /// The resource doesn't exist or isn't ours (e.g. a device the server forgot).
    pub fn is_missing(&self) -> bool {
        matches!(self, ClientError::Api { status, .. } if matches!(*status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN))
    }
}

/// REST client for one Sivryn instance.
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    /// Instance base URL without trailing slash, e.g. `https://chat.example.com`.
    base: String,
    token: Option<String>,
}

impl ApiClient {
    pub fn new(instance_url: &str) -> Result<Self, ClientError> {
        let base = instance_url.trim_end_matches('/').to_owned();
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(ClientError::InvalidUrl(instance_url.to_owned()));
        }
        let http =
            reqwest::Client::builder().user_agent(concat!("sivryn-client/", env!("CARGO_PKG_VERSION"))).build()?;
        Ok(Self { http, base, token: None })
    }

    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// WebSocket URL of the gateway (`wss://…/gateway`).
    pub fn gateway_url(&self) -> String {
        let ws = self.base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        format!("{ws}/gateway")
    }

    async fn request<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        auth: bool,
    ) -> Result<R, ClientError> {
        let mut req = self.http.request(method, format!("{}/api/v1{path}", self.base));
        if auth {
            req = req.bearer_auth(self.token.as_deref().ok_or(ClientError::NotLoggedIn)?);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            // Endpoints answering 204 deserialize into `()` from `null`.
            if status == StatusCode::NO_CONTENT {
                return Ok(serde_json::from_str("null").expect("unit deserializes from null"));
            }
            return Ok(resp.json().await?);
        }
        let err = resp.json::<ErrorBody>().await.unwrap_or_else(|_| ErrorBody {
            error: "unknown".into(),
            message: status.canonical_reason().unwrap_or("request failed").into(),
        });
        Err(ClientError::Api { status, code: err.error, message: err.message })
    }

    async fn get<R: DeserializeOwned>(&self, path: &str) -> Result<R, ClientError> {
        self.request::<(), R>(Method::GET, path, None, true).await
    }

    async fn post<B: Serialize, R: DeserializeOwned>(&self, path: &str, body: &B) -> Result<R, ClientError> {
        self.request(Method::POST, path, Some(body), true).await
    }

    pub async fn instance_info(&self) -> Result<InstanceInfo, ClientError> {
        self.request::<(), _>(Method::GET, "/instance", None, false).await
    }

    /// Registers and stores the session token on success.
    pub async fn register(&mut self, req: &RegisterRequest) -> Result<User, ClientError> {
        let session: SessionResponse = self.request(Method::POST, "/auth/register", Some(req), false).await?;
        self.token = Some(session.token);
        Ok(session.user)
    }

    /// Logs in and stores the session token on success.
    pub async fn login(&mut self, username: &str, password: &str) -> Result<User, ClientError> {
        let req = LoginRequest { username: username.into(), password: password.into() };
        let session: SessionResponse = self.request(Method::POST, "/auth/login", Some(&req), false).await?;
        self.token = Some(session.token);
        Ok(session.user)
    }

    pub async fn me(&self) -> Result<User, ClientError> {
        self.get("/users/@me").await
    }

    pub async fn guilds(&self) -> Result<Vec<GuildWithChannels>, ClientError> {
        self.get("/guilds").await
    }

    pub async fn create_guild(&self, name: &str) -> Result<GuildWithChannels, ClientError> {
        self.post("/guilds", &CreateGuildRequest { name: name.into() }).await
    }

    pub async fn members(&self, guild: GuildId) -> Result<Vec<User>, ClientError> {
        self.get(&format!("/guilds/{guild}/members")).await
    }

    pub async fn create_invite(&self, guild: GuildId) -> Result<String, ClientError> {
        #[derive(serde::Deserialize)]
        struct Invite {
            code: String,
        }
        Ok(self.post::<_, Invite>(&format!("/guilds/{guild}/invites"), &()).await?.code)
    }

    pub async fn accept_invite(&self, code: &str) -> Result<GuildWithChannels, ClientError> {
        self.post(&format!("/invites/{code}"), &()).await
    }

    pub async fn create_channel(&self, guild: GuildId, req: &CreateChannelRequest) -> Result<Channel, ClientError> {
        self.post(&format!("/guilds/{guild}/channels"), req).await
    }

    pub async fn messages(&self, channel: ChannelId, query: &MessageQuery) -> Result<Vec<Message>, ClientError> {
        let mut path = format!("/channels/{channel}/messages?limit={}", query.limit.unwrap_or(50));
        if let Some(before) = query.before {
            path.push_str(&format!("&before={before}"));
        }
        if let Some(after) = query.after {
            path.push_str(&format!("&after={after}"));
        }
        Ok(self.get::<MessagePage>(&path).await?.messages)
    }

    pub async fn send_message(&self, channel: ChannelId, req: &SendMessageRequest) -> Result<Message, ClientError> {
        self.post(&format!("/channels/{channel}/messages"), req).await
    }

    pub async fn register_device(&self, req: &RegisterDeviceRequest) -> Result<RegisterDeviceResponse, ClientError> {
        self.post("/devices", req).await
    }

    pub async fn key_package_count(&self, device: DeviceId) -> Result<u32, ClientError> {
        Ok(self.get::<KeyPackageCount>(&format!("/devices/{device}/key-packages")).await?.count)
    }

    pub async fn upload_key_packages(&self, device: DeviceId, key_packages: Vec<String>) -> Result<(), ClientError> {
        #[derive(Serialize)]
        struct Upload {
            key_packages: Vec<String>,
        }
        self.post(&format!("/devices/{device}/key-packages"), &Upload { key_packages }).await
    }

    pub async fn voice_states(&self, guild: GuildId) -> Result<Vec<VoiceChannelState>, ClientError> {
        self.get(&format!("/guilds/{guild}/voice")).await
    }

    /// Ends the session on the server. The token is useless afterwards.
    pub async fn logout(&self) -> Result<(), ClientError> {
        self.post("/auth/logout", &()).await
    }

    pub async fn claim_key_packages(&self, user: UserId) -> Result<Vec<ClaimedKeyPackage>, ClientError> {
        self.post(&format!("/users/{user}/key-packages/claim"), &()).await
    }

    pub async fn mls_commit(&self, channel: ChannelId, req: &MlsCommitRequest) -> Result<(), ClientError> {
        self.post(&format!("/channels/{channel}/mls/commits"), req).await
    }

    pub async fn mls_commits_since(&self, channel: ChannelId, since: u64) -> Result<Vec<StoredCommit>, ClientError> {
        self.get(&format!("/channels/{channel}/mls/commits?since={since}")).await
    }

    pub async fn take_welcomes(&self, device: DeviceId) -> Result<Vec<PendingWelcome>, ClientError> {
        self.get(&format!("/devices/{device}/welcomes")).await
    }

    pub async fn join_voice(&self, channel: ChannelId) -> Result<VoiceJoinResponse, ClientError> {
        self.post(&format!("/channels/{channel}/voice/join"), &()).await
    }
}
