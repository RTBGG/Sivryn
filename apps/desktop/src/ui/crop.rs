//! Cropping a picked image before it becomes an avatar, banner or server
//! icon: the crop frame always has the right aspect, can be moved by
//! dragging and resized with the zoom slider. Animated GIFs stay animated.

use std::sync::Arc;

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::dioxus_elements::input_data::MouseButton;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::GuildId;

use super::AppState;
use super::icons::{Icon, Svg};
use super::media::{FrameCanvas, Frames};
use super::widgets::{IconButton, Slider};
use crate::files::{self, Crop, centered_crop};
use crate::i18n::t;

/// What the cropped image becomes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CropPurpose {
    Avatar,
    Banner,
    GuildIcon(GuildId),
}

impl CropPurpose {
    /// The final size (and with it the aspect of the crop frame).
    fn size(self) -> (u32, u32) {
        match self {
            CropPurpose::Banner => (960, 340),
            _ => (256, 256),
        }
    }

    fn round(self) -> bool {
        self == CropPurpose::Avatar
    }
}

/// A picked image waiting to be cropped.
#[derive(Clone)]
pub struct CropRequest {
    pub purpose: CropPurpose,
    pub bytes: Arc<Vec<u8>>,
}

impl PartialEq for CropRequest {
    fn eq(&self, other: &Self) -> bool {
        self.purpose == other.purpose && Arc::ptr_eq(&self.bytes, &other.bytes)
    }
}

/// Lets the user pick an image file, then opens the crop dialog for it.
pub fn pick(mut state: AppState, purpose: CropPurpose) {
    spawn_forever(async move {
        let title = match purpose {
            CropPurpose::Avatar => t("profile.pick_avatar"),
            CropPurpose::Banner => t("profile.pick_banner"),
            CropPurpose::GuildIcon(_) => t("guild.pick_icon"),
        };
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title(title)
            .add_filter(t("profile.images"), &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
            .await
        else {
            return;
        };
        let bytes = file.read().await;
        state.crop.set(Some(CropRequest { purpose, bytes: Arc::new(bytes) }));
    });
}

/// Size of the area the image is shown in.
const STAGE: (f64, f64) = (560.0, 380.0);

#[component]
pub fn CropDialog() -> Element {
    let mut state = use_context::<AppState>();
    let Some(request) = state.crop.read().clone() else { return rsx! {} };
    rsx! {
        // Keyed by the image: a new pick starts fresh.
        for r in [request] {
            CropEditor { key: "{Arc::as_ptr(&r.bytes) as usize}", request: r.clone(), on_close: move |_| state.crop.set(None) }
        }
    }
}

#[component]
fn CropEditor(request: CropRequest, on_close: EventHandler<()>) -> Element {
    let state = use_context::<AppState>();
    let purpose = request.purpose;
    let aspect = purpose.size();
    // The image, decoded for display (first frame of an animation).
    let decoded = use_hook({
        let bytes = request.bytes.clone();
        move || {
            let dims = image::ImageReader::new(std::io::Cursor::new(bytes.as_slice()))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok());
            let frames = files::decode(&bytes, 1600, 1600).ok().map(files::play);
            dims.zip(frames)
        }
    });
    let Some(((width, height), frames)) = decoded.clone() else {
        return rsx! {
            div { class: "overlay", onclick: move |_| on_close.call(()),
                div { class: "dialog", {t("crop.unreadable")} }
            }
        };
    };
    let full = centered_crop(width, height, aspect);
    let mut zoom = use_signal(|| 1.0f32);
    // Center of the frame in image pixels.
    let mut center = use_signal(move || (width as f64 / 2.0, height as f64 / 2.0));
    let mut dragging = use_signal(|| None::<((f64, f64), (f64, f64))>);
    let mut busy = use_signal(|| false);

    // How the image is shown: fitted into the stage.
    let scale = (STAGE.0 / width as f64).min(STAGE.1 / height as f64);
    let (dw, dh) = (width as f64 * scale, height as f64 * scale);
    let crop = {
        let f = *zoom.read() as f64;
        let (w, h) = ((full.w as f64 * f).max(16.0), (full.h as f64 * f).max(16.0 * aspect.1 as f64 / aspect.0 as f64));
        let (cx, cy) = *center.read();
        let x = (cx - w / 2.0).clamp(0.0, width as f64 - w);
        let y = (cy - h / 2.0).clamp(0.0, height as f64 - h);
        Crop { x: x as u32, y: y as u32, w: w as u32, h: h as u32 }
    };
    let (fx, fy, fw, fh) = (crop.x as f64 * scale, crop.y as f64 * scale, crop.w as f64 * scale, crop.h as f64 * scale);

    let apply = move |_| {
        let Some(s) = state.session.cloned() else { return };
        if *busy.peek() {
            return;
        }
        busy.set(true);
        let bytes = request.bytes.clone();
        let mut state = state;
        spawn_forever(async move {
            let prepared = tokio::task::spawn_blocking(move || files::crop_image(&bytes, crop, aspect))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let result = match prepared {
                Ok((mime, data)) => match purpose {
                    CropPurpose::Avatar => s.api.set_profile_image(false, mime, data).await.map(|_| ()),
                    CropPurpose::Banner => s.api.set_profile_image(true, mime, data).await.map(|_| ()),
                    CropPurpose::GuildIcon(g) => s.api.set_guild_icon(g, mime, data).await.map(|_| ()),
                }
                .map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            busy.set(false);
            match result {
                Ok(()) => {
                    state.crop.set(None);
                    *state.profile_rev.write() += 1;
                    state.report(state.refresh().await);
                }
                Err(e) => state.report(Err(e)),
            }
        });
    };

    let shape = if purpose.round() { "crop-frame round" } else { "crop-frame" };
    rsx! {
        div { class: "overlay crop-overlay",
            onmousemove: move |e| {
                let Some((start, origin)) = *dragging.peek() else { return };
                if !e.held_buttons().contains(MouseButton::Primary) {
                    dragging.set(None);
                    return;
                }
                let p = e.client_coordinates();
                center.set((origin.0 + (p.x - start.0) / scale, origin.1 + (p.y - start.1) / scale));
            },
            onmouseup: move |_| dragging.set(None),
            div { class: "dialog crop-dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("crop.title")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| on_close.call(()) }
                }
                div { class: "crop-stage", style: "width: {STAGE.0}px; height: {STAGE.1}px;",
                    div { class: "crop-image", style: "width: {dw}px; height: {dh}px;",
                        FrameCanvas { frames: Frames(frames.clone()) }
                        // Darkened outside of the frame.
                        div { class: "crop-shade", style: "left: 0; top: 0; width: {dw}px; height: {fy}px;" }
                        div { class: "crop-shade", style: "left: 0; top: {fy + fh}px; width: {dw}px; height: {(dh - fy - fh).max(0.0)}px;" }
                        div { class: "crop-shade", style: "left: 0; top: {fy}px; width: {fx}px; height: {fh}px;" }
                        div { class: "crop-shade", style: "left: {fx + fw}px; top: {fy}px; width: {(dw - fx - fw).max(0.0)}px; height: {fh}px;" }
                        div {
                            class: "{shape}",
                            style: "left: {fx}px; top: {fy}px; width: {fw}px; height: {fh}px;",
                            onmousedown: move |e| {
                                let p = e.client_coordinates();
                                let origin = (crop.x as f64 + crop.w as f64 / 2.0, crop.y as f64 + crop.h as f64 / 2.0);
                                dragging.set(Some(((p.x, p.y), origin)));
                            },
                        }
                    }
                }
                div { class: "crop-zoom",
                    Svg { icon: Icon::Image, size: 14 }
                    Slider { value: 1.0 - *zoom.read(), min: 0.0, max: 0.9, steps: 45, onchange: move |v: f32| zoom.set(1.0 - v) }
                    Svg { icon: Icon::Image, size: 20 }
                }
                p { class: "hint", {t("crop.hint")} }
                div { class: "dialog-actions",
                    button { class: "button ghost", onclick: move |_| on_close.call(()), {t("common.cancel")} }
                    button { class: if *busy.read() { "button primary busy" } else { "button primary" }, onclick: apply,
                        {t("crop.apply")}
                    }
                }
            }
        }
    }
}
