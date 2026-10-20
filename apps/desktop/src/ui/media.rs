//! Images, animations, videos and files in the chat, and the full-size viewer.

use std::sync::Arc;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;
use dioxus_native::use_wgpu;
use sivryn_client_core::protocol::model::{Attachment, Embed};
use sivryn_media::{MediaPlayer, RgbaFrame};
use tokio::sync::watch;

use super::icons::{Icon, Svg};
use super::{AppState, Services, ViewerImage};
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
pub fn FrameCanvas(frames: Frames, #[props(default)] class: String) -> Element {
    let source = use_wgpu(move || VideoSurface::new(frames.0.clone()));
    rsx! { canvas { class: "frame-canvas {class}", "src": "{source}" } }
}

/// An image attachment: placeholder first, then the decrypted image.
/// Click opens the viewer.
#[component]
pub fn ImageAttachment(attachment: Attachment, #[props(default)] url: Option<String>) -> Element {
    let state = use_context::<AppState>();
    let (w, h) = chat_size(attachment.width.unwrap_or(0), attachment.height.unwrap_or(0));
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
                }));
            }
            Ok(Err(e)) => state.report(Err(e)),
            Err(e) => state.report(Err(e.to_string())),
        }
    });
}

/// Full-size image over everything. It opens at most at its natural size;
/// a click zooms to 100 % (or 2× for small images), another click zooms out.
#[component]
pub fn ImageViewer() -> Element {
    let mut state = use_context::<AppState>();
    let mut zoomed = use_signal(|| false);
    let Some(image) = state.viewer.read().clone() else { return rsx! {} };
    let (w, h) = (image.width.max(1), image.height.max(1));
    let zoom = *zoomed.read();
    // CSS pixels; the window is roughly 1280×800 by default, the stage clips.
    let (dw, dh) = if zoom {
        if w < 600 && h < 600 { (w * 2, h * 2) } else { (w, h) }
    } else {
        // Fit the window (minus margins and the bar), never enlarge.
        let (vw, vh) = crate::window::client_size().unwrap_or((1280.0, 800.0));
        let scale = ((vw - 48.0) / w as f64).min((vh - 100.0) / h as f64).min(1.0);
        (((w as f64) * scale) as u32, ((h as f64) * scale) as u32)
    };

    rsx! {
        div { class: "viewer",
            onclick: move |_| state.viewer.set(None),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    state.viewer.set(None);
                }
            },
            div { class: if zoom { "viewer-stage zoomed" } else { "viewer-stage" },
                div {
                    class: if zoom { "viewer-image zoom-out" } else { "viewer-image zoom-in" },
                    style: "width: {dw}px; height: {dh}px; min-width: {dw}px;",
                    onclick: move |e| {
                        e.stop_propagation();
                        zoomed.toggle();
                    },
                    FrameCanvas { frames: Frames(image.frames.clone()) }
                }
            }
            div { class: "viewer-bar", onclick: move |e| e.stop_propagation(),
                span { class: "viewer-name", "{image.name}" }
                span { class: "muted", "{image.width} × {image.height}" }
                if let Some(url) = image.url.clone() {
                    span { class: "link", onclick: move |_| files::open_link(&url), {t("media.open_original")} }
                }
                div { class: "icon-button", onclick: move |_| state.viewer.set(None), Svg { icon: Icon::X, size: 18 } }
            }
        }
    }
}

/// A video (or audio) attachment, played inline on demand.
#[component]
pub fn VideoAttachment(attachment: Attachment) -> Element {
    let state = use_context::<AppState>();
    let services = use_context::<Services>();
    let mut player = use_signal(|| None::<Rc<MediaPlayer>>);
    let mut loading = use_signal(|| false);
    let mut progress = use_signal(|| (0u64, 0u64, false));
    let is_audio = attachment.mime.starts_with("audio/");

    // Progress polling while a player exists.
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let now = player.peek().as_ref().map(|p| (p.position_ms(), p.duration_ms(), p.is_playing()));
            if let Some(now) = now
                && *progress.peek() != now
            {
                progress.set(now);
            }
        }
    });

    let start = {
        let attachment = attachment.clone();
        let engine = services.engine.clone();
        move |_| {
            if let Some(p) = player.peek().as_ref() {
                p.set_playing(!p.is_playing());
                return;
            }
            let Some(s) = state.session.cloned() else { return };
            if *loading.peek() {
                return;
            }
            loading.set(true);
            let attachment = attachment.clone();
            let engine = engine.clone();
            spawn(async move {
                let result = match files::fetch(&s, &attachment).await {
                    Ok(bytes) => MediaPlayer::open(bytes, engine).map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                loading.set(false);
                match result {
                    Ok(p) => player.set(Some(Rc::new(p))),
                    Err(e) => state.report(Err(e)),
                }
            });
        }
    };
    let (pos, dur, playing) = *progress.read();
    let pct = if dur > 0 { (pos as f64 / dur as f64 * 100.0).min(100.0) } else { 0.0 };
    // Videos usually come without dimensions: a 16:9 frame (letterboxed).
    let (w, h) = match (attachment.width, attachment.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => chat_size(w, h),
        _ => (MAX_CHAT_WIDTH, MAX_CHAT_WIDTH * 9 / 16),
    };
    let w = w.max(320);
    let clock = |ms: u64| format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60);
    let frames = player.read().as_ref().map(|p| p.frames.clone());
    let seek_to = move |frac: f32| {
        if let Some(p) = player.peek().as_ref() {
            p.seek((p.duration_ms() as f32 * frac) as u64);
        }
    };

    rsx! {
        div { class: if is_audio { "chat-video audio" } else { "chat-video" },
            style: if is_audio { String::new() } else { format!("width: {w}px;") },
            if !is_audio {
                div { class: "video-frame", style: "height: {h}px;", onclick: start.clone(),
                    if let Some(frames) = frames {
                        FrameCanvas { frames: Frames(frames) }
                    } else {
                        div { class: "video-poster",
                            Svg { icon: Icon::Video, size: 28 }
                            span { "{attachment.name}" }
                        }
                    }
                    if !playing {
                        div { class: "play-overlay",
                            if *loading.read() { div { class: "spinner" } } else { Svg { icon: Icon::Play, size: 26 } }
                        }
                    }
                }
            }
            div { class: "video-controls",
                div { class: "icon-button", onclick: start,
                    if playing { Svg { icon: Icon::Pause, size: 16 } } else { Svg { icon: Icon::Play, size: 16 } }
                }
                if is_audio {
                    span { class: "video-name", "{attachment.name}" }
                }
                super::widgets::Slider { value: pct as f32, min: 0.0, max: 100.0, steps: 100, onchange: move |v: f32| seek_to(v / 100.0) }
                span { class: "video-time", "{clock(pos)} / {clock(dur)}" }
                DownloadButton { attachment: attachment.clone() }
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
            if let Some(site) = embed.site_name.clone() {
                div { class: "embed-site", "{site}" }
            }
            if let Some(title) = embed.title.clone() {
                div { class: "embed-title link", onclick: move |_| files::open_link(&url), "{title}" }
            }
            if let Some(description) = embed.description.clone() {
                div { class: "embed-description", "{description}" }
            }
            if let Some(image) = embed.image.clone() {
                div { class: "embed-image", ImageAttachment { attachment: image } }
            }
        }
    }
}
