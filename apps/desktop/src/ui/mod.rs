//! Application state and the root component.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine;
use dioxus_native::prelude::dioxus_core::{Task, provide_root_context, spawn_forever};
use dioxus_native::prelude::*;
use sivryn_client_core::connect_gateway;
use sivryn_client_core::protocol::api::{
    DmChannel, FriendsResponse, GuildWithChannels, VoiceJoinResponse, VoiceParticipant,
};
use sivryn_client_core::protocol::gateway::{Event, ServerMessage};
use sivryn_client_core::protocol::model::{Channel, ChannelKind, Guild, User};
use sivryn_client_core::protocol::permissions::Permissions;
use sivryn_client_core::protocol::{ChannelId, GuildId, MessageId, UserId};
use sivryn_media::{AudioEngine, CameraShare, RgbaFrame, ScreenAudio, ScreenShare, Sound, VideoFeed, VoiceCall};
use tokio::sync::watch;

use crate::chat;
use crate::files::PendingFile;
use crate::hotkeys::{HotkeyEvent, Hotkeys};
use crate::i18n::{self, t};
use crate::messages::{ChatMessage, Record, merge};
use crate::session::Session;
use crate::settings::{Settings, Theme};

mod call;
mod chat_view;
mod emoji_picker;
mod guild_settings;
mod home;
pub mod icons;
mod login;
mod media;
mod members;
mod menu;
mod profile;
mod settings_view;
mod shell;
pub mod widgets;

pub use menu::{ContextMenu, MenuItem};

const STYLE: &str = include_str!("style.css");
const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");
const TWEMOJI: &[u8] = include_bytes!("../../assets/fonts/TwemojiMozilla.ttf");

/// `@font-face` rules for the bundled fonts, as data URLs (built once).
fn font_css() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        format!(
            "@font-face {{ font-family: 'Inter'; font-weight: 100 900; font-style: normal; \
             src: url(data:font/ttf;base64,{}) format('truetype'); }}\n\
             @font-face {{ font-family: 'Twemoji'; src: url(data:font/ttf;base64,{}) format('truetype'); }}",
            b64(INTER),
            b64(TWEMOJI)
        )
    })
}

/// Someone in the call, as the UI shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Participant {
    pub identity: String,
    pub name: String,
    pub muted: bool,
    pub sharing: bool,
}

#[derive(Clone)]
pub struct CallState {
    pub channel: ChannelId,
    pub join: VoiceJoinResponse,
    pub call: Rc<RefCell<Option<VoiceCall>>>,
    pub share: Rc<RefCell<Option<ScreenShare>>>,
    pub share_audio: Rc<RefCell<Option<ScreenAudio>>>,
    pub camera: Rc<RefCell<Option<CameraShare>>>,
    pub identity: String,
    pub participants: Vec<Participant>,
    pub feeds: Vec<VideoFeed>,
    pub speakers: Vec<String>,
    pub sharing: bool,
    pub camera_on: bool,
    /// Track shown large; `None` picks the first screen share.
    pub focus: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Modal {
    CreateGuild,
    JoinGuild,
    Invite,
    PickScreen,
    GuildSettings(GuildId),
    /// Pick friends to start a DM or group.
    NewDm,
    /// Pick someone to add to a DM or group.
    AddToDm(ChannelId),
    ConfirmDelete(ChannelId, MessageId),
    /// "Kick / ban this member?"
    ConfirmModeration {
        guild: GuildId,
        user: UserId,
        ban: bool,
    },
    Profile(UserId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Account,
    Privacy,
    Voice,
    Video,
    Keybinds,
    Appearance,
    Notifications,
    About,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Online,
    Offline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

/// What the message box of a channel is doing.
#[derive(Clone, Default, PartialEq)]
pub struct Composer {
    pub draft: String,
    pub reply_to: Option<MessageId>,
    pub editing: Option<MessageId>,
    pub files: Vec<PendingFile>,
    /// Uploading and sending; the box keeps showing the files meanwhile.
    pub sending: bool,
}

/// An image shown full size over everything else.
#[derive(Clone)]
pub struct ViewerImage {
    pub frames: watch::Receiver<Option<Arc<RgbaFrame>>>,
    pub width: u32,
    pub height: u32,
    pub name: String,
    /// Where the image came from (a link), shown while hovering.
    pub url: Option<String>,
}

impl PartialEq for ViewerImage {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.width == other.width && self.height == other.height
    }
}

/// Engine and hotkeys live for the whole app run.
#[derive(Clone)]
pub struct Services {
    pub engine: AudioEngine,
    pub hotkeys: Hotkeys,
}

/// Global UI state, provided as context by [`App`].
#[derive(Clone, Copy)]
pub struct AppState {
    pub session: Signal<Option<Session>>,
    /// Signing in with a saved token at startup.
    pub restoring: Signal<bool>,
    pub guilds: Signal<Vec<GuildWithChannels>>,
    pub dms: Signal<Vec<DmChannel>>,
    pub friends: Signal<FriendsResponse>,
    pub online: Signal<HashSet<UserId>>,
    pub users: Signal<HashMap<UserId, String>>,
    /// Members of each guild.
    pub members: Signal<HashMap<GuildId, Vec<User>>>,
    /// The home view (friends and DMs) instead of a guild.
    pub home: Signal<bool>,
    pub guild: Signal<Option<GuildId>>,
    pub channel: Signal<Option<ChannelId>>,
    pub messages: Signal<HashMap<ChannelId, Vec<ChatMessage>>>,
    pub composer: Signal<HashMap<ChannelId, Composer>>,
    /// Channels whose MLS group this device is in.
    pub groups_ready: Signal<HashSet<ChannelId>>,
    pub unread: Signal<HashSet<ChannelId>>,
    pub voice: Signal<HashMap<ChannelId, Vec<VoiceParticipant>>>,
    pub call: Signal<Option<CallState>>,
    /// An incoming DM or group call: channel and caller.
    pub ringing: Signal<Option<(ChannelId, UserId)>>,
    pub muted: Signal<bool>,
    pub deafened: Signal<bool>,
    pub modal: Signal<Option<Modal>>,
    pub menu: Signal<Option<ContextMenu>>,
    pub picker: Signal<Option<emoji_picker::PickerTarget>>,
    pub viewer: Signal<Option<ViewerImage>>,
    /// A file is being dragged over the window.
    pub drag_over: Signal<bool>,
    pub settings_page: Signal<Option<SettingsPage>>,
    pub settings: Signal<Settings>,
    pub connection: Signal<Connection>,
    pub toasts: Signal<Vec<Toast>>,
    pub gateway: Signal<Option<Task>>,
}

impl AppState {
    pub fn report(self, result: Result<(), String>) {
        if let Err(e) = result {
            tracing::warn!(error = %e);
            self.toast(e, true);
        }
    }

    pub fn toast(mut self, text: impl Into<String>, error: bool) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let text = text.into();
        // Don't stack the same message (e.g. repeated connection errors).
        if self.toasts.peek().iter().any(|t| t.text == text) {
            return;
        }
        self.toasts.write().push(Toast { id, text, error });
        let mut toasts = self.toasts;
        spawn_forever(async move {
            tokio::time::sleep(Duration::from_secs(if error { 8 } else { 4 })).await;
            toasts.write().retain(|t| t.id != id);
        });
    }

    pub fn me(&self) -> Option<UserId> {
        self.session.peek().as_ref().map(|s| s.user.id)
    }

    pub fn username(&self, id: UserId) -> String {
        self.users.read().get(&id).cloned().unwrap_or_else(|| t("common.unknown").into())
    }

    /// Any channel this user can see: guild channels and DMs.
    pub fn find_channel(&self, id: ChannelId) -> Option<Channel> {
        if let Some(c) = self.guilds.read().iter().flat_map(|g| &g.channels).find(|c| c.id == id) {
            return Some(c.clone());
        }
        self.dms.read().iter().find(|d| d.channel.id == id).map(|d| d.channel.clone())
    }

    pub fn channel_kind(&self, id: ChannelId) -> Option<ChannelKind> {
        self.find_channel(id).map(|c| c.kind)
    }

    pub fn dm(&self, id: ChannelId) -> Option<DmChannel> {
        self.dms.read().iter().find(|d| d.channel.id == id).cloned()
    }

    /// The name shown for a channel: `#name`, the other person, or the group.
    pub fn channel_name(&self, id: ChannelId) -> String {
        if let Some(dm) = self.dm(id) {
            return self.dm_title(&dm);
        }
        self.find_channel(id).map(|c| c.name).unwrap_or_default()
    }

    pub fn dm_title(&self, dm: &DmChannel) -> String {
        if !dm.channel.name.is_empty() {
            return dm.channel.name.clone();
        }
        let me = self.me();
        let others: Vec<String> = dm
            .members
            .iter()
            .filter(|u| Some(u.id) != me)
            .map(|u| u.display_name.clone().unwrap_or_else(|| u.username.clone()))
            .collect();
        if others.is_empty() { t("home.just_you").into() } else { others.join(", ") }
    }

    pub fn guild_of(&self, id: ChannelId) -> Option<GuildId> {
        self.find_channel(id).and_then(|c| c.guild_id)
    }

    pub fn guild(&self, id: GuildId) -> Option<Guild> {
        self.guilds.read().iter().find(|g| g.guild.id == id).map(|g| g.guild.clone())
    }

    /// My permissions in a guild (none outside guilds).
    pub fn permissions(&self, guild: GuildId) -> Permissions {
        self.guilds.read().iter().find(|g| g.guild.id == guild).map(|g| g.permissions).unwrap_or(Permissions::empty())
    }

    /// Changes settings, applies them and saves them to disk.
    pub fn update_settings(mut self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings.write());
        self.settings.peek().save();
    }

    /// Opens a channel (guild channel or DM) and marks it read.
    pub fn open_channel(mut self, channel: ChannelId) {
        match self.guild_of(channel) {
            Some(guild) => {
                self.home.set(false);
                self.guild.set(Some(guild));
            }
            None => self.home.set(true),
        }
        self.channel.set(Some(channel));
        self.unread.write().remove(&channel);
    }

    pub fn open_home(mut self) {
        self.home.set(true);
        self.channel.set(None);
    }

    /// Opens (or creates) the DM with these users and shows it.
    pub fn message_users(self, users: Vec<UserId>) {
        let Some(s) = self.session.cloned() else { return };
        spawn_forever(async move {
            match s.api.open_dm(users).await {
                Ok(dm) => {
                    let id = dm.channel.id;
                    self.add_dm(dm);
                    self.open_channel(id);
                }
                Err(e) => self.report(Err(e.to_string())),
            }
        });
    }

    fn add_dm(mut self, dm: DmChannel) {
        for u in &dm.members {
            self.users.write().insert(u.id, u.display_name.clone().unwrap_or_else(|| u.username.clone()));
        }
        let mut dms = self.dms.write();
        dms.retain(|d| d.channel.id != dm.channel.id);
        dms.insert(0, dm);
    }

    /// Reloads guilds and member names after membership changes.
    pub async fn refresh(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let guilds = s.api.guilds().await.map_err(|e| e.to_string())?;
        let mut users = self.users.peek().clone();
        let mut members = HashMap::new();
        users.insert(s.user.id, s.user.display_name.clone().unwrap_or_else(|| s.user.username.clone()));
        for g in &guilds {
            let list = s.api.members(g.guild.id).await.map_err(|e| e.to_string())?;
            for m in &list {
                users.insert(m.id, m.display_name.clone().unwrap_or_else(|| m.username.clone()));
            }
            members.insert(g.guild.id, list);
        }
        self.members.set(members);
        let current = *self.guild.peek();
        if current.is_none_or(|id| !guilds.iter().any(|g| g.guild.id == id)) {
            let remembered = self.settings.peek().last_guild.filter(|id| guilds.iter().any(|g| g.guild.id == *id));
            let guild = remembered.or_else(|| guilds.first().map(|g| g.guild.id));
            self.guild.set(guild);
            // Open its first text channel, so a restart lands in a chat.
            if !*self.home.peek() && self.channel.peek().is_none() {
                let first = guilds
                    .iter()
                    .find(|g| Some(g.guild.id) == guild)
                    .and_then(|g| g.channels.iter().find(|c| c.kind == ChannelKind::Text))
                    .map(|c| c.id);
                self.channel.set(first);
            }
            if remembered.is_none() && guilds.is_empty() {
                self.home.set(true);
            }
        }
        self.guilds.set(guilds);
        self.users.set(users);
        Ok(())
    }

    pub async fn refresh_dms(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let dms = s.api.dms().await.map_err(|e| e.to_string())?;
        let mut users = self.users.write();
        for u in dms.iter().flat_map(|d| &d.members) {
            users.insert(u.id, u.display_name.clone().unwrap_or_else(|| u.username.clone()));
        }
        drop(users);
        self.dms.set(dms);
        Ok(())
    }

    pub async fn refresh_friends(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let friends = s.api.friends().await.map_err(|e| e.to_string())?;
        let mut users = self.users.write();
        for u in friends.friends.iter().chain(&friends.incoming).chain(&friends.outgoing).chain(&friends.blocked) {
            users.insert(u.id, u.display_name.clone().unwrap_or_else(|| u.username.clone()));
        }
        drop(users);
        self.friends.set(friends);
        Ok(())
    }

    async fn refresh_presence(mut self) {
        let Some(s) = self.session.cloned() else { return };
        if let Ok(online) = s.api.presence().await {
            let mut online: HashSet<UserId> = online.into_iter().collect();
            // You are online whenever you see this.
            online.insert(s.user.id);
            self.online.set(online);
        }
    }

    /// Who is in the selected guild's voice channels.
    pub async fn refresh_voice(mut self) {
        let (Some(s), Some(guild)) = (self.session.cloned(), *self.guild.peek()) else { return };
        if *self.home.peek() {
            return;
        }
        match s.api.voice_states(guild).await {
            Ok(states) => {
                let map = states.into_iter().map(|v| (v.channel_id, v.participants)).collect();
                if *self.voice.peek() != map {
                    self.voice.set(map);
                }
            }
            Err(e) => tracing::debug!(error = %e, "voice states unavailable"),
        }
    }

    /// Loads everything and catches up every channel this device has a group for.
    async fn sync(mut self, s: &Session) -> Result<(), String> {
        self.refresh().await?;
        self.refresh_dms().await?;
        self.refresh_friends().await?;
        self.refresh_presence().await;
        for channel in chat::accept_welcomes(s).await? {
            self.groups_ready.write().insert(channel);
        }
        let channels: Vec<ChannelId> = s.mls.borrow().channels().collect();
        for channel in channels {
            self.groups_ready.write().insert(channel);
            let new = chat::catch_up(s, channel).await?;
            self.on_new_records(s, channel, new);
        }
        Ok(())
    }

    fn on_new_records(mut self, s: &Session, channel: ChannelId, records: Vec<Record>) {
        if records.is_empty() {
            return;
        }
        let from_others = records
            .iter()
            .any(|r| r.author != s.user.id && r.kind == sivryn_client_core::protocol::model::MessageKind::Message);
        if let Some(list) = self.messages.write().get_mut(&channel) {
            merge(list, records);
        }
        if from_others && *self.channel.peek() != Some(channel) {
            self.unread.write().insert(channel);
        }
    }

    /// Runs after login until logout: syncs, then pumps gateway events,
    /// reconnecting with backoff when the connection drops.
    pub async fn run_gateway(mut self) {
        let mut backoff = 1;
        loop {
            let Some(s) = self.session.cloned() else { return };
            self.connection.set(Connection::Connecting);
            if let Err(e) = self.sync(&s).await {
                tracing::warn!(error = %e, "sync failed");
            }
            match connect_gateway(&s.api, Some(s.device_id)).await {
                Ok(mut conn) => {
                    self.connection.set(Connection::Online);
                    backoff = 1;
                    while let Some(msg) = conn.events.recv().await {
                        match msg {
                            ServerMessage::Event(event) => self.handle_event(&s, event).await,
                            ServerMessage::InvalidSession { reason } => {
                                tracing::warn!(%reason, "session ended by server");
                                self.toast(t("toast.session_ended"), true);
                                return self.sign_out(false).await;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "gateway connection failed"),
            }
            self.connection.set(Connection::Offline);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    }

    /// Text channels of a guild this device has a group for.
    fn guild_group_channels(&self, s: &Session, guild: GuildId) -> Vec<ChannelId> {
        self.guilds
            .read()
            .iter()
            .filter(|g| g.guild.id == guild)
            .flat_map(|g| g.channels.iter().map(|c| c.id))
            .filter(|c| s.mls.borrow().has_group(*c))
            .collect()
    }

    async fn handle_event(mut self, s: &Session, event: Event) {
        match event {
            Event::MessageCreate { message } => {
                let channel = message.channel_id;
                if !s.mls.borrow().has_group(channel) {
                    return;
                }
                let records = chat::receive(s, channel, std::slice::from_ref(&message));
                let from_other = message.author_id != s.user.id
                    && message.kind == sivryn_client_core::protocol::model::MessageKind::Message;
                let viewing = *self.channel.peek() == Some(channel);
                self.on_new_records(s, channel, records);
                if from_other && !viewing && self.settings.peek().notifications.message_sound {
                    use_services_engine().play(Sound::Message);
                }
                // Busy DMs move to the top of the list.
                let is_dm = self.dms.peek().iter().position(|d| d.channel.id == channel);
                if let Some(i) = is_dm.filter(|i| *i > 0) {
                    let dm = self.dms.write().remove(i);
                    self.dms.write().insert(0, dm);
                }
            }
            Event::MessageDelete { channel_id, message_id } => {
                let _ = s.store.delete(message_id);
                if let Some(list) = self.messages.write().get_mut(&channel_id) {
                    list.retain(|m| m.id != message_id);
                }
            }
            Event::MlsCommit { channel_id, commit, .. } => {
                if let Some(epoch) = chat::apply_commit(s, channel_id, &commit) {
                    self.on_epoch_change(s, channel_id, epoch);
                }
            }
            Event::MlsWelcome { .. } => match chat::accept_welcomes(s).await {
                Ok(joined) => {
                    for channel in joined {
                        self.groups_ready.write().insert(channel);
                        match chat::catch_up(s, channel).await {
                            Ok(new) => self.on_new_records(s, channel, new),
                            Err(e) => self.report(Err(e)),
                        }
                    }
                }
                Err(e) => self.report(Err(e)),
            },
            Event::MemberJoin { guild_id, user } => {
                self.report(self.refresh().await);
                if user.id != s.user.id {
                    for channel in self.guild_group_channels(s, guild_id) {
                        self.report(chat::add_user(s, channel, user.id).await);
                    }
                }
            }
            Event::MemberLeave { guild_id, user_id } => {
                if user_id != s.user.id {
                    for channel in self.guild_group_channels(s, guild_id) {
                        self.report(chat::remove_users(s, channel, &[user_id]).await);
                    }
                } else if *self.guild.peek() == Some(guild_id) {
                    self.guild.set(None);
                    self.channel.set(None);
                }
                self.report(self.refresh().await);
            }
            Event::GuildCreate { .. }
            | Event::GuildUpdate { .. }
            | Event::GuildDelete { .. }
            | Event::ChannelCreate { .. }
            | Event::ChannelDelete { .. } => self.report(self.refresh().await),
            Event::DmUpdate { channel_id, added, removed } => {
                self.report(self.refresh_dms().await);
                if removed.contains(&s.user.id) {
                    s.mls.borrow_mut().remove_group(channel_id);
                    s.persist_mls();
                    return;
                }
                let others: Vec<UserId> = added.into_iter().filter(|u| *u != s.user.id).collect();
                for user in others {
                    self.report(chat::add_user(s, channel_id, user).await);
                }
                if !removed.is_empty() {
                    self.report(chat::remove_users(s, channel_id, &removed).await);
                }
            }
            Event::CallRing { channel_id, user_id } => {
                let in_this_call = self.call.peek().as_ref().is_some_and(|c| c.channel == channel_id);
                if !in_this_call && user_id != s.user.id {
                    self.report(self.refresh_dms().await);
                    self.ringing.set(Some((channel_id, user_id)));
                    call::ring(self);
                }
            }
            Event::RelationshipsChanged => {
                self.report(self.refresh_friends().await);
                // New friends share presence from now on.
                self.refresh_presence().await;
            }
            Event::PresenceUpdate { user_id, online } => {
                if online {
                    self.online.write().insert(user_id);
                } else {
                    self.online.write().remove(&user_id);
                }
            }
        }
    }

    /// A new MLS epoch means a new media key for E2EE calls.
    fn on_epoch_change(&self, s: &Session, channel: ChannelId, epoch: u64) {
        let call = self.call.read();
        let Some(call) = call.as_ref().filter(|c| c.channel == channel && c.join.e2ee) else { return };
        if let (Ok(key), Some(vc)) = (s.mls.borrow().media_key(channel), call.call.borrow().as_ref()) {
            vc.update_media_key(key, epoch);
        }
    }

    /// Called after a successful login or token restore.
    pub fn signed_in(mut self, session: Session) {
        let account = session.account();
        let instance = session.api.base_url().to_owned();
        let username = session.user.username.clone();
        self.update_settings(|s| {
            s.auto_login = Some(account);
            s.last_instance = Some(instance);
            s.last_username = Some(username);
        });
        self.session.set(Some(session));
        // Outlives the login screen, which unmounts now.
        let task = spawn_forever(self.run_gateway());
        self.gateway.set(Some(task));
    }

    /// Leaves any call, stops the gateway and returns to the login screen.
    /// `server` also ends the session on the server (explicit logout).
    pub async fn sign_out(mut self, server: bool) {
        if let Some(call) = self.call.take() {
            call::leave(call).await;
        }
        if let Some(task) = self.gateway.take() {
            task.cancel();
        }
        if let Some(s) = self.session.take() {
            if server {
                s.logout().await;
            } else {
                let _ = s.store.forget_token();
            }
        }
        self.update_settings(|s| s.auto_login = None);
        self.guilds.set(Vec::new());
        self.dms.set(Vec::new());
        self.friends.set(FriendsResponse::default());
        self.online.set(HashSet::new());
        self.users.set(HashMap::new());
        self.members.set(HashMap::new());
        self.guild.set(None);
        self.channel.set(None);
        self.messages.set(HashMap::new());
        self.composer.set(HashMap::new());
        self.groups_ready.set(HashSet::new());
        self.unread.set(HashSet::new());
        self.voice.set(HashMap::new());
        self.modal.set(None);
        self.menu.set(None);
        self.picker.set(None);
        self.viewer.set(None);
        self.ringing.set(None);
        self.settings_page.set(None);
        self.connection.set(Connection::Offline);
    }

    pub fn set_muted(mut self, muted: bool) {
        // Unmuting while deafened undeafens too, like everywhere else.
        if !muted && *self.deafened.peek() {
            self.set_deafened(false);
        }
        self.muted.set(muted);
        self.apply_mute();
        use_services_engine().play(if muted { Sound::Mute } else { Sound::Unmute });
    }

    pub fn set_deafened(mut self, deafened: bool) {
        self.deafened.set(deafened);
        use_services_engine().set_deafened(deafened);
        self.apply_mute();
        use_services_engine().play(if deafened { Sound::Deafen } else { Sound::Undeafen });
    }

    /// Deafening also mutes; others see the microphone as muted.
    fn apply_mute(&self) {
        let silenced = *self.muted.peek() || *self.deafened.peek();
        use_services_engine().set_muted(silenced);
        if let Some(call) = self.call.peek().as_ref()
            && let Some(vc) = call.call.borrow().as_ref()
        {
            vc.set_muted(silenced);
        }
    }

    /// Volumes per participant, with locally muted users at 0.
    pub fn effective_volumes(&self) -> HashMap<String, f32> {
        let s = self.settings.read();
        let mut volumes = s.volumes.clone();
        for key in &s.muted_users {
            volumes.insert(key.clone(), 0.0);
        }
        volumes
    }
}

thread_local! {
    static ENGINE: RefCell<Option<AudioEngine>> = const { RefCell::new(None) };
}

/// The audio engine, reachable outside components (set once by [`App`]).
pub fn use_services_engine() -> AudioEngine {
    ENGINE.with(|e| e.borrow().clone().expect("audio engine initialized by App"))
}

fn effective_theme(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
        Theme::System => {
            if system_prefers_light() {
                "light"
            } else {
                "dark"
            }
        }
    }
}

#[cfg(windows)]
fn system_prefers_light() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into a correctly sized buffer.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    result.is_ok() && value == 1
}

#[cfg(not(windows))]
fn system_prefers_light() -> bool {
    false
}

#[component]
pub fn App() -> Element {
    // Owned by the root scope: long-running tasks (gateway, calls) run there
    // via `spawn_forever` and use this state.
    let state = use_hook(|| {
        fn root<T: 'static>(value: T) -> Signal<T> {
            Signal::new_in_scope(value, ScopeId::ROOT)
        }
        let settings = Settings::load();
        AppState {
            session: root(None),
            restoring: root(settings.auto_login.is_some()),
            guilds: root(Vec::new()),
            dms: root(Vec::new()),
            friends: root(FriendsResponse::default()),
            online: root(HashSet::new()),
            users: root(HashMap::new()),
            members: root(HashMap::new()),
            home: root(false),
            guild: root(None),
            channel: root(None),
            messages: root(HashMap::new()),
            composer: root(HashMap::new()),
            groups_ready: root(HashSet::new()),
            unread: root(HashSet::new()),
            voice: root(HashMap::new()),
            call: root(None),
            ringing: root(None),
            muted: root(false),
            deafened: root(false),
            modal: root(None),
            menu: root(None),
            picker: root(None),
            viewer: root(None),
            drag_over: root(false),
            settings_page: root(None),
            settings: root(settings),
            connection: root(Connection::Offline),
            toasts: root(Vec::new()),
            gateway: root(None),
        }
    });
    use_context_provider(|| state);

    let services = use_hook(|| {
        let settings = state.settings.peek().clone();
        let engine = AudioEngine::new(settings.audio.clone());
        engine.set_volumes(state.effective_volumes());
        ENGINE.with(|e| *e.borrow_mut() = Some(engine.clone()));
        let (hotkeys, mut events) = Hotkeys::start(settings.keybinds.clone(), engine.clone());

        // Global mute/deafen hotkeys. Captured bindings go to the settings page
        // through the same channel.
        let mut captured = Signal::new_in_scope(None::<Option<crate::hotkeys::Keybind>>, ScopeId::ROOT);
        spawn_forever(async move {
            while let Some(event) = events.recv().await {
                match event {
                    HotkeyEvent::ToggleMute => state.set_muted(!*state.muted.peek()),
                    HotkeyEvent::ToggleDeafen => state.set_deafened(!*state.deafened.peek()),
                    HotkeyEvent::Captured(bind) => captured.set(Some(bind)),
                }
            }
        });
        provide_root_context(CapturedKey(captured));

        // Files dragged onto the window go to the open channel's message box.
        let (drop_tx, mut drops) = tokio::sync::mpsc::unbounded_channel();
        blitz_shell::file_drop::set_handler(move |event| {
            let _ = drop_tx.send(event);
        });
        spawn_forever(async move {
            while let Some(event) = drops.recv().await {
                chat_view::handle_file_drop(state, event);
            }
        });

        // Voice presence refresh.
        spawn_forever(async move {
            loop {
                state.refresh_voice().await;
                tokio::time::sleep(Duration::from_secs(8)).await;
            }
        });
        // Moves idle members to the guild's AFK channel.
        spawn_forever(call::afk_watch(state));

        // Sign in with the saved token.
        if let Some(account) = settings.auto_login.clone() {
            let mut state = state;
            spawn_forever(async move {
                match Session::restore(&account).await {
                    Ok(Some(session)) => state.signed_in(session),
                    Ok(None) => state.toast(t("toast.signed_out"), false),
                    Err(e) => state.toast(format!("{}: {e}", t("toast.restore_failed")), true),
                }
                state.restoring.set(false);
            });
        }
        Services { engine, hotkeys }
    });
    use_context_provider(|| services.clone());

    // Apply settings whenever they change.
    use_effect(move || {
        let s = state.settings.read();
        services.engine.set_config(s.audio.clone());
        services.hotkeys.set_binds(s.keybinds.clone());
        drop(s);
        services.engine.set_volumes(state.effective_volumes());
    });

    let settings = state.settings.read();
    i18n::set_language(settings.appearance.language);
    let theme = effective_theme(settings.appearance.theme);
    let lang = i18n::current().code();
    let font_size = settings.appearance.font_size.clamp(13, 18);
    let compact = if settings.appearance.compact { "compact" } else { "" };
    drop(settings);
    let dynamic_css = format!(":root {{ font-size: {font_size}px; }}");

    // Title bar follows the theme.
    let mut styled_theme = use_signal(|| None::<&'static str>);
    if *styled_theme.peek() != Some(theme) {
        styled_theme.set(Some(theme));
        spawn_forever(crate::window::style_title_bar(theme == "dark"));
    }

    rsx! {
        // Keyed by theme and language: Blitz bakes icon colors and text at build
        // time, so switching either rebuilds the tree.
        div { key: "{theme}-{lang}", class: "root theme-{theme} {compact}",
            style { {font_css()} }
            style { {STYLE} }
            style { {dynamic_css} }
            if *state.restoring.read() {
                div { class: "splash",
                    icons::Logo { size: 56 }
                    div { class: "spinner" }
                }
            } else if state.session.read().is_some() {
                shell::Shell {}
            } else {
                login::Login {}
            }
            // Overlays come last so they paint on top of everything.
            if state.viewer.read().is_some() {
                media::ImageViewer {}
            }
            if state.ringing.read().is_some() {
                call::IncomingCall {}
            }
            if state.picker.read().is_some() {
                emoji_picker::EmojiPicker {}
            }
            if state.menu.read().is_some() {
                menu::ContextMenuView {}
            }
            Toasts {}
            CaretBlink {}
        }
    }
}

/// Blitz repaints only when the DOM changes, so this toggles a hidden
/// attribute at every caret blink while a text field has focus.
#[component]
fn CaretBlink() -> Element {
    let mut tick = use_signal(|| false);
    use_future(move || async move {
        loop {
            if blitz_dom::caret::input_focused() {
                let wait = blitz_dom::caret::ms_until_toggle() + 5;
                tokio::time::sleep(Duration::from_millis(wait)).await;
                tick.toggle();
            } else {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }
    });
    rsx! { div { class: "caret-tick", "data-on": "{tick}" } }
}

/// A key captured for a binding on the settings page (`Some(None)` = Escape).
#[derive(Clone, Copy)]
pub struct CapturedKey(pub Signal<Option<Option<crate::hotkeys::Keybind>>>);

#[component]
fn Toasts() -> Element {
    let mut state = use_context::<AppState>();
    rsx! {
        div { class: "toasts",
            for toast in state.toasts.read().iter() {
                div {
                    key: "{toast.id}",
                    class: if toast.error { "toast error" } else { "toast" },
                    onclick: {
                        let id = toast.id;
                        move |_| state.toasts.write().retain(|t| t.id != id)
                    },
                    "{toast.text}"
                }
            }
        }
    }
}
