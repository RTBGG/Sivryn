//! Application state and the root component.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use dioxus_native::prelude::dioxus_core::{Task, provide_root_context, spawn_forever};
use dioxus_native::prelude::*;
use sivryn_client_core::connect_gateway;
use sivryn_client_core::protocol::api::{GuildWithChannels, VoiceJoinResponse, VoiceParticipant};
use sivryn_client_core::protocol::gateway::{Event, ServerMessage};
use sivryn_client_core::protocol::model::ChannelKind;
use sivryn_client_core::protocol::{ChannelId, GuildId, UserId};
use sivryn_media::{AudioEngine, CameraShare, ScreenAudio, ScreenShare, Sound, VideoFeed, VoiceCall};

use crate::chat::{self, ChatLine};
use crate::hotkeys::{HotkeyEvent, Hotkeys};
use crate::i18n::{self, t};
use crate::session::Session;
use crate::settings::{Settings, Theme};

mod call;
mod chat_view;
pub mod icons;
mod login;
mod settings_view;
mod shell;
pub mod widgets;

const STYLE: &str = include_str!("style.css");
const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");

/// `@font-face` for the bundled Inter font, as a data URL (built once).
fn font_css() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        let data = base64::engine::general_purpose::STANDARD.encode(INTER);
        format!(
            "@font-face {{ font-family: 'Inter'; font-weight: 100 900; font-style: normal; \
             src: url(data:font/ttf;base64,{data}) format('truetype'); }}"
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

#[derive(Clone, Copy, PartialEq)]
pub enum Modal {
    CreateGuild,
    JoinGuild,
    Invite,
    PickScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Account,
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
    pub users: Signal<HashMap<UserId, String>>,
    pub guild: Signal<Option<GuildId>>,
    pub channel: Signal<Option<ChannelId>>,
    pub messages: Signal<HashMap<ChannelId, Vec<ChatLine>>>,
    /// Channels whose MLS group this device is in.
    pub groups_ready: Signal<HashSet<ChannelId>>,
    pub unread: Signal<HashSet<ChannelId>>,
    pub voice: Signal<HashMap<ChannelId, Vec<VoiceParticipant>>>,
    pub call: Signal<Option<CallState>>,
    pub muted: Signal<bool>,
    pub deafened: Signal<bool>,
    pub modal: Signal<Option<Modal>>,
    pub settings_page: Signal<Option<SettingsPage>>,
    pub settings: Signal<Settings>,
    pub connection: Signal<Connection>,
    pub toasts: Signal<Vec<Toast>>,
    pub gateway: Signal<Option<Task>>,
}

/// Inserts lines into a channel's list, skipping ones already there (the
/// gateway and catch-up can deliver the same message) and keeping order.
pub fn merge_lines(list: &mut Vec<ChatLine>, new: impl IntoIterator<Item = ChatLine>) {
    for line in new {
        match line.id.and_then(|id| list.iter().position(|l| l.id == Some(id))) {
            // A second copy can't be decrypted again; keep the readable one.
            Some(i) if list[i].body.is_none() => list[i] = line,
            Some(_) => {}
            None => list.push(line),
        }
    }
    list.sort_by_key(|l| (l.created_at, l.id.map(|id| id.0)));
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

    pub fn username(&self, id: UserId) -> String {
        self.users.read().get(&id).cloned().unwrap_or_else(|| t("common.unknown").into())
    }

    pub fn channel_kind(&self, id: ChannelId) -> Option<ChannelKind> {
        self.guilds.read().iter().flat_map(|g| &g.channels).find(|c| c.id == id).map(|c| c.kind)
    }

    pub fn channel_name(&self, id: ChannelId) -> String {
        self.guilds
            .read()
            .iter()
            .flat_map(|g| &g.channels)
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }

    pub fn guild_of(&self, id: ChannelId) -> Option<GuildId> {
        self.guilds.read().iter().flat_map(|g| &g.channels).find(|c| c.id == id).map(|c| c.guild_id)
    }

    /// Changes settings, applies them and saves them to disk.
    pub fn update_settings(mut self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings.write());
        self.settings.peek().save();
    }

    /// Reloads guilds and member names after membership changes.
    pub async fn refresh(mut self) -> Result<(), String> {
        let Some(s) = self.session.cloned() else { return Ok(()) };
        let guilds = s.api.guilds().await.map_err(|e| e.to_string())?;
        let mut users = HashMap::from([(s.user.id, s.user.username.clone())]);
        for g in &guilds {
            for m in s.api.members(g.guild.id).await.map_err(|e| e.to_string())? {
                users.insert(m.id, m.display_name.clone().unwrap_or(m.username));
            }
        }
        let current = *self.guild.peek();
        if current.is_none_or(|id| !guilds.iter().any(|g| g.guild.id == id)) {
            let remembered = self.settings.peek().last_guild.filter(|id| guilds.iter().any(|g| g.guild.id == *id));
            self.guild.set(remembered.or_else(|| guilds.first().map(|g| g.guild.id)));
        }
        self.guilds.set(guilds);
        self.users.set(users);
        Ok(())
    }

    /// Who is in the selected guild's voice channels.
    pub async fn refresh_voice(mut self) {
        let (Some(s), Some(guild)) = (self.session.cloned(), *self.guild.peek()) else { return };
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

    /// Catches up every channel this device has a group for.
    async fn sync(mut self, s: &Session) -> Result<(), String> {
        self.refresh().await?;
        for channel in chat::accept_welcomes(s).await? {
            self.groups_ready.write().insert(channel);
        }
        let channels: Vec<ChannelId> = s.mls.borrow().channels().collect();
        for channel in channels {
            self.groups_ready.write().insert(channel);
            let new = chat::catch_up(s, channel).await?;
            self.on_new_lines(s, channel, new);
        }
        Ok(())
    }

    fn on_new_lines(mut self, s: &Session, channel: ChannelId, lines: Vec<ChatLine>) {
        if lines.is_empty() {
            return;
        }
        let from_others = lines.iter().any(|l| l.author != s.user.id);
        if let Some(list) = self.messages.write().get_mut(&channel) {
            merge_lines(list, lines);
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

    async fn handle_event(mut self, s: &Session, event: Event) {
        match event {
            Event::MessageCreate { message, .. } => {
                let channel = message.channel_id;
                if !s.mls.borrow().has_group(channel) {
                    return;
                }
                let lines = chat::receive(s, channel, std::slice::from_ref(&message));
                let from_other = message.author_id != s.user.id;
                let viewing = *self.channel.peek() == Some(channel);
                self.on_new_lines(s, channel, lines);
                if from_other && !viewing && self.settings.peek().notifications.message_sound {
                    use_services_engine().play(Sound::Message);
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
                            Ok(new) => self.on_new_lines(s, channel, new),
                            Err(e) => self.report(Err(e)),
                        }
                    }
                }
                Err(e) => self.report(Err(e)),
            },
            Event::MemberJoin { guild_id, user } => {
                if user.id != s.user.id {
                    let channels: Vec<ChannelId> = self
                        .guilds
                        .read()
                        .iter()
                        .filter(|g| g.guild.id == guild_id)
                        .flat_map(|g| g.channels.iter().map(|c| c.id))
                        .collect();
                    for channel in channels {
                        self.report(chat::add_user(s, channel, user.id).await);
                    }
                }
                self.report(self.refresh().await);
            }
            Event::GuildCreate { .. }
            | Event::GuildDelete { .. }
            | Event::ChannelCreate { .. }
            | Event::ChannelDelete { .. }
            | Event::MemberLeave { .. } => self.report(self.refresh().await),
            _ => {}
        }
    }

    /// A new MLS epoch means a new media key for E2EE voice channels.
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
        self.users.set(HashMap::new());
        self.guild.set(None);
        self.channel.set(None);
        self.messages.set(HashMap::new());
        self.groups_ready.set(HashSet::new());
        self.unread.set(HashSet::new());
        self.voice.set(HashMap::new());
        self.modal.set(None);
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
            users: root(HashMap::new()),
            guild: root(None),
            channel: root(None),
            messages: root(HashMap::new()),
            groups_ready: root(HashSet::new()),
            unread: root(HashSet::new()),
            voice: root(HashMap::new()),
            call: root(None),
            muted: root(false),
            deafened: root(false),
            modal: root(None),
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
        engine.set_volumes(settings.volumes.clone());
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

        // Voice presence refresh.
        spawn_forever(async move {
            loop {
                state.refresh_voice().await;
                tokio::time::sleep(Duration::from_secs(8)).await;
            }
        });

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
        services.engine.set_volumes(s.volumes.clone());
        services.hotkeys.set_binds(s.keybinds.clone());
    });

    let settings = state.settings.read();
    i18n::set_language(settings.appearance.language);
    let theme = effective_theme(settings.appearance.theme);
    let lang = i18n::current().code();
    let font_size = settings.appearance.font_size.clamp(13, 18);
    let compact = if settings.appearance.compact { "compact" } else { "" };
    drop(settings);
    let dynamic_css = format!(":root {{ font-size: {font_size}px; }}");

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
            Toasts {}
        }
    }
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
