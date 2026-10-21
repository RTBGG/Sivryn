//! Application state and the root component.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dioxus_native::prelude::dioxus_core::{Task, provide_root_context, spawn_forever};
use dioxus_native::prelude::*;
use sivryn_client_core::connect_gateway;
use sivryn_client_core::protocol::api::{
    DmChannel, FriendsResponse, GuildMember, GuildWithChannels, MessageQuery, VoiceJoinResponse, VoiceParticipant,
};
use sivryn_client_core::protocol::gateway::{Event, ServerMessage};
use sivryn_client_core::protocol::model::{
    Channel, ChannelKind, Guild, MessageKind, NotifyLevel, Role, User, UserStatus,
};
use sivryn_client_core::protocol::permissions::Permissions;
use sivryn_client_core::protocol::{CategoryId, ChannelId, FileId, GuildId, MessageId, RoleId, UserId};
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
mod channels;
mod chat_view;
mod crop;
mod dnd;
mod emoji_picker;
mod fullscreen;
mod guild_settings;
mod home;
pub mod icons;
mod login;
mod media;
mod members;
mod menu;
mod place_settings;
mod profile;
mod resize;
mod roles;
mod settings_view;
mod shell;
mod updates;
pub mod widgets;

pub use menu::{ContextMenu, MenuItem};

const STYLE: &str = include_str!("style.css");
const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");
const TWEMOJI: &[u8] = include_bytes!("../../assets/fonts/TwemojiMozilla.ttf");

/// Makes the bundled fonts available to the UI. Blitz ignores `@font-face`
/// in inline styles, so they are registered directly; CSS refers to them by
/// their own family names, "Inter" and "Twemoji Mozilla". Call before launch.
pub fn register_fonts() {
    blitz_dom::bundled_fonts::register(INTER);
    blitz_dom::bundled_fonts::register(TWEMOJI);
}

/// Someone in the call, as the UI shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Participant {
    pub identity: String,
    pub name: String,
    pub muted: bool,
    /// Turned off their sound.
    pub deafened: bool,
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
    /// Shown large: a screen share or a participant. `None` shows the grid,
    /// or the first screen share while `auto_focus` is on.
    pub focus: Option<Focus>,
    pub auto_focus: bool,
    /// Hide the small tiles below the focused one.
    pub strip_hidden: bool,
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
    CreateChannel(GuildId, Option<CategoryId>, ChannelKind),
    /// Create (`None`) or edit a category.
    Category(GuildId, Option<CategoryId>),
    ChannelSettings(ChannelId),
    NotifySettings(GuildId),
    /// "Leave the current voice channel for this one?"
    SwitchVoice(ChannelId),
    /// "Join this voice channel?" (from a mention in the chat).
    JoinVoice(ChannelId),
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
    /// Mentions picked from the suggestions: the `@Name` shown, the token sent.
    pub mentions: Vec<crate::mentions::Picked>,
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
    /// Scale relative to the image's own size; `None` fits the window.
    pub zoom: Option<f64>,
    /// The attachment it comes from, for copying and saving.
    pub attachment: Option<sivryn_client_core::protocol::model::Attachment>,
}

impl PartialEq for ViewerImage {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.width == other.width && self.height == other.height && self.zoom == other.zoom
    }
}

/// Set while an overlay is open that Escape closes.
static ESCAPABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Closes fullscreen, the image viewer, a menu, a dialog or the settings, in that order.
fn close_topmost(mut state: AppState) {
    if state.full_view.peek().is_some() {
        fullscreen::close(state);
    } else if state.viewer.peek().is_some() {
        state.viewer.set(None);
    } else if state.menu.peek().is_some() {
        state.menu.set(None);
    } else if state.modal.peek().is_some() {
        state.modal.set(None);
    } else if state.settings_page.peek().is_some() {
        state.settings_page.set(None);
    }
}

/// What a call shows large.
#[derive(Clone, Debug, PartialEq)]
pub enum Focus {
    /// A screen share, by track.
    Screen(String),
    /// A participant (their camera, or their avatar), by identity.
    User(String),
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
    /// Status of everyone online that this user can see (offline = absent).
    pub online: Signal<HashMap<UserId, UserStatus>>,
    /// Display names (or account names) of known users.
    pub users: Signal<HashMap<UserId, String>>,
    pub avatars: Signal<HashMap<UserId, FileId>>,
    /// Names members chose for themselves in a guild.
    pub nicks: Signal<HashMap<(GuildId, UserId), String>>,
    /// Members of each guild.
    pub members: Signal<HashMap<GuildId, Vec<GuildMember>>>,
    /// Your own status (online, away, busy, invisible).
    pub my_status: Signal<UserStatus>,
    /// The home view (friends and DMs) instead of a guild.
    pub home: Signal<bool>,
    pub guild: Signal<Option<GuildId>>,
    pub channel: Signal<Option<ChannelId>>,
    pub messages: Signal<HashMap<ChannelId, Vec<ChatMessage>>>,
    pub composer: Signal<HashMap<ChannelId, Composer>>,
    /// Channels whose MLS group this device is in.
    pub groups_ready: Signal<HashSet<ChannelId>>,
    /// New messages per channel since you last read it.
    pub unread: Signal<HashMap<ChannelId, u32>>,
    /// The newest message acknowledged per channel (so it's sent once).
    pub acked: Signal<HashMap<ChannelId, MessageId>>,
    /// Muted channels: until when (`None` = until unmuted).
    pub mutes: Signal<HashMap<ChannelId, Option<DateTime<Utc>>>>,
    /// Show the chat of the selected voice channel instead of the call.
    pub voice_chat: Signal<bool>,
    pub voice: Signal<HashMap<ChannelId, Vec<VoiceParticipant>>>,
    pub call: Signal<Option<CallState>>,
    /// The voice channel being connected to.
    pub connecting: Signal<Option<ChannelId>>,
    /// A channel, category or voice member being dragged.
    pub drag: Signal<Option<dnd::Drag>>,
    /// The video that has a player (only one at a time, to save memory).
    pub active_video: Signal<u64>,
    /// A video or stream shown over the whole screen.
    pub full_view: Signal<Option<fullscreen::FullView>>,
    /// Looking for, downloading or holding an update.
    pub update: Signal<updates::UpdateState>,
    /// A picked image being cropped (avatar, banner, server icon).
    pub crop: Signal<Option<crop::CropRequest>>,
    /// Bumped when the own profile images changed, so editors reload them.
    pub profile_rev: Signal<u32>,
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

    /// The name to show in a guild: the member's name there, else their own.
    pub fn display_name_in(&self, guild: Option<GuildId>, id: UserId) -> String {
        guild.and_then(|g| self.nicks.read().get(&(g, id)).cloned()).unwrap_or_else(|| self.username(id))
    }

    /// How a mention reads: `@Name` (the name in the guild) or `#channel`.
    pub fn mention_label(&self, guild: Option<GuildId>, token: crate::mentions::Token) -> String {
        match token {
            crate::mentions::Token::User(id) => format!("@{}", self.display_name_in(guild, id)),
            crate::mentions::Token::Channel(id) => match self.find_channel(id) {
                Some(c) => format!("#{}", c.name),
                None => format!("#{}", t("chat.unknown_channel")),
            },
        }
    }

    /// Remembers a user's name and picture.
    pub fn learn_user(mut self, u: &User) {
        self.users.write().insert(u.id, u.name().to_owned());
        let mut avatars = self.avatars.write();
        match u.avatar_id {
            Some(file) => avatars.insert(u.id, file),
            None => avatars.remove(&u.id),
        };
    }

    pub fn status_of(&self, id: UserId) -> Option<UserStatus> {
        self.online.read().get(&id).copied()
    }

    pub fn is_muted(&self, channel: ChannelId) -> bool {
        self.mutes.read().get(&channel).is_some_and(|until| until.is_none_or(|t| t > Utc::now()))
    }

    /// New messages to show for a channel (none while muted).
    pub fn unread_count(&self, channel: ChannelId) -> u32 {
        if self.is_muted(channel) {
            return 0;
        }
        self.unread.read().get(&channel).copied().unwrap_or(0)
    }

    pub fn guild_unread(&self, guild: GuildId) -> u32 {
        let channels: Vec<ChannelId> = self
            .guilds
            .read()
            .iter()
            .filter(|g| g.guild.id == guild)
            .flat_map(|g| g.channels.iter().map(|c| c.id))
            .collect();
        channels.into_iter().map(|c| self.unread_count(c)).sum()
    }

    /// Unread DMs and groups plus incoming friend requests (on the logo).
    pub fn home_unread(&self) -> u32 {
        let dms: Vec<ChannelId> = self.dms.read().iter().map(|d| d.channel.id).collect();
        let messages: u32 = dms.into_iter().map(|c| self.unread_count(c)).sum();
        messages + self.friends.read().incoming.len() as u32
    }

    /// Marks a channel read here and on the server (up to its newest message).
    pub fn mark_read(mut self, channel: ChannelId) {
        self.unread.write().remove(&channel);
        let Some(s) = self.session.cloned() else { return };
        let newest = self.messages.peek().get(&channel).and_then(|l| l.last().map(|m| m.id));
        spawn_forever(async move {
            let newest = match newest {
                Some(id) => Some(id),
                // Not loaded here: ask the server for the newest message.
                None => {
                    let query = MessageQuery { limit: Some(1), ..Default::default() };
                    s.api.messages(channel, &query).await.ok().and_then(|m| m.first().map(|m| m.id))
                }
            };
            if let Some(id) = newest {
                self.ack(&s, channel, id).await;
            }
        });
    }

    /// Tells the server (once per message) how far a channel has been read.
    async fn ack(mut self, s: &Session, channel: ChannelId, message: MessageId) {
        if self.acked.peek().get(&channel).is_some_and(|a| *a >= message) {
            return;
        }
        self.acked.write().insert(channel, message);
        if let Err(e) = s.api.ack(channel, message).await {
            tracing::debug!(error = %e, "could not mark channel read");
        }
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

    /// My permissions in one channel (after its overwrites). DMs and groups
    /// allow everything except managing.
    pub fn channel_permissions(&self, channel: ChannelId) -> Permissions {
        let guilds = self.guilds.read();
        match guilds.iter().find(|g| g.channels.iter().any(|c| c.id == channel)) {
            Some(g) => g.channel_permissions.get(&channel).copied().unwrap_or(g.permissions),
            None => {
                Permissions::VIEW_CHANNEL
                    | Permissions::SEND_MESSAGES
                    | Permissions::ATTACH_FILES
                    | Permissions::CONNECT
                    | Permissions::SPEAK
                    | Permissions::VIDEO
                    | Permissions::SCREEN_SHARE
            }
        }
    }

    /// A guild's roles, highest first (`@everyone` last).
    pub fn roles(&self, guild: GuildId) -> Vec<Role> {
        self.guilds.read().iter().find(|g| g.guild.id == guild).map(|g| g.roles.clone()).unwrap_or_default()
    }

    /// The roles a member has (besides `@everyone`).
    pub fn member_roles(&self, guild: GuildId, user: UserId) -> Vec<RoleId> {
        self.members
            .read()
            .get(&guild)
            .and_then(|l| l.iter().find(|m| m.user.id == user).map(|m| m.roles.clone()))
            .unwrap_or_default()
    }

    /// The color of a member's highest colored role.
    pub fn role_color(&self, guild: Option<GuildId>, user: UserId) -> Option<u32> {
        let guild = guild?;
        let mine = self.member_roles(guild, user);
        self.roles(guild).into_iter().filter(|r| mine.contains(&r.id)).find_map(|r| r.color)
    }

    pub fn is_owner(&self, guild: GuildId, user: UserId) -> bool {
        self.guild(guild).is_some_and(|g| g.owner_id == user)
    }

    /// Position of a member's highest role (0 with only `@everyone`).
    pub fn top_position(&self, guild: GuildId, user: UserId) -> i32 {
        let mine = self.member_roles(guild, user);
        self.roles(guild).iter().filter(|r| mine.contains(&r.id)).map(|r| r.position).max().unwrap_or(0)
    }

    /// Whether I may manage a role at `position` (or a member whose highest
    /// role is there): only below my own highest role, unless I own the guild.
    pub fn outranks(&self, guild: GuildId, position: i32) -> bool {
        let Some(me) = self.me() else { return false };
        self.is_owner(guild, me) || self.top_position(guild, me) > position
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
        self.voice_chat.set(false);
        self.mark_read(channel);
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
            self.learn_user(u);
        }
        let mut dms = self.dms.write();
        dms.retain(|d| d.channel.id != dm.channel.id);
        dms.insert(0, dm);
    }

    /// Reloads guilds and member names after membership changes.
    pub async fn refresh(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let mut guilds = s.api.guilds().await.map_err(|e| e.to_string())?;
        let mut members = HashMap::new();
        let mut nicks = HashMap::new();
        self.learn_user(&s.user);
        let mut gone = Vec::new();
        for g in &guilds {
            let list = match s.api.members(g.guild.id).await {
                Ok(list) => list,
                // Deleted while we were loading (its delete event goes out
                // just before the guild disappears).
                Err(e) if e.is_not_found() => {
                    gone.push(g.guild.id);
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            };
            for m in &list {
                self.learn_user(&m.user);
                if let Some(nick) = &m.nick {
                    nicks.insert((g.guild.id, m.user.id), nick.clone());
                }
            }
            members.insert(g.guild.id, list);
        }
        guilds.retain(|g| !gone.contains(&g.guild.id));
        self.members.set(members);
        self.nicks.set(nicks);
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
        Ok(())
    }

    pub async fn refresh_dms(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let dms = s.api.dms().await.map_err(|e| e.to_string())?;
        for u in dms.iter().flat_map(|d| &d.members) {
            self.learn_user(u);
        }
        self.dms.set(dms);
        Ok(())
    }

    pub async fn refresh_friends(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let friends = s.api.friends().await.map_err(|e| e.to_string())?;
        for u in friends.friends.iter().chain(&friends.incoming).chain(&friends.outgoing).chain(&friends.blocked) {
            self.learn_user(u);
        }
        self.friends.set(friends);
        Ok(())
    }

    async fn refresh_presence(mut self) {
        let Some(s) = self.session.cloned() else { return };
        if let Ok(list) = s.api.presence().await {
            let online: HashMap<UserId, UserStatus> = list.into_iter().map(|p| (p.user_id, p.status)).collect();
            if let Some(mine) = online.get(&s.user.id) {
                self.my_status.set(*mine);
            }
            self.online.set(online);
        }
    }

    /// Unread counts from the server (what happened while offline).
    async fn refresh_read_states(mut self) {
        let Some(s) = self.session.cloned() else { return };
        match s.api.read_states().await {
            Ok(states) => {
                let viewing = *self.channel.peek();
                let unread = states
                    .iter()
                    .filter(|r| r.unread > 0 && Some(r.channel_id) != viewing)
                    .map(|r| (r.channel_id, r.unread))
                    .collect();
                let acked = states.iter().filter_map(|r| r.last_read_id.map(|m| (r.channel_id, m))).collect();
                self.unread.set(unread);
                self.acked.set(acked);
            }
            Err(e) => tracing::debug!(error = %e, "read states unavailable"),
        }
    }

    async fn refresh_mutes(mut self) {
        let Some(s) = self.session.cloned() else { return };
        if let Ok(mutes) = s.api.mutes().await {
            self.mutes.set(mutes.into_iter().map(|m| (m.channel_id, m.until)).collect());
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
        self.refresh_read_states().await;
        self.refresh_mutes().await;
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
        let from_others = records.iter().filter(|r| r.author != s.user.id && r.kind == MessageKind::Message).count();
        let newest = records.iter().filter(|r| r.kind == MessageKind::Message).map(|r| r.id).max();
        if let Some(list) = self.messages.write().get_mut(&channel) {
            merge(list, records);
        }
        let viewing = *self.channel.peek() == Some(channel) && crate::window::is_focused();
        if viewing {
            if let Some(id) = newest {
                let s = s.clone();
                spawn_forever(async move { self.ack(&s, channel, id).await });
            }
        } else if from_others > 0 {
            *self.unread.write().entry(channel).or_default() += from_others as u32;
        }
    }

    /// Shows a Windows notification for a new message, unless it shouldn't.
    fn maybe_notify(&self, s: &Session, channel: ChannelId, record: &Record) {
        if record.author == s.user.id || record.kind != MessageKind::Message {
            return;
        }
        if !self.settings.peek().notifications.desktop || *self.my_status.peek() == UserStatus::Dnd {
            return;
        }
        if self.is_muted(channel) || (*self.channel.peek() == Some(channel) && crate::window::is_focused()) {
            return;
        }
        let body = record.content.as_ref().map(|c| c.body.trim().to_owned()).unwrap_or_default();
        let guild = self.guild_of(channel);
        if let Some(guild) = guild {
            let level =
                self.guilds.peek().iter().find(|g| g.guild.id == guild).map(|g| g.notify_level).unwrap_or_default();
            let mentioned = || {
                if body.contains(&crate::mentions::Token::User(s.user.id).encode()) {
                    return true;
                }
                let lower = body.to_lowercase();
                let mut names = vec![s.user.username.clone(), self.username(s.user.id)];
                names.extend(self.nicks.peek().get(&(guild, s.user.id)).cloned());
                names.iter().any(|n| lower.contains(&format!("@{}", n.to_lowercase())))
            };
            match level {
                NotifyLevel::All => {}
                NotifyLevel::Mentions if mentioned() => {}
                _ => return,
            }
        }
        let author = self.display_name_in(guild, record.author);
        let title = match guild.and_then(|g| self.guild(g)) {
            Some(g) => format!("{author} (#{}, {})", self.channel_name(channel), g.name),
            None if self.channel_kind(channel) == Some(ChannelKind::Group) => {
                format!("{author} ({})", self.channel_name(channel))
            }
            None => author,
        };
        let attachments = record.content.as_ref().map(|c| c.attachments.len()).unwrap_or(0);
        let text = if !body.is_empty() {
            crate::mentions::plain(&body, |t| self.mention_label(guild, t)).chars().take(200).collect::<String>()
        } else if attachments > 0 {
            format!("📎 {attachments} {}", t("notify.files"))
        } else {
            t("notify.new_message").to_string()
        };
        crate::notify::show(&title, &text, channel);
        crate::window::flash();
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
                let from_other = message.author_id != s.user.id && message.kind == MessageKind::Message;
                let viewing = *self.channel.peek() == Some(channel);
                for record in &records {
                    self.maybe_notify(s, channel, record);
                }
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
                    // Only channels they may read: private ones stay closed.
                    for channel in self.guild_group_channels(s, guild_id) {
                        self.report(chat::sync_members(s, channel).await);
                    }
                }
            }
            Event::ChannelAccessChanged { guild_id } => {
                // The guild was reloaded by the GuildUpdate sent just before.
                for channel in self.guild_group_channels(s, guild_id) {
                    self.report(chat::sync_members(s, channel).await);
                }
            }
            Event::VoiceMove { channel_id } => {
                if self.call.peek().as_ref().is_some_and(|c| c.channel != channel_id) {
                    call::start(self, channel_id, false);
                    if *self.channel.peek() == self.call.peek().as_ref().map(|c| c.channel) {
                        self.open_channel(channel_id);
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
            Event::GuildUpdate { guild } => {
                self.apply_bitrate(&guild).await;
                self.report(self.refresh().await);
            }
            Event::GuildCreate { .. } | Event::GuildDelete { .. } | Event::ChannelCreate { .. } => {
                self.report(self.refresh().await)
            }
            Event::ChannelDelete { channel_id, .. } => {
                if *self.channel.peek() == Some(channel_id) {
                    self.channel.set(None);
                }
                self.report(self.refresh().await)
            }
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
            Event::PresenceUpdate { user_id, status } => {
                if user_id == s.user.id
                    && let Some(status) = status
                {
                    self.my_status.set(status);
                }
                match status {
                    Some(status) => self.online.write().insert(user_id, status),
                    None => self.online.write().remove(&user_id),
                };
            }
            Event::UserUpdate { user } => {
                self.learn_user(&user);
                let mut dms = self.dms.write();
                for m in dms.iter_mut().flat_map(|d| d.members.iter_mut()).filter(|m| m.id == user.id) {
                    *m = user.clone();
                }
                drop(dms);
                let mut members = self.members.write();
                for m in members.values_mut().flat_map(|l| l.iter_mut()).filter(|m| m.user.id == user.id) {
                    m.user = user.clone();
                }
            }
            Event::MemberUpdate { guild_id, user_id, nick } => {
                match &nick {
                    Some(n) => self.nicks.write().insert((guild_id, user_id), n.clone()),
                    None => self.nicks.write().remove(&(guild_id, user_id)),
                };
                if let Some(list) = self.members.write().get_mut(&guild_id) {
                    for m in list.iter_mut().filter(|m| m.user.id == user_id) {
                        m.nick = nick.clone();
                    }
                }
                if user_id == s.user.id {
                    self.report(self.refresh().await);
                }
            }
            Event::ReadStateUpdate { channel_id, last_read_id } => {
                // Read on another device.
                self.acked.write().insert(channel_id, last_read_id);
                self.unread.write().remove(&channel_id);
            }
            Event::NotificationSettingsChanged => {
                self.refresh_mutes().await;
                self.report(self.refresh().await);
            }
        }
    }

    /// A changed guild bitrate applies to an ongoing call in that guild.
    async fn apply_bitrate(&self, guild: &Guild) {
        let call = self.call.peek().clone();
        let Some(call) = call.filter(|c| self.guild_of(c.channel) == Some(guild.id)) else { return };
        let vc = call.call.borrow_mut().take();
        if let Some(mut vc) = vc {
            if let Err(e) = vc.set_audio_bitrate(guild.audio_bitrate).await {
                tracing::warn!(error = %e, "could not change the voice bitrate");
            }
            *call.call.borrow_mut() = Some(vc);
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
        self.online.set(HashMap::new());
        self.users.set(HashMap::new());
        self.avatars.set(HashMap::new());
        self.nicks.set(HashMap::new());
        self.members.set(HashMap::new());
        self.acked.set(HashMap::new());
        self.mutes.set(HashMap::new());
        self.my_status.set(UserStatus::Online);
        self.guild.set(None);
        self.channel.set(None);
        self.messages.set(HashMap::new());
        self.composer.set(HashMap::new());
        self.groups_ready.set(HashSet::new());
        self.unread.set(HashMap::new());
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
        if let Some(call) = self.call.peek().as_ref()
            && let Some(vc) = call.call.borrow().as_ref()
        {
            vc.set_deafened(deafened);
        }
        if let Some(c) = self.call.write().as_mut() {
            let me = c.identity.clone();
            if let Some(p) = c.participants.iter_mut().find(|p| p.identity == me) {
                p.deafened = deafened;
            }
        }
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
            online: root(HashMap::new()),
            users: root(HashMap::new()),
            avatars: root(HashMap::new()),
            nicks: root(HashMap::new()),
            members: root(HashMap::new()),
            my_status: root(UserStatus::Online),
            home: root(false),
            guild: root(None),
            channel: root(None),
            messages: root(HashMap::new()),
            composer: root(HashMap::new()),
            groups_ready: root(HashSet::new()),
            unread: root(HashMap::new()),
            acked: root(HashMap::new()),
            mutes: root(HashMap::new()),
            voice_chat: root(false),
            voice: root(HashMap::new()),
            call: root(None),
            connecting: root(None),
            drag: root(None),
            crop: root(None),
            active_video: root(0),
            full_view: root(None),
            update: root(Default::default()),
            profile_rev: root(0),
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

        // Escape closes the topmost overlay, even when nothing in it has focus.
        let (esc_tx, mut escapes) = tokio::sync::mpsc::unbounded_channel();
        blitz_shell::escape::set_escape_handler(move || {
            ESCAPABLE.load(std::sync::atomic::Ordering::Relaxed)
                && !settings_view::RECORDING_KEY.load(std::sync::atomic::Ordering::Relaxed)
                && esc_tx.send(()).is_ok()
        });
        spawn_forever(async move {
            while escapes.recv().await.is_some() {
                close_topmost(state);
            }
        });

        // Keep the file cache on disk within its size limit.
        spawn_forever(async {
            let _ = tokio::task::spawn_blocking(crate::files::prune_disk_cache).await;
        });
        // Clicking a notification opens its channel.
        let mut clicks = crate::notify::init();
        spawn_forever(async move {
            while let Some(channel) = clicks.recv().await {
                crate::window::bring_to_front();
                state.open_channel(channel);
            }
        });
        // Ctrl+wheel zooms the image viewer.
        let (zoom_tx, mut zooms) = tokio::sync::mpsc::unbounded_channel();
        blitz_shell::scrolling::set_ctrl_wheel_handler(move |notches| {
            media::VIEWER_OPEN.load(std::sync::atomic::Ordering::Relaxed) && zoom_tx.send(notches).is_ok()
        });
        spawn_forever(async move {
            while let Some(notches) = zooms.recv().await {
                media::wheel_zoom(state, notches);
            }
        });
        // Reading a channel while the window was in the background: catch up
        // the read state once it gets focus again.
        spawn_forever(async move {
            let mut focused = false;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let now = crate::window::is_focused();
                if now
                    && !focused
                    && let Some(channel) = *state.channel.peek()
                    && state.unread.peek().contains_key(&channel)
                {
                    state.mark_read(channel);
                }
                focused = now;
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

    // Whether Escape has something to close (read by the window's key handler).
    use_effect(move || {
        let open = state.full_view.read().is_some()
            || state.viewer.read().is_some()
            || state.menu.read().is_some()
            || state.modal.read().is_some()
            || state.settings_page.read().is_some();
        ESCAPABLE.store(open, std::sync::atomic::Ordering::Relaxed);
    });

    use_hook(move || updates::start(state));

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
            if state.full_view.read().is_some() {
                fullscreen::FullscreenView {}
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
