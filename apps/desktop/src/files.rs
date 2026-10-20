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
/// Longest edge of the placeholder sent with images.
const THUMBNAIL_EDGE: u32 = 48;
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
    let mime = mime_for(&name).to_owned();
    let mut file = PendingFile { name, mime, bytes: Arc::new(bytes), width: None, height: None, thumbnail: None };
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
    let uploaded = s.api.upload_attachment(channel, sealed).await.map_err(|e| e.to_string())?;
    let attachment = Attachment {
        file_id: uploaded.id,
        name: file.name.clone(),
        mime: file.mime.clone(),
        size: file.bytes.len() as u64,
        key,
        width: file.width,
        height: file.height,
        duration_ms: None,
        thumbnail: file.thumbnail.clone(),
    };
    // The sender has the plaintext already; no need to download it again.
    cache().put(uploaded.id, file.bytes.clone());
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

/// Downloads and decrypts an attachment (cached in memory).
pub async fn fetch(s: &Session, attachment: &Attachment) -> Result<Arc<Vec<u8>>, String> {
    if let Some(data) = cache().get(attachment.file_id) {
        return Ok(data);
    }
    let sealed = s.api.download_file(attachment.file_id).await.map_err(|e| e.to_string())?;
    let plain = decrypt(&attachment.key, &sealed)?;
    let data = Arc::new(plain);
    cache().put(attachment.file_id, data.clone());
    Ok(data)
}

/// Downloads a public, unencrypted file such as a server icon (cached).
pub async fn fetch_plain(s: &Session, file: FileId) -> Result<Arc<Vec<u8>>, String> {
    if let Some(data) = cache().get(file) {
        return Ok(data);
    }
    let data = Arc::new(s.api.download_file(file).await.map_err(|e| e.to_string())?);
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
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
