//! Attachments: end-to-end encryption, upload, download, decoding.
//!
//! Each file is encrypted with a fresh random key (XChaCha20-Poly1305, the
//! 24-byte nonce prepended). The key travels inside the MLS-encrypted message,
//! so the server only ever sees ciphertext and a size.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use image::AnimationDecoder;
use sivryn_client_core::protocol::model::Attachment;
use sivryn_client_core::protocol::{ChannelId, FileId};
use sivryn_media::RgbaFrame;
use tokio::sync::watch;

use crate::session::Session;

const NONCE_LEN: usize = 24;
/// Decrypted files kept in memory, so scrolling back doesn't re-download.
const CACHE_BYTES: usize = 256 * 1024 * 1024;
/// Larger files are only cached on disk.
const MAX_MEMORY_CACHED: usize = 24 * 1024 * 1024;
/// Longest edge of the placeholder sent with images.
const THUMBNAIL_EDGE: u32 = 48;
/// Longest edge of the poster a video gets when it is sent.
const POSTER_EDGE: u32 = 640;
/// Animated images are capped so a huge GIF can't eat all memory.
const MAX_FRAMES: usize = 600;

pub fn encrypt(plain: &[u8]) -> (String, Vec<u8>) {
    let mut key = [0u8; 32];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut key).expect("OS random number generator");
    getrandom::fill(&mut nonce).expect("OS random number generator");
    let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
    let ciphertext = cipher.encrypt(&XNonce::from(nonce), plain).expect("encryption cannot fail");
    (B64.encode(key), [nonce.as_slice(), &ciphertext].concat())
}

pub fn decrypt(key: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
    let key = B64.decode(key).map_err(|_| "invalid attachment key")?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| "invalid attachment key")?;
    if sealed.len() < NONCE_LEN {
        return Err("attachment is corrupt".into());
    }
    let (nonce, ct) = sealed.split_at(NONCE_LEN);
    let nonce = XNonce::try_from(nonce).map_err(|_| "attachment is corrupt")?;
    cipher.decrypt(&nonce, ct).map_err(|_| "attachment could not be decrypted".into())
}

/// A file chosen to be sent, read and inspected but not uploaded yet.
#[derive(Clone, PartialEq)]
pub struct PendingFile {
    pub name: String,
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
    /// Videos: their length.
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub thumbnail: Option<String>,
}

impl PendingFile {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }
}

pub fn mime_for(name: &str) -> &'static str {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" | "log" | "md" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Reads a file and, for images, notes its size and makes a placeholder.
pub fn prepare(name: String, bytes: Vec<u8>) -> PendingFile {
    let mut mime = mime_for(&name).to_owned();
    // Without a telling extension (e.g. a link preview), look at the content.
    if mime == "application/octet-stream"
        && let Ok(format) = image::guess_format(&bytes)
    {
        mime = format.to_mime_type().to_owned();
    }
    let mut file = PendingFile {
        name,
        mime,
        bytes: Arc::new(bytes),
        width: None,
        height: None,
        duration_ms: None,
        thumbnail: None,
    };
    if file.is_image()
        && let Ok(img) = image::load_from_memory(&file.bytes)
    {
        file.width = Some(img.width());
        file.height = Some(img.height());
        let thumb = img.thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE);
        let mut png = Vec::new();
        if thumb.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).is_ok() {
            file.thumbnail = Some(B64.encode(png));
        }
    }
    // Videos get their first picture as a poster, so they show something
    // before anyone presses play.
    if file.mime.starts_with("video/")
        && let Some((frame, duration)) = sivryn_media::video_poster(&file.bytes)
    {
        file.width = Some(frame.width);
        file.height = Some(frame.height);
        file.duration_ms = Some(duration);
        if let Some(img) = image::RgbaImage::from_raw(frame.width, frame.height, frame.data) {
            let small = image::DynamicImage::ImageRgba8(img).thumbnail(POSTER_EDGE, POSTER_EDGE).to_rgb8();
            let mut jpeg = Vec::new();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75);
            if encoder.encode_image(&small).is_ok() {
                file.thumbnail = Some(B64.encode(jpeg));
            }
        }
    }
    file
}

pub fn prepare_path(path: &Path) -> Result<PendingFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    Ok(prepare(name, bytes))
}

/// Encrypts and uploads a file; returns what goes into the message.
pub async fn upload(s: &Session, channel: ChannelId, file: &PendingFile) -> Result<Attachment, String> {
    let (key, sealed) = encrypt(&file.bytes);
    let uploaded = s.api.upload_attachment(channel, sealed.clone()).await.map_err(|e| e.to_string())?;
    // Big files don't stay in memory: keep the sender's copy on disk, so a
    // restart doesn't download it again.
    disk_write(uploaded.id, &sealed).await;
    let attachment = Attachment {
        file_id: uploaded.id,
        name: file.name.clone(),
        mime: file.mime.clone(),
        size: file.bytes.len() as u64,
        key,
        width: file.width,
        height: file.height,
        duration_ms: file.duration_ms,
        thumbnail: file.thumbnail.clone(),
    };
    // The sender has the plaintext already; no need to download it again.
    if file.bytes.len() <= MAX_MEMORY_CACHED {
        cache().put(uploaded.id, file.bytes.clone());
    }
    Ok(attachment)
}

struct Cache {
    files: HashMap<FileId, Arc<Vec<u8>>>,
    order: Vec<FileId>,
    bytes: usize,
}

struct SharedCache(Mutex<Cache>);

impl SharedCache {
    fn get(&self, id: FileId) -> Option<Arc<Vec<u8>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).files.get(&id).cloned()
    }

    fn put(&self, id: FileId, data: Arc<Vec<u8>>) {
        let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if c.files.contains_key(&id) {
            return;
        }
        c.bytes += data.len();
        c.files.insert(id, data);
        c.order.push(id);
        while c.bytes > CACHE_BYTES && c.order.len() > 1 {
            let old = c.order.remove(0);
            if let Some(d) = c.files.remove(&old) {
                c.bytes -= d.len();
            }
        }
    }
}

fn cache() -> &'static SharedCache {
    static CACHE: std::sync::OnceLock<SharedCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| SharedCache(Mutex::new(Cache { files: HashMap::new(), order: Vec::new(), bytes: 0 })))
}

/// Files kept on disk at most, so nothing is downloaded twice.
const DISK_CACHE_BYTES: u64 = 1024 * 1024 * 1024;

/// `%LOCALAPPDATA%\Sivryn\cache\files`. File IDs never change content, so
/// cached files never go stale. Attachments are stored as the server has
/// them, encrypted; their keys only live in the (encrypted) message store.
fn disk_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("XDG_CACHE_HOME"))?;
    Some(std::path::PathBuf::from(base).join("Sivryn").join("cache").join("files"))
}

async fn disk_read(file: FileId) -> Option<Vec<u8>> {
    let path = disk_dir()?.join(file.to_string());
    let data = tokio::fs::read(&path).await.ok()?;
    // Touch it, so pruning keeps recently used files.
    let _ =
        std::fs::File::options().append(true).open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    Some(data)
}

async fn disk_remove(file: FileId) {
    if let Some(dir) = disk_dir() {
        let _ = tokio::fs::remove_file(dir.join(file.to_string())).await;
    }
}

async fn disk_write(file: FileId, data: &[u8]) {
    let Some(dir) = disk_dir() else { return };
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return;
    }
    // Write to a temporary name first, so a crash never leaves half a file.
    let tmp = dir.join(format!("{file}.part"));
    if tokio::fs::write(&tmp, data).await.is_ok() {
        let _ = tokio::fs::rename(&tmp, dir.join(file.to_string())).await;
    }
}

/// Deletes the least recently used cached files above the size limit.
pub fn prune_disk_cache() {
    let Some(dir) = disk_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= DISK_CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// The file's bytes as the server stores them: memory and disk cache first.
async fn download(s: &Session, file: FileId, progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>, String> {
    if let Some(data) = disk_read(file).await {
        return Ok(data);
    }
    let data = s.api.download_file_with_progress(file, progress).await.map_err(|e| e.to_string())?;
    disk_write(file, &data).await;
    Ok(data)
}

/// Downloads and decrypts an attachment (cached in memory and on disk).
pub async fn fetch(s: &Session, attachment: &Attachment) -> Result<Arc<Vec<u8>>, String> {
    fetch_with_progress(s, attachment, |_, _| {}).await
}

/// [`fetch`], reporting download progress (bytes so far, total) when the
/// file isn't cached.
pub async fn fetch_with_progress(
    s: &Session,
    attachment: &Attachment,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<Arc<Vec<u8>>, String> {
    if let Some(data) = cache().get(attachment.file_id) {
        return Ok(data);
    }
    let sealed = download(s, attachment.file_id, progress).await?;
    let key = attachment.key.clone();
    let plain = match tokio::task::spawn_blocking(move || decrypt(&key, &sealed)).await.map_err(|e| e.to_string())? {
        Ok(plain) => plain,
        Err(e) => {
            // A damaged copy (e.g. a cut-off download) must not stay cached.
            disk_remove(attachment.file_id).await;
            return Err(e);
        }
    };
    let data = Arc::new(plain);
    // Big files (videos) aren't kept decrypted in memory; the disk cache
    // still saves downloading them again.
    if data.len() <= MAX_MEMORY_CACHED {
        cache().put(attachment.file_id, data.clone());
    }
    Ok(data)
}

/// Downloads a public, unencrypted file such as a server icon or avatar
/// (cached in memory and on disk).
pub async fn fetch_plain(s: &Session, file: FileId) -> Result<Arc<Vec<u8>>, String> {
    if let Some(data) = cache().get(file) {
        return Ok(data);
    }
    let data = Arc::new(download(s, file, |_, _| {}).await?);
    cache().put(file, data.clone());
    Ok(data)
}

/// Asks where to save a file, then writes it.
pub async fn save_as(name: &str, data: Arc<Vec<u8>>) -> Result<bool, String> {
    let Some(target) = rfd::AsyncFileDialog::new().set_file_name(name).save_file().await else {
        return Ok(false);
    };
    tokio::fs::write(target.path(), data.as_slice()).await.map_err(|e| e.to_string())?;
    Ok(true)
}

/// A decoded image, ready for the GPU: one frame, or an animation.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<(Arc<RgbaFrame>, Duration)>,
}

fn to_frame(img: image::RgbaImage) -> Arc<RgbaFrame> {
    let (width, height) = img.dimensions();
    Arc::new(RgbaFrame { width, height, data: img.into_raw() })
}

/// Decodes an image, scaled down to fit `max_w × max_h` (never up).
/// Animated GIFs keep their frames and timing.
pub fn decode(bytes: &[u8], max_w: u32, max_h: u32) -> Result<Decoded, String> {
    let fit = |img: image::RgbaImage| {
        let (w, h) = img.dimensions();
        if w <= max_w && h <= max_h {
            return img;
        }
        let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
        let (nw, nh) = (((w as f64 * scale) as u32).max(1), ((h as f64 * scale) as u32).max(1));
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
    };

    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    if format == image::ImageFormat::Gif
        && let Ok(decoder) = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
    {
        let frames: Vec<image::Frame> = decoder.into_frames().take(MAX_FRAMES).filter_map(Result::ok).collect();
        if frames.len() > 1 {
            let first = frames[0].buffer().dimensions();
            let decoded: Vec<(Arc<RgbaFrame>, Duration)> = frames
                .into_iter()
                .map(|f| {
                    let (n, d) = f.delay().numer_denom_ms();
                    // Browsers treat very short GIF delays as 100 ms.
                    let ms = n.checked_div(d).map_or(100, |ms| ms.max(20));
                    (to_frame(fit(f.into_buffer())), Duration::from_millis(ms as u64))
                })
                .collect();
            return Ok(Decoded { width: first.0, height: first.1, frames: decoded });
        }
    }
    let img = image::load_from_memory_with_format(bytes, format).map_err(|e| e.to_string())?.to_rgba8();
    let (width, height) = img.dimensions();
    Ok(Decoded { width, height, frames: vec![(to_frame(fit(img)), Duration::ZERO)] })
}

/// A rectangle of an image, in its own pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Largest upload for avatars, banners and server icons.
const MAX_PROFILE_IMAGE: usize = 4 * 1024 * 1024;

/// Cuts `crop` out of an image and scales it to `size` (avatars and icons
/// 256², banners 960×340). Animated GIFs are cut frame by frame and stay
/// animated; if the result is too big, every other frame is dropped (with
/// the delays added up). Returns the content type and the bytes.
pub fn crop_image(bytes: &[u8], crop: Crop, size: (u32, u32)) -> Result<(&'static str, Vec<u8>), String> {
    use image::AnimationDecoder;
    let format = image::guess_format(bytes).map_err(|_| "not an image".to_string())?;
    if format == image::ImageFormat::Gif {
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let frames: Vec<image::Frame> =
            decoder.into_frames().take(MAX_FRAMES).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        if frames.len() > 1 {
            let cut: Vec<(image::RgbaImage, u32)> = frames
                .iter()
                .map(|f| {
                    let (num, den) = f.delay().numer_denom_ms();
                    let piece = image::imageops::crop_imm(f.buffer(), crop.x, crop.y, crop.w, crop.h).to_image();
                    let scaled = image::imageops::resize(&piece, size.0, size.1, image::imageops::FilterType::Triangle);
                    (scaled, num.checked_div(den).unwrap_or(100))
                })
                .collect();
            let mut step = 1;
            loop {
                let gif = encode_gif(&cut, step)?;
                if gif.len() <= MAX_PROFILE_IMAGE || step >= 8 {
                    return Ok(("image/gif", gif));
                }
                step *= 2;
            }
        }
    }
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let piece = img.crop_imm(crop.x, crop.y, crop.w, crop.h);
    let scaled = piece.resize_exact(size.0, size.1, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    scaled.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(("image/png", out))
}

/// An animated GIF from every `step`-th frame (delays in milliseconds).
fn encode_gif(frames: &[(image::RgbaImage, u32)], step: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 10);
        encoder.set_repeat(image::codecs::gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        for chunk in frames.chunks(step) {
            let delay: u32 = chunk.iter().map(|(_, d)| *d).sum();
            let frame =
                image::Frame::from_parts(chunk[0].0.clone(), 0, 0, image::Delay::from_numer_denom_ms(delay.max(20), 1));
            encoder.encode_frame(frame).map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

/// The largest rectangle with the aspect `w:h` in the middle of an image.
pub fn centered_crop(width: u32, height: u32, aspect: (u32, u32)) -> Crop {
    let target = aspect.0 as f64 / aspect.1 as f64;
    let (w, h) = if width as f64 / height as f64 > target {
        ((height as f64 * target).round() as u32, height)
    } else {
        (width, (width as f64 / target).round() as u32)
    };
    let (w, h) = (w.clamp(1, width), h.clamp(1, height));
    Crop { x: (width - w) / 2, y: (height - h) / 2, w, h }
}

/// Makes everything outside the inscribed circle transparent (avatars).
pub fn round(decoded: &mut Decoded) {
    for (frame, _) in &mut decoded.frames {
        let f = Arc::make_mut(frame);
        let (w, h) = (f.width as f32, f.height as f32);
        let r = w.min(h) / 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        for y in 0..f.height {
            for x in 0..f.width {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                // One pixel of anti-aliasing at the edge.
                let coverage = (r - d + 0.5).clamp(0.0, 1.0);
                if coverage < 1.0 {
                    let i = ((y * f.width + x) * 4 + 3) as usize;
                    f.data[i] = (f.data[i] as f32 * coverage) as u8;
                }
            }
        }
    }
}

/// Shows a decoded image through a `watch` channel, like a video feed:
/// animations loop until nobody watches any more.
pub fn play(decoded: Decoded) -> watch::Receiver<Option<Arc<RgbaFrame>>> {
    let (tx, rx) = watch::channel(decoded.frames.first().map(|f| f.0.clone()));
    if decoded.frames.len() > 1 {
        tokio::spawn(async move {
            loop {
                for (frame, delay) in &decoded.frames {
                    if tx.send(Some(frame.clone())).is_err() {
                        return;
                    }
                    tokio::time::sleep(*delay).await;
                }
            }
        });
    }
    rx
}

/// Like [`play`], but an animation only runs while `gate` is true; otherwise
/// it rests on its first frame. Avatars, banners and server icons animate
/// only while hovered: every frame repaints the whole window.
pub fn play_gated(decoded: Decoded, mut gate: watch::Receiver<bool>) -> watch::Receiver<Option<Arc<RgbaFrame>>> {
    let first = decoded.frames.first().map(|f| f.0.clone());
    let (tx, rx) = watch::channel(first.clone());
    if decoded.frames.len() > 1 {
        tokio::spawn(async move {
            loop {
                // Resting: the first frame until the gate opens.
                while !*gate.borrow_and_update() {
                    if tx.send(first.clone()).is_err() || gate.changed().await.is_err() {
                        return;
                    }
                }
                for (frame, delay) in &decoded.frames {
                    if !*gate.borrow() {
                        break;
                    }
                    if tx.send(Some(frame.clone())).is_err() {
                        return;
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(*delay) => {}
                        changed = gate.changed() => {
                            if changed.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
    }
    rx
}

/// Decodes the small placeholder that comes with an image attachment.
pub fn thumbnail(attachment: &Attachment) -> Option<Arc<RgbaFrame>> {
    let png = B64.decode(attachment.thumbnail.as_ref()?).ok()?;
    let img = image::load_from_memory(&png).ok()?.to_rgba8();
    Some(to_frame(img))
}

/// Opens a web link in the default browser (http and https only).
pub fn open_link(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{HSTRING, w};
        // SAFETY: plain shell call with valid, NUL-terminated strings.
        unsafe {
            ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
        }
    }
}

pub fn human_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / 1024.0 / 1024.0),
        b => format!("{:.2} GB", b as f64 / 1024.0 / 1024.0 / 1024.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_keep_the_aspect_and_gifs_stay_animated() {
        assert_eq!(centered_crop(400, 200, (1, 1)), Crop { x: 100, y: 0, w: 200, h: 200 });
        assert_eq!(centered_crop(960, 1000, (960, 340)), Crop { x: 0, y: 330, w: 960, h: 340 });

        // A two-frame GIF, red then blue.
        let mut gif = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let img = image::RgbaImage::from_pixel(40, 20, image::Rgba(color));
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(100, 1)))
                    .unwrap();
            }
        }
        let crop = centered_crop(40, 20, (1, 1));
        let (mime, out) = crop_image(&gif, crop, (16, 16)).unwrap();
        assert_eq!(mime, "image/gif");
        let decoded = decode(&out, 64, 64).unwrap();
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!((decoded.frames[0].0.width, decoded.frames[0].0.height), (16, 16));

        let png = {
            let mut out = Vec::new();
            image::RgbaImage::from_pixel(30, 10, image::Rgba([0, 255, 0, 255]))
                .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        let (mime, out) = crop_image(&png, centered_crop(30, 10, (960, 340)), (96, 34)).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(image::load_from_memory(&out).unwrap().width(), 96);
    }

    #[test]
    fn attachments_round_trip_and_reject_wrong_keys() {
        let data = b"top secret picture".to_vec();
        let (key, sealed) = encrypt(&data);
        assert_ne!(&sealed[NONCE_LEN..], data.as_slice());
        assert_eq!(decrypt(&key, &sealed).unwrap(), data);
        let (other, _) = encrypt(b"x");
        assert!(decrypt(&other, &sealed).is_err());
        assert!(decrypt(&key, &sealed[..10]).is_err());
    }

    #[test]
    fn images_get_size_and_placeholder() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(300, 200, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let file = prepare("cat.PNG".into(), png.clone());
        assert_eq!((file.mime.as_str(), file.width, file.height), ("image/png", Some(300), Some(200)));
        assert!(file.thumbnail.is_some());

        let decoded = decode(&png, 150, 150).unwrap();
        assert_eq!((decoded.width, decoded.height), (300, 200));
        assert_eq!((decoded.frames[0].0.width, decoded.frames[0].0.height), (150, 100));
        assert_eq!(mime_for("clip.mp4"), "video/mp4");
        assert_eq!(mime_for("noext"), "application/octet-stream");

        // Images without an extension (link previews) are recognized by content.
        let preview = prepare("preview".into(), png);
        assert_eq!((preview.mime.as_str(), preview.width), ("image/png", Some(300)));
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
