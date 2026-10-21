use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sivryn_protocol::api::*;
use sivryn_protocol::model::{
    Category, Channel, Guild, Message, NotifyLevel, OverwriteTarget, PermissionOverwrite, Role, User,
};
use sivryn_protocol::{CategoryId, ChannelId, DeviceId, FileId, GuildId, MessageId, RoleId, UserId};

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
    /// The thing asked for doesn't exist (any more).
    pub fn is_not_found(&self) -> bool {
        matches!(self, ClientError::Api { status, .. } if *status == StatusCode::NOT_FOUND)
    }

    /// A network hiccup worth retrying (connection broke, nothing arrived).
    pub fn is_transient(&self) -> bool {
        match self {
            ClientError::Network(e) => !e.is_builder() && !e.is_redirect(),
            ClientError::Api { status, .. } => *status == StatusCode::GATEWAY_TIMEOUT,
            _ => false,
        }
    }

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
        Self::finish(req).await
    }

    async fn finish<R: DeserializeOwned>(req: reqwest::RequestBuilder) -> Result<R, ClientError> {
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

    async fn put<B: Serialize, R: DeserializeOwned>(&self, path: &str, body: &B) -> Result<R, ClientError> {
        self.request(Method::PUT, path, Some(body), true).await
    }

    async fn patch<B: Serialize, R: DeserializeOwned>(&self, path: &str, body: &B) -> Result<R, ClientError> {
        self.request(Method::PATCH, path, Some(body), true).await
    }

    async fn delete(&self, path: &str) -> Result<(), ClientError> {
        self.request::<(), ()>(Method::DELETE, path, None, true).await
    }

    fn authed(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, ClientError> {
        let token = self.token.as_deref().ok_or(ClientError::NotLoggedIn)?;
        Ok(self.http.request(method, format!("{}/api/v1{path}", self.base)).bearer_auth(token))
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

    /// Creates a guild; the category names are in the creator's language.
    pub async fn create_guild(
        &self,
        name: &str,
        text_category: &str,
        voice_category: &str,
    ) -> Result<GuildWithChannels, ClientError> {
        let req = CreateGuildRequest {
            name: name.into(),
            text_category: Some(text_category.into()),
            voice_category: Some(voice_category.into()),
        };
        self.post("/guilds", &req).await
    }

    pub async fn members(&self, guild: GuildId) -> Result<Vec<GuildMember>, ClientError> {
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

/// Social features, moderation and files.
impl ApiClient {
    pub async fn profile(&self, user: UserId) -> Result<UserProfile, ClientError> {
        self.get(&format!("/users/{user}")).await
    }

    /// Users you can see who are online right now.
    pub async fn presence(&self) -> Result<Vec<UserPresence>, ClientError> {
        self.get("/presence").await
    }

    pub async fn me_settings(&self) -> Result<MeSettings, ClientError> {
        self.get("/users/@me/settings").await
    }

    pub async fn update_me(&self, req: &UpdateMeRequest) -> Result<MeSettings, ClientError> {
        self.patch("/users/@me", req).await
    }

    pub async fn friends(&self) -> Result<FriendsResponse, ClientError> {
        self.get("/friends").await
    }

    pub async fn send_friend_request(&self, username: &str) -> Result<(), ClientError> {
        self.post("/friends/requests", &FriendRequest { username: username.into() }).await
    }

    pub async fn accept_friend(&self, user: UserId) -> Result<(), ClientError> {
        self.post(&format!("/friends/requests/{user}/accept"), &()).await
    }

    /// Declines an incoming or cancels an outgoing request.
    pub async fn cancel_friend_request(&self, user: UserId) -> Result<(), ClientError> {
        self.delete(&format!("/friends/requests/{user}")).await
    }

    pub async fn remove_friend(&self, user: UserId) -> Result<(), ClientError> {
        self.delete(&format!("/friends/{user}")).await
    }

    pub async fn block(&self, user: UserId) -> Result<(), ClientError> {
        self.put(&format!("/blocks/{user}"), &()).await
    }

    pub async fn unblock(&self, user: UserId) -> Result<(), ClientError> {
        self.delete(&format!("/blocks/{user}")).await
    }

    pub async fn dms(&self) -> Result<Vec<DmChannel>, ClientError> {
        self.get("/dms").await
    }

    pub async fn open_dm(&self, users: Vec<UserId>) -> Result<DmChannel, ClientError> {
        self.post("/dms", &OpenDmRequest { user_ids: users }).await
    }

    pub async fn add_dm_member(&self, channel: ChannelId, user: UserId) -> Result<DmChannel, ClientError> {
        self.post(&format!("/dms/{channel}/members"), &AddDmMemberRequest { user_id: user }).await
    }

    pub async fn leave_dm(&self, channel: ChannelId) -> Result<(), ClientError> {
        self.delete(&format!("/dms/{channel}/members/@me")).await
    }

    pub async fn rename_dm(&self, channel: ChannelId, name: &str) -> Result<DmChannel, ClientError> {
        self.patch(&format!("/dms/{channel}"), &RenameDmRequest { name: name.into() }).await
    }

    pub async fn channel_members(&self, channel: ChannelId) -> Result<Vec<User>, ClientError> {
        self.get(&format!("/channels/{channel}/members")).await
    }

    pub async fn update_guild(&self, guild: GuildId, req: &UpdateGuildRequest) -> Result<Guild, ClientError> {
        self.patch(&format!("/guilds/{guild}"), req).await
    }

    pub async fn set_guild_icon(
        &self,
        guild: GuildId,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<UploadedFile, ClientError> {
        let req = self
            .authed(Method::PUT, &format!("/guilds/{guild}/icon"))?
            .header("content-type", content_type)
            .body(bytes);
        Self::finish(req).await
    }

    /// Kicks a member, or leaves the guild when `user` is yourself.
    pub async fn remove_member(&self, guild: GuildId, user: UserId) -> Result<(), ClientError> {
        self.delete(&format!("/guilds/{guild}/members/{user}")).await
    }

    pub async fn bans(&self, guild: GuildId) -> Result<Vec<Ban>, ClientError> {
        self.get(&format!("/guilds/{guild}/bans")).await
    }

    pub async fn ban(&self, guild: GuildId, user: UserId, reason: Option<String>) -> Result<(), ClientError> {
        self.put(&format!("/guilds/{guild}/bans/{user}"), &BanRequest { reason }).await
    }

    pub async fn unban(&self, guild: GuildId, user: UserId) -> Result<(), ClientError> {
        self.delete(&format!("/guilds/{guild}/bans/{user}")).await
    }

    pub async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Result<(), ClientError> {
        self.delete(&format!("/channels/{channel}/messages/{message}")).await
    }

    /// Uploads an (already encrypted) attachment.
    pub async fn upload_attachment(&self, channel: ChannelId, bytes: Vec<u8>) -> Result<UploadedFile, ClientError> {
        let req = self
            .authed(Method::POST, &format!("/channels/{channel}/attachments"))?
            .header("content-type", "application/octet-stream")
            .body(bytes);
        Self::finish(req).await
    }

    pub async fn download_file(&self, file: FileId) -> Result<Vec<u8>, ClientError> {
        self.download_file_with_progress(file, |_, _| {}).await
    }

    /// Downloads a file, reporting the bytes received so far and the total
    /// (when known). A download that receives nothing for a while, or whose
    /// connection breaks, resumes where it stopped (`Range`); a server that
    /// doesn't support that sends it from the start again.
    pub async fn download_file_with_progress(
        &self,
        file: FileId,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, ClientError> {
        /// Failed attempts in a row (without new data) before giving up.
        const ATTEMPTS: u32 = 5;
        let started = std::time::Instant::now();
        let mut data = Vec::new();
        let mut total = None;
        let mut failures = 0;
        loop {
            let before = data.len();
            match self.download_part(file, &mut data, &mut total, &mut progress).await {
                Ok(()) => break,
                Err(e) if e.is_transient() && failures + 1 < ATTEMPTS => {
                    failures = if data.len() > before { 0 } else { failures + 1 };
                    tracing::warn!(%file, received = data.len(), error = %e, "download interrupted, resuming");
                }
                Err(e) => return Err(e),
            }
        }
        tracing::debug!(%file, bytes = data.len(), elapsed = ?started.elapsed(), "download finished");
        Ok(data)
    }

    /// One request of a download: the rest of the file after `data`.
    async fn download_part(
        &self,
        file: FileId,
        data: &mut Vec<u8>,
        total: &mut Option<u64>,
        progress: &mut impl FnMut(u64, Option<u64>),
    ) -> Result<(), ClientError> {
        let stalled = || ClientError::Api {
            status: StatusCode::GATEWAY_TIMEOUT,
            code: "download".into(),
            message: "download stalled".into(),
        };
        let mut req = self.authed(Method::GET, &format!("/files/{file}"))?;
        if !data.is_empty() {
            req = req.header(reqwest::header::RANGE, format!("bytes={}-", data.len()));
        }
        let mut resp =
            tokio::time::timeout(std::time::Duration::from_secs(20), req.send()).await.map_err(|_| stalled())??;
        let status = resp.status();
        // A server that can resume gets a short wait before resuming; one that
        // can't (it would start over) gets a long one.
        let resumable = resp.headers().get(reqwest::header::ACCEPT_RANGES).is_some_and(|v| v.as_bytes() == b"bytes");
        let patience = std::time::Duration::from_secs(if resumable { 20 } else { 120 });
        match status {
            StatusCode::PARTIAL_CONTENT => {
                // "bytes start-end/total": it must continue exactly where we are.
                let range = resp.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok());
                let parsed = range.and_then(|r| {
                    let (span, size) = r.strip_prefix("bytes ")?.split_once('/')?;
                    Some((span.split_once('-')?.0.parse::<usize>().ok()?, size.parse::<u64>().ok()))
                });
                match parsed {
                    Some((start, size)) if start == data.len() => *total = size.or(*total),
                    _ => data.clear(),
                }
            }
            s if s.is_success() => {
                // The whole file (first request, or the server ignored the range).
                data.clear();
                *total = resp.content_length();
            }
            _ => return Err(ClientError::Api { status, code: "download".into(), message: "file unavailable".into() }),
        }
        tracing::debug!(%file, %status, offset = data.len(), total = ?*total, "download started");
        while let Some(chunk) = tokio::time::timeout(patience, resp.chunk()).await.map_err(|_| stalled())?? {
            data.extend_from_slice(&chunk);
            progress(data.len() as u64, *total);
        }
        if total.is_some_and(|t| (data.len() as u64) < t) {
            return Err(stalled());
        }
        Ok(())
    }

    pub async fn channel_voice_state(&self, channel: ChannelId) -> Result<VoiceChannelState, ClientError> {
        self.get(&format!("/channels/{channel}/voice")).await
    }

    /// Deletes a guild (owner only).
    pub async fn delete_guild(&self, guild: GuildId) -> Result<(), ClientError> {
        self.delete(&format!("/guilds/{guild}")).await
    }

    pub async fn update_channel(&self, channel: ChannelId, req: &UpdateChannelRequest) -> Result<Channel, ClientError> {
        self.patch(&format!("/channels/{channel}"), req).await
    }

    pub async fn delete_channel(&self, channel: ChannelId) -> Result<(), ClientError> {
        self.delete(&format!("/channels/{channel}")).await
    }

    pub async fn create_category(&self, guild: GuildId, name: &str) -> Result<Category, ClientError> {
        self.post(&format!("/guilds/{guild}/categories"), &CategoryRequest { name: name.into() }).await
    }

    pub async fn rename_category(&self, category: CategoryId, name: &str) -> Result<(), ClientError> {
        self.patch(&format!("/categories/{category}"), &CategoryRequest { name: name.into() }).await
    }

    pub async fn delete_category(&self, category: CategoryId) -> Result<(), ClientError> {
        self.delete(&format!("/categories/{category}")).await
    }

    /// Your name (`Some(None)` clears it) or notification level in a guild.
    pub async fn update_my_member(
        &self,
        guild: GuildId,
        nick: Option<Option<String>>,
        notify_level: Option<NotifyLevel>,
    ) -> Result<(), ClientError> {
        self.patch(&format!("/guilds/{guild}/members/@me"), &UpdateMemberRequest { nick, notify_level }).await
    }

    pub async fn read_states(&self) -> Result<Vec<ReadState>, ClientError> {
        self.get("/users/@me/read-states").await
    }

    /// Marks a channel read up to `message`.
    pub async fn ack(&self, channel: ChannelId, message: MessageId) -> Result<(), ClientError> {
        self.put(&format!("/channels/{channel}/read"), &AckRequest { message_id: message }).await
    }

    pub async fn mutes(&self) -> Result<Vec<ChannelMute>, ClientError> {
        self.get("/users/@me/mutes").await
    }

    /// Mutes a channel until `until`, or until unmuted.
    pub async fn mute(&self, channel: ChannelId, until: Option<DateTime<Utc>>) -> Result<(), ClientError> {
        self.put(&format!("/channels/{channel}/mute"), &MuteRequest { until }).await
    }

    pub async fn unmute(&self, channel: ChannelId) -> Result<(), ClientError> {
        self.delete(&format!("/channels/{channel}/mute")).await
    }

    /// Sets the profile picture (`banner: false`) or banner. Body: the image.
    pub async fn set_profile_image(
        &self,
        banner: bool,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<UploadedFile, ClientError> {
        let path = if banner { "/users/@me/banner" } else { "/users/@me/avatar" };
        let req = self.authed(Method::PUT, path)?.header("content-type", content_type).body(bytes);
        Self::finish(req).await
    }

    pub async fn remove_profile_image(&self, banner: bool) -> Result<(), ClientError> {
        self.delete(if banner { "/users/@me/banner" } else { "/users/@me/avatar" }).await
    }

    // --- Roles and permissions ---------------------------------------------------

    pub async fn create_role(&self, guild: GuildId, req: &RoleRequest) -> Result<Role, ClientError> {
        self.post(&format!("/guilds/{guild}/roles"), req).await
    }

    pub async fn update_role(&self, role: RoleId, req: &RoleRequest) -> Result<Role, ClientError> {
        self.patch(&format!("/roles/{role}"), req).await
    }

    pub async fn delete_role(&self, role: RoleId) -> Result<(), ClientError> {
        self.delete(&format!("/roles/{role}")).await
    }

    /// Orders the roles from highest to lowest (without `@everyone`).
    pub async fn reorder_roles(&self, guild: GuildId, roles: Vec<RoleId>) -> Result<(), ClientError> {
        self.put(&format!("/guilds/{guild}/roles/order"), &RoleOrderRequest { roles }).await
    }

    /// Gives (`add`) or takes a role.
    pub async fn set_member_role(
        &self,
        guild: GuildId,
        user: UserId,
        role: RoleId,
        add: bool,
    ) -> Result<(), ClientError> {
        let path = format!("/guilds/{guild}/members/{user}/roles/{role}");
        if add { self.put(&path, &()).await } else { self.delete(&path).await }
    }

    /// Sets (or with empty allow and deny: removes) an overwrite on a channel
    /// (`category: None`) or a category.
    pub async fn set_overwrite(
        &self,
        channel: Option<ChannelId>,
        category: Option<CategoryId>,
        overwrite: &PermissionOverwrite,
    ) -> Result<(), ClientError> {
        let base = match (channel, category) {
            (Some(c), _) => format!("/channels/{c}/overwrites"),
            (None, Some(c)) => format!("/categories/{c}/overwrites"),
            (None, None) => return Ok(()),
        };
        if overwrite.allow.is_empty() && overwrite.deny.is_empty() {
            let target = match overwrite.target {
                OverwriteTarget::Role(r) => format!("role/{r}"),
                OverwriteTarget::Member(u) => format!("member/{u}"),
            };
            self.delete(&format!("{base}/{target}")).await
        } else {
            self.put(&base, overwrite).await
        }
    }

    /// Saves the whole channel list order after a drag.
    pub async fn set_layout(&self, guild: GuildId, req: &LayoutRequest) -> Result<(), ClientError> {
        self.put(&format!("/guilds/{guild}/layout"), req).await
    }

    /// Moves someone into another voice channel (needs `MOVE_MEMBERS`).
    pub async fn move_voice_member(&self, guild: GuildId, user: UserId, channel: ChannelId) -> Result<(), ClientError> {
        self.post(&format!("/guilds/{guild}/voice/move"), &MoveMemberRequest { user_id: user, channel_id: channel })
            .await
    }

    /// Disconnects someone from a guild voice channel (needs `MOVE_MEMBERS`).
    pub async fn disconnect_voice(&self, channel: ChannelId, user: UserId) -> Result<(), ClientError> {
        self.delete(&format!("/channels/{channel}/voice/{user}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A server that sends at most `cut` bytes per response and then drops
    /// the connection; `ranges` says whether it honours `Range`.
    async fn flaky_server(file: Vec<u8>, cut: usize, ranges: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut req = vec![0u8; 4096];
                let n = sock.read(&mut req).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&req[..n]).to_lowercase();
                let start = req
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| r.trim().trim_end_matches('-').parse::<usize>().ok())
                    .filter(|_| ranges)
                    .unwrap_or(0);
                let rest = &file[start..];
                let head = if start > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nAccept-Ranges: bytes\r\n\r\n",
                        rest.len(),
                        file.len() - 1,
                        file.len()
                    )
                } else {
                    let accept = if ranges { "Accept-Ranges: bytes\r\n" } else { "" };
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{accept}\r\n", rest.len())
                };
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&rest[..rest.len().min(cut)]).await;
                // Dropping the socket cuts the response short.
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn broken_downloads_resume_where_they_stopped() {
        let file: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let base = flaky_server(file.clone(), 100_000, true).await;
        let api = ApiClient::new(&base).unwrap().with_token("t".into());
        let mut reports = 0;
        let got = api.download_file_with_progress(FileId::new(), |_, _| reports += 1).await.unwrap();
        assert_eq!(got, file);
        assert!(reports > 0);
    }

    #[tokio::test]
    async fn without_range_support_it_gives_up_instead_of_looping() {
        let file = vec![7u8; 50_000];
        let base = flaky_server(file, 10_000, false).await;
        let api = ApiClient::new(&base).unwrap().with_token("t".into());
        assert!(api.download_file(FileId::new()).await.is_err());
    }
}
