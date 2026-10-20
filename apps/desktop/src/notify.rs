//! Windows notifications (toasts) for new messages.
//!
//! Unpackaged apps need an App User Model ID with a display name and icon in
//! the registry before Windows shows their toasts; [`init`] registers it
//! under HKCU once per start (no admin rights needed). Clicking a toast sends
//! its channel to the receiver returned by [`init`].

use sivryn_client_core::protocol::ChannelId;
use tokio::sync::mpsc;

/// Identifies Sivryn to Windows' notification system.
#[cfg(windows)]
const AUMID: &str = "Sivryn.Desktop";

/// Registers the app for notifications; clicked toasts arrive on the receiver.
pub fn init() -> mpsc::UnboundedReceiver<ChannelId> {
    let (tx, rx) = mpsc::unbounded_channel();
    #[cfg(windows)]
    {
        if let Err(e) = imp::register() {
            tracing::warn!(error = %e, "could not register for notifications");
        }
        let _ = imp::CLICKS.set(tx);
    }
    #[cfg(not(windows))]
    drop(tx);
    rx
}

/// Shows a notification; clicking it opens `channel`.
pub fn show(title: &str, body: &str, channel: ChannelId) {
    #[cfg(windows)]
    if let Err(e) = imp::show(title, body, channel) {
        tracing::debug!(error = %e, "could not show notification");
    }
    #[cfg(not(windows))]
    let _ = (title, body, channel);
}

#[cfg(windows)]
mod imp {
    use std::sync::{Mutex, OnceLock};

    use sivryn_client_core::protocol::ChannelId;
    use tokio::sync::mpsc;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegSetValueExW,
    };
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::core::{HSTRING, PCWSTR};

    use super::AUMID;

    pub static CLICKS: OnceLock<mpsc::UnboundedSender<ChannelId>> = OnceLock::new();
    /// Toasts are kept alive so their click handlers keep working.
    static RECENT: Mutex<Vec<ToastNotification>> = Mutex::new(Vec::new());
    const KEEP: usize = 20;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn set_string(key: HKEY, name: &str, value: &str) -> windows::core::Result<()> {
        let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        // SAFETY: valid key handle, NUL-terminated name and data.
        unsafe { RegSetValueExW(key, PCWSTR(wide(name).as_ptr()), None, REG_SZ, Some(&data)).ok() }
    }

    pub fn register() -> Result<(), Box<dyn std::error::Error>> {
        // The icon Windows shows next to our toasts.
        let dir = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_default().join("Sivryn");
        std::fs::create_dir_all(&dir)?;
        let icon = dir.join("notification-icon.png");
        std::fs::write(&icon, include_bytes!("../assets/icons/icon-256.png"))?;

        let path = wide(&format!("Software\\Classes\\AppUserModelId\\{AUMID}"));
        let mut key = HKEY::default();
        // SAFETY: plain registry calls with valid, NUL-terminated strings.
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
            .ok()?;
        }
        let result =
            set_string(key, "DisplayName", "Sivryn").and_then(|_| set_string(key, "IconUri", &icon.to_string_lossy()));
        // SAFETY: the key was opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
        result?;
        // SAFETY: NUL-terminated ID; sets this process's identity for the shell.
        unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID))? };
        Ok(())
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    pub fn show(title: &str, body: &str, channel: ChannelId) -> windows::core::Result<()> {
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
            escape(title),
            escape(body)
        )))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        toast.Activated(&TypedEventHandler::new(move |_, _| {
            if let Some(tx) = CLICKS.get() {
                let _ = tx.send(channel);
            }
            Ok(())
        }))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)?;
        let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        recent.push(toast);
        if recent.len() > KEEP {
            recent.remove(0);
        }
        Ok(())
    }
}
