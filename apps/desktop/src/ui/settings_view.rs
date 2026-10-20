//! Full-screen settings with a section list on the left.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::media::{ContentHint, VideoCodec};
use sivryn_media::audio::{input_devices, output_devices};
use sivryn_media::codec::HardwareEncoders;
use sivryn_media::{CameraQuality, NoiseSuppression, VoiceMode, list_cameras, media_foundation_available};

use super::icons::{Icon, Svg};
use super::widgets::{Avatar, IconButton, Segmented, Select, Slider, Toggle, opt};
use super::{AppState, CapturedKey, Services, SettingsPage};
use crate::hotkeys::Keybind;
use crate::i18n::t;
use crate::settings::{Keybinds, Language, ScreenQuality, Theme};

const PAGES: [(SettingsPage, Icon, &str); 8] = [
    (SettingsPage::Account, Icon::User, "settings.account"),
    (SettingsPage::Privacy, Icon::Lock, "settings.privacy"),
    (SettingsPage::Voice, Icon::Mic, "settings.voice"),
    (SettingsPage::Video, Icon::Video, "settings.video"),
    (SettingsPage::Keybinds, Icon::Keyboard, "settings.keybinds"),
    (SettingsPage::Appearance, Icon::Palette, "settings.appearance"),
    (SettingsPage::Notifications, Icon::Bell, "settings.notifications"),
    (SettingsPage::About, Icon::Info, "settings.about"),
];

#[component]
pub fn SettingsView() -> Element {
    let mut state = use_context::<AppState>();
    let Some(page) = *state.settings_page.read() else { return rsx! {} };

    rsx! {
        div { class: "settings",
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    state.settings_page.set(None);
                }
            },
            nav { class: "settings-nav",
                div { class: "settings-nav-title", {t("settings.title")} }
                for (p, icon, key) in PAGES {
                    div {
                        key: "{key}",
                        class: if p == page { "nav-item active" } else { "nav-item" },
                        onclick: move |_| state.settings_page.set(Some(p)),
                        Svg { icon, size: 18 }
                        span { {t(key)} }
                    }
                }
            }
            div { class: "settings-body",
                div { class: "settings-content",
                    match page {
                        SettingsPage::Account => rsx! { AccountPage {} },
                        SettingsPage::Privacy => rsx! { PrivacyPage {} },
                        SettingsPage::Voice => rsx! { VoicePage {} },
                        SettingsPage::Video => rsx! { VideoPage {} },
                        SettingsPage::Keybinds => rsx! { KeybindsPage {} },
                        SettingsPage::Appearance => rsx! { AppearancePage {} },
                        SettingsPage::Notifications => rsx! { NotificationsPage {} },
                        SettingsPage::About => rsx! { AboutPage {} },
                    }
                }
                div { class: "settings-close",
                    IconButton { icon: Icon::X, class: "outlined", onclick: move |_| state.settings_page.set(None) }
                    span { "ESC" }
                }
            }
        }
    }
}

#[component]
fn Field(label: String, #[props(default)] hint: Option<String>, children: Element) -> Element {
    rsx! {
        div { class: "field",
            div { class: "field-label", "{label}" }
            {children}
            if let Some(hint) = hint {
                div { class: "hint", "{hint}" }
            }
        }
    }
}

#[component]
fn ToggleRow(label: String, #[props(default)] hint: Option<String>, on: bool, onchange: EventHandler<bool>) -> Element {
    rsx! {
        div { class: "toggle-row",
            div {
                div { class: "toggle-label", "{label}" }
                if let Some(hint) = hint {
                    div { class: "hint", "{hint}" }
                }
            }
            Toggle { on, onchange }
        }
    }
}

#[component]
fn AccountPage() -> Element {
    let state = use_context::<AppState>();
    let session = state.session.read();
    let Some(s) = session.as_ref() else { return rsx! {} };
    let name = s.user.display_name.clone().unwrap_or_else(|| s.user.username.clone());
    let (username, id, instance, device) =
        (s.user.username.clone(), s.user.id, s.api.base_url().to_owned(), s.device_id);
    drop(session);

    rsx! {
        h1 { class: "section-title", {t("settings.account")} }
        div { class: "account-card",
            Avatar { name: name.clone(), id: Some(id), size: 64 }
            div {
                div { class: "account-name", "{name}" }
                div { class: "muted", "@{username}" }
            }
        }
        div { class: "info-grid",
            div { class: "info-label", {t("settings.instance")} }
            div { "{instance}" }
            div { class: "info-label", {t("settings.device")} }
            div { class: "mono", "{device}" }
        }
        div { class: "callout",
            Svg { icon: Icon::ShieldCheck, size: 18 }
            span { {t("settings.device_hint")} }
        }
        button {
            class: "button danger",
            onclick: move |_| {
                spawn_forever(state.sign_out(true));
            },
            Svg { icon: Icon::LogOut, size: 16 }
            {t("settings.logout")}
        }
    }
}

#[component]
fn VoicePage() -> Element {
    let state = use_context::<AppState>();
    let services = use_context::<Services>();
    let inputs = use_hook(input_devices);
    let outputs = use_hook(output_devices);
    let audio = state.settings.read().audio.clone();
    let mut testing = use_signal(|| false);
    let mut level = use_signal(|| -100.0f32);
    let mut transmitting = use_signal(|| false);

    // Live meter while this page is open.
    let engine = services.engine.clone();
    use_future(move || {
        let engine = engine.clone();
        async move {
            loop {
                level.set(engine.level_db());
                transmitting.set(engine.transmitting());
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    });
    let engine = services.engine.clone();
    use_drop(move || engine.set_mic_test(false));

    let mut input_options = vec![opt("", t("settings.system_default"))];
    input_options.extend(inputs.iter().map(|d| opt(d.id.clone(), d.name.clone())));
    let mut output_options = vec![opt("", t("settings.system_default"))];
    output_options.extend(outputs.iter().map(|d| opt(d.id.clone(), d.name.clone())));

    let ns = match audio.noise_suppression {
        NoiseSuppression::DeepFilter => "deepfilter",
        NoiseSuppression::RnNoise => "rnnoise",
        NoiseSuppression::WebRtc => "webrtc",
        NoiseSuppression::Off => "off",
    };
    let ns_hint = match audio.noise_suppression {
        NoiseSuppression::DeepFilter => t("voice.ns_deepfilter_hint"),
        NoiseSuppression::RnNoise => t("voice.ns_rnnoise_hint"),
        NoiseSuppression::WebRtc => t("voice.ns_webrtc_hint"),
        NoiseSuppression::Off => t("voice.ns_off_hint"),
    };
    let mode = if audio.voice_mode == VoiceMode::PushToTalk { "ptt" } else { "vad" };
    let ptt_key = state.settings.read().keybinds.push_to_talk;
    let current_level = *level.read();
    let meter = ((current_level + 80.0) / 80.0).clamp(0.0, 1.0) * 100.0;
    let test_engine = services.engine.clone();

    rsx! {
        h1 { class: "section-title", {t("settings.voice")} }
        div { class: "two-col",
            Field { label: t("voice.input_device").to_string(),
                Select {
                    value: audio.input_device.clone().unwrap_or_default(),
                    options: input_options,
                    onchange: move |v: String| state.update_settings(|s| s.audio.input_device = Some(v).filter(|v| !v.is_empty())),
                }
            }
            Field { label: t("voice.output_device").to_string(),
                Select {
                    value: audio.output_device.clone().unwrap_or_default(),
                    options: output_options,
                    onchange: move |v: String| state.update_settings(|s| s.audio.output_device = Some(v).filter(|v| !v.is_empty())),
                }
            }
        }
        div { class: "two-col",
            Field { label: format!("{} · {} %", t("voice.input_volume"), (audio.input_volume * 100.0).round()),
                Slider { value: audio.input_volume, min: 0.0, max: 2.0, steps: 40, onchange: move |v| state.update_settings(|s| s.audio.input_volume = v) }
            }
            Field { label: format!("{} · {} %", t("voice.output_volume"), (audio.output_volume * 100.0).round()),
                Slider { value: audio.output_volume, min: 0.0, max: 2.0, steps: 40, onchange: move |v| state.update_settings(|s| s.audio.output_volume = v) }
            }
        }

        Field { label: t("voice.mic_test").to_string(), hint: t("voice.mic_test_hint").to_string(),
            div { class: "mic-test",
                button {
                    class: if *testing.read() { "button danger" } else { "button primary" },
                    onclick: move |_| {
                        let on = !*testing.peek();
                        testing.set(on);
                        test_engine.set_mic_test(on);
                    },
                    if *testing.read() { {t("voice.stop_test")} } else { {t("voice.start_test")} }
                }
                div { class: "meter",
                    div { class: if *transmitting.read() { "meter-fill live" } else { "meter-fill" }, style: "width: {meter}%;" }
                }
            }
        }

        div { class: "divider" }
        Field { label: t("voice.noise_suppression").to_string(), hint: ns_hint.to_string(),
            Segmented {
                value: ns.to_string(),
                options: vec![
                    opt("deepfilter", "DeepFilterNet"),
                    opt("rnnoise", "RNNoise"),
                    opt("webrtc", "WebRTC"),
                    opt("off", t("common.off")),
                ],
                onchange: move |v: String| state.update_settings(|s| s.audio.noise_suppression = match v.as_str() {
                    "rnnoise" => NoiseSuppression::RnNoise,
                    "webrtc" => NoiseSuppression::WebRtc,
                    "off" => NoiseSuppression::Off,
                    _ => NoiseSuppression::DeepFilter,
                }),
            }
        }
        ToggleRow {
            label: t("voice.echo").to_string(),
            hint: t("voice.echo_hint").to_string(),
            on: audio.echo_cancellation,
            onchange: move |v| state.update_settings(|s| s.audio.echo_cancellation = v),
        }
        ToggleRow {
            label: t("voice.agc").to_string(),
            hint: t("voice.agc_hint").to_string(),
            on: audio.auto_gain,
            onchange: move |v| state.update_settings(|s| s.audio.auto_gain = v),
        }

        div { class: "divider" }
        Field { label: t("voice.mode").to_string(),
            Segmented {
                value: mode.to_string(),
                options: vec![opt("vad", t("voice.mode_vad")), opt("ptt", t("voice.mode_ptt"))],
                onchange: move |v: String| state.update_settings(|s| {
                    s.audio.voice_mode = if v == "ptt" { VoiceMode::PushToTalk } else { VoiceMode::VoiceActivity }
                }),
            }
        }
        if audio.voice_mode == VoiceMode::VoiceActivity {
            ToggleRow {
                label: t("voice.auto_sensitivity").to_string(),
                hint: t("voice.auto_sensitivity_hint").to_string(),
                on: audio.auto_sensitivity,
                onchange: move |v| state.update_settings(|s| s.audio.auto_sensitivity = v),
            }
            if !audio.auto_sensitivity {
                Field { label: format!("{} · {} dB", t("voice.sensitivity"), audio.sensitivity_db.round()), hint: t("voice.sensitivity_hint").to_string(),
                    Slider {
                        value: audio.sensitivity_db,
                        min: -80.0,
                        max: 0.0,
                        steps: 80,
                        level: Some(current_level),
                        onchange: move |v| state.update_settings(|s| s.audio.sensitivity_db = v),
                    }
                }
            }
        } else {
            div { class: "two-col",
                Field { label: t("keys.ptt").to_string(),
                    KeybindButton { bind: ptt_key, set: move |b| state.update_settings(|s| s.keybinds.push_to_talk = b) }
                }
                Field { label: format!("{} · {} ms", t("voice.ptt_release"), audio.ptt_release_ms),
                    Slider {
                        value: audio.ptt_release_ms as f32,
                        min: 0.0,
                        max: 1000.0,
                        steps: 20,
                        onchange: move |v: f32| state.update_settings(|s| s.audio.ptt_release_ms = v as u32),
                    }
                }
            }
        }
    }
}

#[component]
fn VideoPage() -> Element {
    let state = use_context::<AppState>();
    let cameras = use_hook(list_cameras);
    let video = state.settings.read().video.clone();
    let mut camera_options = vec![opt("", t("settings.system_default"))];
    camera_options.extend(cameras.iter().map(|c| opt(c.name.clone(), c.name.clone())));
    let quality = match video.camera_quality {
        CameraQuality::P720_30 => "720",
        CameraQuality::P1080_30 => "1080",
        CameraQuality::P720_60 => "720-60",
    };

    rsx! {
        h1 { class: "section-title", {t("settings.video")} }
        if !media_foundation_available() {
            div { class: "callout warning", Svg { icon: Icon::Info, size: 18 } span { {t("video.no_mf")} } }
        }
        h2 { class: "subsection", {t("video.camera")} }
        div { class: "two-col",
            Field { label: t("video.camera_device").to_string(),
                if cameras.is_empty() {
                    div { class: "muted", {t("video.no_camera")} }
                } else {
                    Select {
                        value: video.camera.clone().unwrap_or_default(),
                        options: camera_options,
                        onchange: move |v: String| state.update_settings(|s| s.video.camera = Some(v).filter(|v| !v.is_empty())),
                    }
                }
            }
            Field { label: t("video.camera_quality").to_string(),
                Segmented {
                    value: quality.to_string(),
                    options: vec![opt("720", "720p · 30"), opt("1080", "1080p · 30"), opt("720-60", "720p · 60")],
                    onchange: move |v: String| state.update_settings(|s| s.video.camera_quality = match v.as_str() {
                        "720" => CameraQuality::P720_30,
                        "720-60" => CameraQuality::P720_60,
                        _ => CameraQuality::P1080_30,
                    }),
                }
            }
        }
        div { class: "divider" }
        h2 { class: "subsection", {t("video.screen")} }
        Field { label: t("share.quality").to_string(), hint: t("video.screen_quality_hint").to_string(),
            Segmented {
                value: format!("{:?}", video.screen_quality),
                options: ScreenQuality::ALL.iter().map(|q| opt(format!("{q:?}"), q.label())).collect(),
                onchange: move |v: String| {
                    if let Some(q) = ScreenQuality::ALL.into_iter().find(|q| format!("{q:?}") == v) {
                        state.update_settings(|s| s.video.screen_quality = q);
                    }
                },
            }
        }
        Field { label: t("share.optimize").to_string(),
            Segmented {
                value: if video.screen_hint == ContentHint::Motion { "motion".to_string() } else { "detail".to_string() },
                options: vec![opt("detail", t("share.detail")), opt("motion", t("share.motion"))],
                onchange: move |v: String| state.update_settings(|s| {
                    s.video.screen_hint = if v == "motion" { ContentHint::Motion } else { ContentHint::Detail }
                }),
            }
        }
        ToggleRow {
            label: t("share.audio").to_string(),
            hint: t("video.screen_audio_hint").to_string(),
            on: video.screen_audio,
            onchange: move |v| state.update_settings(|s| s.video.screen_audio = v),
        }
    }
}

/// Shows a binding; click to record a new key (Escape clears it).
#[component]
fn KeybindButton(bind: Option<Keybind>, set: EventHandler<Option<Keybind>>) -> Element {
    let services = use_context::<Services>();
    let mut captured = use_context::<CapturedKey>().0;
    let mut recording = use_signal(|| false);

    // Take the key the hotkey thread reports while this button is recording.
    use_effect(move || {
        let key = *captured.read();
        if let Some(key) = key
            && *recording.peek()
        {
            recording.set(false);
            captured.set(None);
            set.call(key);
        }
    });
    let hotkeys = services.hotkeys.clone();
    use_drop(move || hotkeys.cancel_capture());

    let label = if *recording.read() {
        t("keys.press_key").to_string()
    } else {
        bind.map(|b| b.label()).unwrap_or_else(|| t("keys.not_set").to_string())
    };
    let hotkeys = services.hotkeys.clone();
    rsx! {
        div { class: "keybind-row",
            div {
                class: if *recording.read() { "keybind recording" } else if bind.is_some() { "keybind" } else { "keybind empty" },
                onclick: move |_| {
                    captured.set(None);
                    recording.set(true);
                    hotkeys.capture_next();
                },
                Svg { icon: Icon::Keyboard, size: 16 }
                span { "{label}" }
            }
            if bind.is_some() {
                IconButton { icon: Icon::X, size: 16, title: t("keys.clear"), onclick: move |_| set.call(None) }
            }
        }
    }
}

#[component]
fn KeybindsPage() -> Element {
    let state = use_context::<AppState>();
    let Keybinds { push_to_talk, toggle_mute, toggle_deafen } = state.settings.read().keybinds.clone();
    rsx! {
        h1 { class: "section-title", {t("settings.keybinds")} }
        p { class: "muted lead", {t("keys.hint")} }
        Field { label: t("keys.ptt").to_string(), hint: t("keys.ptt_hint").to_string(),
            KeybindButton { bind: push_to_talk, set: move |b| state.update_settings(|s| {
                s.keybinds.push_to_talk = b;
                if b.is_some() {
                    s.audio.voice_mode = VoiceMode::PushToTalk;
                }
            }) }
        }
        Field { label: t("keys.mute").to_string(),
            KeybindButton { bind: toggle_mute, set: move |b| state.update_settings(|s| s.keybinds.toggle_mute = b) }
        }
        Field { label: t("keys.deafen").to_string(),
            KeybindButton { bind: toggle_deafen, set: move |b| state.update_settings(|s| s.keybinds.toggle_deafen = b) }
        }
    }
}

#[component]
fn AppearancePage() -> Element {
    let state = use_context::<AppState>();
    let appearance = state.settings.read().appearance.clone();
    let theme = match appearance.theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
        Theme::System => "system",
    };
    let language = match appearance.language {
        Language::System => "system",
        Language::German => "de",
        Language::English => "en",
    };
    rsx! {
        h1 { class: "section-title", {t("settings.appearance")} }
        Field { label: t("appearance.theme").to_string(),
            div { class: "theme-cards",
                for (value, label) in [("dark", t("appearance.dark")), ("light", t("appearance.light")), ("system", t("appearance.system"))] {
                    div {
                        key: "{value}",
                        class: if value == theme { "theme-card active" } else { "theme-card" },
                        onclick: move |_| state.update_settings(|s| s.appearance.theme = match value {
                            "dark" => Theme::Dark,
                            "light" => Theme::Light,
                            _ => Theme::System,
                        }),
                        div { class: "theme-preview {value}",
                            div { class: "tp-rail" }
                            div { class: "tp-side" }
                            div { class: "tp-main", div { class: "tp-line" } div { class: "tp-line short" } }
                        }
                        span { "{label}" }
                    }
                }
            }
        }
        Field { label: t("appearance.language").to_string(),
            Segmented {
                value: language.to_string(),
                options: vec![opt("system", t("appearance.system")), opt("de", "Deutsch"), opt("en", "English")],
                onchange: move |v: String| state.update_settings(|s| s.appearance.language = match v.as_str() {
                    "de" => Language::German,
                    "en" => Language::English,
                    _ => Language::System,
                }),
            }
        }
        Field { label: format!("{} · {} px", t("appearance.font_size"), appearance.font_size),
            Slider {
                value: appearance.font_size as f32,
                min: 13.0,
                max: 18.0,
                steps: 5,
                onchange: move |v: f32| state.update_settings(|s| s.appearance.font_size = v.round() as u8),
            }
        }
        ToggleRow {
            label: t("appearance.compact").to_string(),
            hint: t("appearance.compact_hint").to_string(),
            on: appearance.compact,
            onchange: move |v| state.update_settings(|s| s.appearance.compact = v),
        }
    }
}

#[component]
fn PrivacyPage() -> Element {
    let state = use_context::<AppState>();
    let settings = state.settings.read().clone();
    // The server keeps who may message you; load it once.
    let mut allow_server_dms = use_signal(|| None::<bool>);
    use_hook(move || {
        let Some(s) = state.session.cloned() else { return };
        spawn(async move {
            match s.api.me_settings().await {
                Ok(me) => allow_server_dms.set(Some(me.allow_server_dms)),
                Err(e) => state.report(Err(e.to_string())),
            }
        });
    });
    let set_allow = move |v: bool| {
        let Some(s) = state.session.cloned() else { return };
        let before = *allow_server_dms.peek();
        allow_server_dms.set(Some(v));
        spawn(async move {
            let req = sivryn_client_core::protocol::api::UpdateMeRequest { allow_server_dms: Some(v) };
            if let Err(e) = s.api.update_me(&req).await {
                allow_server_dms.set(before);
                state.report(Err(e.to_string()));
            }
        });
    };
    let blocked = state.friends.read().blocked.clone();

    rsx! {
        h1 { class: "section-title", {t("settings.privacy")} }
        if let Some(allow) = *allow_server_dms.read() {
            ToggleRow {
                label: t("privacy.server_dms").to_string(),
                hint: t("privacy.server_dms_hint").to_string(),
                on: allow,
                onchange: set_allow,
            }
        }
        ToggleRow {
            label: t("privacy.link_previews").to_string(),
            hint: t("privacy.link_previews_hint").to_string(),
            on: settings.link_previews,
            onchange: move |v| state.update_settings(|s| s.link_previews = v),
        }
        Field { label: t("privacy.blocked").to_string(),
            if blocked.is_empty() {
                div { class: "muted", {t("privacy.blocked_none")} }
            }
            div { class: "user-rows",
                for u in blocked {
                    div { key: "{u.id}", class: "user-row",
                        Avatar { name: u.display_name.clone().unwrap_or_else(|| u.username.clone()), id: Some(u.id), size: 32 }
                        span { class: "user-row-name", "{u.display_name.clone().unwrap_or_else(|| u.username.clone())}" }
                        span { class: "muted small", "@{u.username}" }
                        span { class: "topbar-spacer" }
                        button {
                            class: "button small",
                            onclick: {
                                let id = u.id;
                                move |_| {
                                    let Some(s) = state.session.cloned() else { return };
                                    spawn_forever(async move {
                                        state.report(s.api.unblock(id).await.map_err(|e| e.to_string()));
                                        state.report(state.refresh_friends().await);
                                    });
                                }
                            },
                            {t("menu.unblock")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NotificationsPage() -> Element {
    let state = use_context::<AppState>();
    let settings = state.settings.read().clone();
    rsx! {
        h1 { class: "section-title", {t("settings.notifications")} }
        ToggleRow {
            label: t("notify.sounds").to_string(),
            hint: t("notify.sounds_hint").to_string(),
            on: settings.audio.sounds,
            onchange: move |v| state.update_settings(|s| s.audio.sounds = v),
        }
        ToggleRow {
            label: t("notify.messages").to_string(),
            hint: t("notify.messages_hint").to_string(),
            on: settings.notifications.message_sound,
            onchange: move |v| state.update_settings(|s| s.notifications.message_sound = v),
        }
    }
}

#[component]
fn AboutPage() -> Element {
    let hw = use_hook(HardwareEncoders::detect);
    let yes_no = |b: bool| if b { t("common.yes") } else { t("common.no") };
    rsx! {
        h1 { class: "section-title", {t("settings.about")} }
        div { class: "about-hero",
            super::icons::Logo { size: 56 }
            div {
                div { class: "account-name", "Sivryn" }
                div { class: "muted", "Version {env!(\"CARGO_PKG_VERSION\")} · AGPL-3.0" }
            }
        }
        h2 { class: "subsection", {t("about.media")} }
        div { class: "info-grid",
            div { class: "info-label", "AV1 (GPU)" }
            div { {yes_no(hw.supports(VideoCodec::Av1))} }
            div { class: "info-label", "H.264 (GPU)" }
            div { {yes_no(hw.supports(VideoCodec::H264))} }
            div { class: "info-label", "Media Foundation" }
            div { {yes_no(media_foundation_available())} }
        }
        p { class: "muted", {t("about.links")} }
        p { class: "mono muted", "github.com/RTBGG/Sivryn" }
    }
}
