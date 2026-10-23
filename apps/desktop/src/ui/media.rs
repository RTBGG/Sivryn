//! Images, animations, videos and files in the chat, and the full-size viewer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use dioxus_native::use_wgpu;
use sivryn_client_core::protocol::model::{Attachment, Embed};
use sivryn_media::{MediaPlayer, RgbaFrame};
use tokio::sync::watch;

use super::icons::{Icon, Svg};
use super::{AppState, Services, ViewerImage, menu};
use crate::files;
use crate::i18n::t;
use crate::video_surface::VideoSurface;

/// Largest size an image takes in the chat (CSS pixels).
pub const MAX_CHAT_WIDTH: u32 = 420;
pub const MAX_CHAT_HEIGHT: u32 = 320;

/// Size an image is shown at in the chat, keeping its aspect ratio.
pub fn chat_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (MAX_CHAT_WIDTH, MAX_CHAT_HEIGHT / 2);
    }
    let scale = (MAX_CHAT_WIDTH as f64 / width as f64).min(MAX_CHAT_HEIGHT as f64 / height as f64).min(1.0);
    (((width as f64 * scale) as u32).max(16), ((height as f64 * scale) as u32).max(16))
}

/// Frames to paint; equal when they come from the same channel.
#[derive(Clone)]
pub struct Frames(pub watch::Receiver<Option<Arc<RgbaFrame>>>);

impl PartialEq for Frames {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_channel(&other.0)
    }
}

/// Paints frames from a watch channel through the GPU, letterboxed.
#[component]
pub fn FrameCanvas(
    frames: Frames,
    #[props(default)] class: String,
    /// Fill the canvas, cropping the overflow, instead of letterboxing.
    #[props(default)]
    cover: bool,
) -> Element {
    let source = use_wgpu(move || {
        let surface = VideoSurface::new(frames.0.clone());
        if cover { surface.cover() } else { surface }
    });
    rsx! { canvas { class: "frame-canvas {class}", "src": "{source}" } }
}

/// An image attachment: placeholder first, then the decrypted image.
/// Click opens the viewer.
#[component]
pub fn ImageAttachment(
    attachment: Attachment,
    #[props(default)] url: Option<String>,
    /// Inside a link preview, which is a little narrower.
    #[props(default)]
    in_embed: bool,
) -> Element {
    let state = use_context::<AppState>();
    let (mut w, mut h) = chat_size(attachment.width.unwrap_or(0), attachment.height.unwrap_or(0));
    if in_embed && w > 400 {
        h = h * 400 / w;
        w = 400;
    }
    let menu_attachment = attachment.clone();
    let mut frames = use_signal(|| None::<watch::Receiver<Option<Arc<RgbaFrame>>>>);
    let mut failed = use_signal(|| false);
    let placeholder = use_hook(|| files::thumbnail(&attachment).map(|f| watch::channel(Some(f)).1));

    let att = attachment.clone();
    use_hook(move || {
        let Some(s) = state.session.cloned() else { return };
        spawn(async move {
            match files::fetch(&s, &att).await {
                Ok(bytes) => {
                    let decoded = tokio::task::spawn_blocking(move || {
                        files::decode(&bytes, MAX_CHAT_WIDTH * 2, MAX_CHAT_HEIGHT * 2)
                    })
                    .await;
                    match decoded {
                        Ok(Ok(d)) => frames.set(Some(files::play(d))),
                        _ => failed.set(true),
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "attachment unavailable");
                    failed.set(true);
                }
            }
        });
    });

    let open = {
        let attachment = attachment.clone();
        let url = url.clone();
        move |_| open_viewer(state, attachment.clone(), url.clone())
    };
    let title = url.clone().unwrap_or_else(|| attachment.name.clone());
    // The canvas binds its frames once, so the real image needs a new one.
    // Keys only take effect in lists, hence the one-item list.
    let shown = match (frames.read().clone(), placeholder) {
        (Some(f), _) => Some(("img", f, "")),
        (None, Some(p)) => Some(("thumb", p, "blurred")),
        (None, None) => None,
    };

    rsx! {
        div { class: "chat-image", style: "width: {w}px; height: {h}px;", title: "{title}", onclick: open,
            onmousedown: move |e| {
                if let Some((x, y)) = menu::right_click(&e) {
                    e.stop_propagation();
                    menu::open(state, x, y, menu::image_items(&menu_attachment));
                }
            },
            for (key, f, class) in shown.clone() {
                FrameCanvas { key: "{key}", frames: Frames(f), class }
            }
            if shown.is_none() {
                div { class: "image-loading", div { class: "spinner small" } }
            }
            if *failed.read() {
                div { class: "image-failed", Svg { icon: Icon::File, size: 20 } span { {t("media.unavailable")} } }
            }
            if let Some(url) = url {
                div { class: "image-link", "{url}" }
            }
        }
    }
}

fn open_viewer(mut state: AppState, attachment: Attachment, url: Option<String>) {
    let Some(s) = state.session.cloned() else { return };
    spawn_forever(async move {
        let bytes = match files::fetch(&s, &attachment).await {
            Ok(b) => b,
            Err(e) => return state.report(Err(e)),
        };
        // Full resolution up to a sane maximum for the GPU texture.
        let decoded = tokio::task::spawn_blocking(move || files::decode(&bytes, 8192, 8192)).await;
        match decoded {
            Ok(Ok(d)) => {
                let (width, height) = (d.width, d.height);
                state.viewer.set(Some(ViewerImage {
                    frames: files::play(d),
                    width,
                    height,
                    name: attachment.name.clone(),
                    url,
                    zoom: None,
                    attachment: Some(attachment.clone()),
                }));
            }
            Ok(Err(e)) => state.report(Err(e)),
            Err(e) => state.report(Err(e.to_string())),
        }
    });
}

/// Set while the image viewer is open, so Ctrl+wheel zooms instead of scrolling.
pub static VIEWER_OPEN: AtomicBool = AtomicBool::new(false);
/// Largest zoom, relative to the image's own size.
const MAX_ZOOM: f64 = 4.0;
/// Zoom change per wheel notch.
const ZOOM_STEP: f64 = 1.25;

/// The scale that fits the image into the window with some margin, but
/// never above 100 %.
fn base_scale(w: u32, h: u32) -> f64 {
    let (vw, vh) = crate::window::client_size().unwrap_or((1280.0, 800.0));
    ((vw - 48.0) / w.max(1) as f64).min((vh - 100.0) / h.max(1) as f64).min(1.0)
}

/// Ctrl+wheel in the viewer: zoom between the fitted size and 400 %.
pub fn wheel_zoom(mut state: AppState, notches: f64) {
    let mut viewer = state.viewer.write();
    let Some(image) = viewer.as_mut() else { return };
    let base = base_scale(image.width, image.height);
    let current = image.zoom.unwrap_or(base);
    let next = (current * ZOOM_STEP.powf(notches)).clamp(base, MAX_ZOOM);
    image.zoom = if (next - base).abs() < 1e-3 { None } else { Some(next) };
}

/// Full-size image over everything. It opens at most at its natural size
/// (smaller when it doesn't fit the window). A click zooms to 100 % (2× for
/// small images) and back; Ctrl+wheel zooms between that and 400 %.
#[component]
pub fn ImageViewer() -> Element {
    let mut state = use_context::<AppState>();
    use_hook(|| VIEWER_OPEN.store(true, Ordering::Relaxed));
    use_drop(|| VIEWER_OPEN.store(false, Ordering::Relaxed));
    let Some(image) = state.viewer.read().clone() else { return rsx! {} };
    let (w, h) = (image.width.max(1), image.height.max(1));
    let base = base_scale(w, h);
    let scale = image.zoom.unwrap_or(base);
    let zoomed = scale > base + 1e-3;
    let (dw, dh) = ((w as f64 * scale) as u32, (h as f64 * scale) as u32);
    let percent = (scale * 100.0).round();
    let toggle = move |e: MouseEvent| {
        e.stop_propagation();
        let mut viewer = state.viewer.write();
        if let Some(image) = viewer.as_mut() {
            image.zoom = if image.zoom.is_some() {
                None
            } else {
                let natural: f64 = if w < 600 && h < 600 { 2.0 } else { 1.0 };
                Some(natural.max(base * 2.0).min(MAX_ZOOM))
            };
        }
    };
    let attachment = image.attachment.clone();

    rsx! {
        div { class: "viewer",
            onclick: move |_| state.viewer.set(None),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    state.viewer.set(None);
                }
            },
            // `data-zoom`: zooming keeps the spot under the mouse in place.
            div { class: if zoomed { "viewer-stage zoomed" } else { "viewer-stage" }, "data-zoom": "{scale}",
                div {
                    class: if zoomed { "viewer-image zoom-out" } else { "viewer-image zoom-in" },
                    style: "width: {dw}px; height: {dh}px; min-width: {dw}px; min-height: {dh}px;",
                    onclick: toggle,
                    onmousedown: move |e| {
                        if let (Some((x, y)), Some(a)) = (menu::right_click(&e), attachment.as_ref()) {
                            e.stop_propagation();
                            menu::open(state, x, y, menu::image_items(a));
                        }
                    },
                    FrameCanvas { frames: Frames(image.frames.clone()) }
                }
            }
            div { class: "viewer-bar", onclick: move |e| e.stop_propagation(),
                span { class: "viewer-name", "{image.name}" }
                span { class: "muted", "{image.width} × {image.height} · {percent} %" }
                span { class: "muted small", {t("media.zoom_hint")} }
                if let Some(url) = image.url.clone() {
                    span { class: "link", onclick: move |_| files::open_link(&url), {t("media.open_original")} }
                }
                div { class: "icon-button", onclick: move |_| state.viewer.set(None), Svg { icon: Icon::X, size: 18 } }
            }
        }
    }
}

/// Loudness is perceived roughly logarithmically: the slider's position
/// cubed gives an even feel from quiet to loud (and 0 is silence).
pub(super) fn gain(volume: f32) -> f32 {
    volume.clamp(0.0, 1.0).powi(3)
}

thread_local! {
    /// The volume last set on any video, for the next one.
    static LAST_VOLUME: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// The volume last set on any video.
pub(super) fn remembered_volume() -> f32 {
    LAST_VOLUME.with(|v| v.get())
}

pub(super) fn remember_volume(volume: f32) {
    LAST_VOLUME.with(|v| v.set(volume));
}

thread_local! {
    /// The volume before the speaker button muted it, to go back to.
    static BEFORE_MUTE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// The speaker button: mutes, or goes back to the volume from before (full
/// volume when the slider itself was at zero).
pub(super) fn toggle_mute(current: f32) -> f32 {
    if current > 0.0 {
        BEFORE_MUTE.with(|v| v.set(current));
        0.0
    } else {
        BEFORE_MUTE.with(|v| v.get()).max(0.05)
    }
}

/// Height of the controls over a chat video.
const OVERLAY_HEIGHT: u32 = 52;

static NEXT_VIDEO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A video (or audio) attachment, played inline on demand. Only one video
/// has a player at a time: starting another frees this one's decoder and
/// memory, and it resumes where it was when played again.
#[component]
pub fn VideoAttachment(attachment: Attachment) -> Element {
    let mut state = use_context::<AppState>();
    let services = use_context::<Services>();
    let id = use_hook(|| NEXT_VIDEO.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let mut player = use_signal(|| None::<Rc<MediaPlayer>>);
    let mut loading = use_signal(|| false);
    // Download progress in percent while the file comes from the server.
    let mut downloaded = use_signal(|| None::<u8>);
    let mut progress = use_signal(|| (0u64, attachment.duration_ms.unwrap_or(0), false));
    // While the timeline is dragged: the thumb follows the pointer, not the player.
    let mut scrub = use_signal(|| None::<f32>);
    let mut resume_at = use_signal(|| 0u64);
    let mut volume = use_signal(remembered_volume);
    // Fullscreen was asked for before the video was loaded.
    let mut want_full = use_signal(|| false);
    // The controls lie over the picture and hide while the mouse rests on
    // it, or is elsewhere.
    let (hovered, mut mark) = super::widgets::use_hover();
    let (active, mut kick) = super::fullscreen::use_activity();
    let is_audio = attachment.mime.starts_with("audio/");
    // The sender's poster, until the video itself shows.
    let poster = use_hook({
        let attachment = attachment.clone();
        move || files::thumbnail(&attachment).map(|f| tokio::sync::watch::channel(Some(f)).1)
    });

    // Back from fullscreen, where the volume may have changed.
    use_effect(move || {
        if state.full_view.read().is_none() && *volume.peek() != remembered_volume() {
            volume.set(remembered_volume());
        }
    });

    // Another video took over: let go of the player.
    use_effect(move || {
        let active = *state.active_video.read();
        if active != id && player.peek().is_some() {
            if let Some(p) = player.peek().as_ref() {
                resume_at.set(p.position_ms());
            }
            player.set(None);
            progress.write().2 = false;
        }
    });

    // Progress polling while a player exists.
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if scrub.peek().is_some() {
                continue;
            }
            let now = player.peek().as_ref().map(|p| (p.position_ms(), p.duration_ms(), p.is_playing()));
            if let Some(now) = now
                && *progress.peek() != now
            {
                progress.set(now);
            }
        }
    });

    // Loads the file and opens a player: playing (from where it was left),
    // or paused at `still` to show the picture there.
    let open = {
        let attachment = attachment.clone();
        let engine = services.engine.clone();
        move |still: Option<u64>| {
            let Some(s) = state.session.cloned() else { return };
            if *loading.peek() {
                return;
            }
            loading.set(true);
            state.active_video.set(id);
            let attachment = attachment.clone();
            let engine = engine.clone();
            let resume = *resume_at.peek();
            spawn(async move {
                let fetched = files::fetch_with_progress(&s, &attachment, |got, total| {
                    let total = total.unwrap_or(attachment.size).max(1);
                    let pct = (got.saturating_mul(100) / total).min(99) as u8;
                    if *downloaded.peek() != Some(pct) {
                        downloaded.set(Some(pct));
                    }
                })
                .await;
                downloaded.set(None);
                let result = match fetched {
                    Ok(bytes) => match still {
                        Some(ms) => MediaPlayer::open_paused(bytes, engine, ms),
                        None => MediaPlayer::open(bytes, engine),
                    }
                    .map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                loading.set(false);
                match result {
                    Ok(p) => {
                        p.set_volume(gain(*volume.peek()));
                        if still.is_none() && resume > 0 {
                            p.seek(resume);
                        }
                        progress.write().2 = still.is_none();
                        let p = Rc::new(p);
                        player.set(Some(p.clone()));
                        if *want_full.peek() {
                            want_full.set(false);
                            super::fullscreen::open(state, super::fullscreen::FullView::Video(p));
                        }
                    }
                    Err(e) => state.report(Err(e)),
                }
            });
        }
    };

    let start = {
        let mut open = open.clone();
        move |_| {
            if let Some(p) = player.peek().as_ref() {
                let play = !p.is_playing();
                p.set_playing(play);
                // Show the new state right away, not at the next poll.
                progress.write().2 = play;
                if play {
                    state.active_video.set(id);
                }
                return;
            }
            open(None);
        }
    };
    let (pos, dur, playing) = *progress.read();
    let shown = scrub.read().unwrap_or(if dur > 0 { pos as f32 / dur as f32 } else { 0.0 });
    let pct = (shown * 100.0).clamp(0.0, 100.0);
    let pos = scrub.read().map_or(pos, |f| (f * dur as f32) as u64);
    // Videos usually come without dimensions: a 16:9 frame (letterboxed).
    let (w, h) = match (attachment.width, attachment.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => chat_size(w, h),
        _ => (MAX_CHAT_WIDTH, MAX_CHAT_WIDTH * 9 / 16),
    };
    let w = w.max(320);
    let clock = |ms: u64| format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60);
    // The canvas binds its frames once, so the player's picture needs a new
    // one in place of the poster (keys only work in lists).
    let shown_frames = match (player.read().as_ref(), poster.clone()) {
        (Some(p), _) => Some((format!("player-{:p}", Rc::as_ptr(p)), p.frames.clone())),
        (None, Some(poster)) => Some(("poster".to_string(), poster)),
        (None, None) => None,
    };
    let mut seek_to = {
        let mut open = open.clone();
        move |frac: f32| {
            let dur = progress.peek().1;
            if dur == 0 {
                return;
            }
            let ms = (dur as f32 * frac.clamp(0.0, 1.0)) as u64;
            match player.peek().as_ref() {
                Some(p) => p.seek(ms),
                // Not loaded yet: open it paused there to show the picture.
                None => open(Some(ms)),
            }
            resume_at.set(ms);
        }
    };
    let mut set_volume = move |v: f32| {
        volume.set(v);
        LAST_VOLUME.with(|l| l.set(v));
        if let Some(p) = player.peek().as_ref() {
            p.set_volume(gain(v));
        }
    };

    let fullscreen = {
        let mut open = open.clone();
        move |_| match player.peek().clone() {
            Some(p) => super::fullscreen::open(state, super::fullscreen::FullView::Video(p)),
            None => {
                want_full.set(true);
                open(None);
            }
        }
    };
    let start_frame = start.clone();
    // Audio: name and buttons on top, a wide timeline below. Video: one row
    // over the picture.
    let controls = if is_audio {
        rsx! {
            div { class: "audio-controls",
                div { class: "audio-row",
                    div { class: "icon-button", onclick: start,
                        if playing { Svg { icon: Icon::Pause, size: 18 } } else { Svg { icon: Icon::Play, size: 18 } }
                    }
                    span { class: "audio-name", "{attachment.name}" }
                    DownloadButton { attachment: attachment.clone() }
                }
                div { class: "audio-row timeline",
                    super::widgets::Slider {
                        value: pct,
                        min: 0.0,
                        max: 100.0,
                        steps: 200,
                        onchange: move |v: f32| {
                            scrub.set(Some(v / 100.0));
                            seek_to(v / 100.0);
                        },
                        onrelease: move |_| {
                            if let Some(f) = *scrub.peek() {
                                let dur = progress.peek().1;
                                progress.write().0 = (f * dur as f32) as u64;
                            }
                            scrub.set(None);
                        },
                    }
                    span { class: "video-time", "{clock(pos)} / {clock(dur)}" }
                    div {
                        class: "icon-button",
                        title: t("media.volume"),
                        onclick: move |_| {
                            let v = toggle_mute(*volume.peek());
                            set_volume(v);
                        },
                        if *volume.read() > 0.0 { Svg { icon: Icon::Volume, size: 16 } } else { Svg { icon: Icon::VolumeX, size: 16 } }
                    }
                    div { class: "video-volume",
                        super::widgets::Slider { value: *volume.read(), min: 0.0, max: 1.0, steps: 50, onchange: set_volume }
                    }
                }
            }
        }
    } else {
        rsx! {
        div {
            class: "video-controls overlay",
            // Placed from the frame's top: Blitz put a `bottom: 0` box (and a
            // flex-end one) in the middle of the frame.
            style: "top: {h.saturating_sub(OVERLAY_HEIGHT)}px; height: {OVERLAY_HEIGHT}px;",
            // Clicks on the controls don't play or pause.
            onclick: move |e| e.stop_propagation(),
            div { class: "icon-button", onclick: start,
                if playing { Svg { icon: Icon::Pause, size: 16 } } else { Svg { icon: Icon::Play, size: 16 } }
            }
            super::widgets::Slider {
                value: pct,
                min: 0.0,
                max: 100.0,
                steps: 200,
                onchange: move |v: f32| {
                    scrub.set(Some(v / 100.0));
                    seek_to(v / 100.0);
                },
                onrelease: move |_| {
                    if let Some(f) = *scrub.peek() {
                        let dur = progress.peek().1;
                        progress.write().0 = (f * dur as f32) as u64;
                    }
                    scrub.set(None);
                },
            }
            span { class: "video-time", "{clock(pos)} / {clock(dur)}" }
            // Volume: click the speaker to mute, the slider for 0–100 %.
            div {
                class: "icon-button",
                title: t("media.volume"),
                onclick: move |_| {
                    let v = toggle_mute(*volume.peek());
                    set_volume(v);
                },
                if *volume.read() > 0.0 { Svg { icon: Icon::Volume, size: 16 } } else { Svg { icon: Icon::VolumeX, size: 16 } }
            }
            div { class: "video-volume",
                super::widgets::Slider { value: *volume.read(), min: 0.0, max: 1.0, steps: 50, onchange: set_volume }
            }
            div { class: "icon-button", title: t("media.fullscreen"), onclick: fullscreen,
                Svg { icon: Icon::Maximize, size: 16 }
            }
            DownloadButton { attachment: attachment.clone() }
        }
        }
    };
    let show_controls = is_audio || !playing || (hovered && active);

    rsx! {
        div { class: if is_audio { "chat-video audio" } else { "chat-video" },
            style: if is_audio { String::new() } else { format!("width: {w}px;") },
            if !is_audio {
                div { class: "video-frame", style: "height: {h}px;", onclick: start_frame,
                    onmousemove: move |e| {
                        mark(e);
                        kick();
                    },
                    for (key, frames) in shown_frames.clone() {
                        FrameCanvas { key: "{key}", frames: Frames(frames) }
                    }
                    if shown_frames.is_none() {
                        div { class: "video-poster",
                            Svg { icon: Icon::Video, size: 28 }
                            span { "{attachment.name}" }
                        }
                    }
                    if !playing {
                        div { class: "play-overlay",
                            if *loading.read() {
                                div { class: "spinner" }
                            } else {
                                Svg { icon: Icon::Play, size: 26 }
                            }
                        }
                    }
                    if let Some(pct) = *downloaded.read() {
                        div { class: "download-progress", "{pct} %" }
                    }
                    if show_controls {
                        {controls.clone()}
                    }
                }
            }
            if is_audio {
                {controls}
            }
        }
    }
}

use std::rc::Rc;

#[component]
pub fn DownloadButton(attachment: Attachment) -> Element {
    let state = use_context::<AppState>();
    let save = move |_| {
        let Some(s) = state.session.cloned() else { return };
        let attachment = attachment.clone();
        spawn_forever(async move {
            match files::fetch(&s, &attachment).await {
                Ok(bytes) => match files::save_as(&attachment.name, bytes).await {
                    Ok(true) => state.toast(t("media.saved"), false),
                    Ok(false) => {}
                    Err(e) => state.report(Err(e)),
                },
                Err(e) => state.report(Err(e)),
            }
        });
    };
    rsx! { div { class: "icon-button", title: t("media.download"), onclick: save, Svg { icon: Icon::Download, size: 16 } } }
}

/// Any other file: name, size and a download button.
#[component]
pub fn FileAttachment(attachment: Attachment) -> Element {
    rsx! {
        div { class: "chat-file",
            div { class: "file-icon", Svg { icon: Icon::File, size: 26 } }
            div { class: "file-meta",
                div { class: "file-name", "{attachment.name}" }
                div { class: "muted small", "{files::human_size(attachment.size)}" }
            }
            DownloadButton { attachment: attachment.clone() }
        }
    }
}

#[component]
pub fn AttachmentView(attachment: Attachment) -> Element {
    if attachment.mime.starts_with("image/") {
        rsx! { ImageAttachment { attachment } }
    } else if attachment.mime.starts_with("video/") || attachment.mime.starts_with("audio/") {
        rsx! { VideoAttachment { attachment } }
    } else {
        rsx! { FileAttachment { attachment } }
    }
}

/// A link preview. Links that are just an image show only the image.
#[component]
pub fn EmbedView(embed: Embed) -> Element {
    if embed.image_only {
        return match embed.image {
            Some(image) => rsx! { ImageAttachment { attachment: image, url: Some(embed.url.clone()) } },
            None => rsx! {},
        };
    }
    let url = embed.url.clone();
    rsx! {
        div { class: "embed",
            div { class: "embed-bar" }
            div { class: "embed-content",
                if let Some(site) = embed.site_name.clone() {
                    div { class: "embed-site selectable", "{site}" }
                }
                if let Some(title) = embed.title.clone() {
                    div { class: "embed-title selectable",
                        span { class: "link", onclick: move |_| files::open_link(&url), "{title}" }
                    }
                }
                if let Some(description) = embed.description.clone() {
                    div { class: "embed-description selectable", "{description}" }
                }
                if let Some(image) = embed.image.clone() {
                    div { class: "embed-image", ImageAttachment { attachment: image, in_embed: true } }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmuting_goes_back_to_the_volume_before() {
        assert_eq!(toggle_mute(0.3), 0.0);
        assert_eq!(toggle_mute(0.0), 0.3);
        // Muted by the slider (no earlier level to go back to): full volume.
        BEFORE_MUTE.with(|v| v.set(1.0));
        assert_eq!(toggle_mute(0.0), 1.0);
    }
}
