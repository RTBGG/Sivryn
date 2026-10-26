//! `sivryn://` links: invite pages on an instance (`https://host/CODE`) open
//! `sivryn://join/{host}/{code}`, which starts Sivryn (or reaches the running
//! one through `tray::already_running`) with the invite.
//!
//! Like the autostart entry, only installed copies register the scheme under
//! `HKCU\Software\Classes\sivryn`; the uninstaller removes it again.

use std::sync::Mutex;

/// An invite to join (or sign up with) on an instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// Base URL, e.g. `https://chat.example.com`.
    pub instance: String,
    pub code: String,
}

impl Invite {
    /// Whether this invite belongs to the instance at `base_url`.
    pub fn is_for(&self, base_url: &str) -> bool {
        host_of(&self.instance) == host_of(base_url)
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or("").to_ascii_lowercase()
}

/// `sivryn://join/{host}/{code}` as an invite.
pub fn parse(link: &str) -> Option<Invite> {
    let rest = link.trim().strip_prefix("sivryn://")?;
    let mut parts = rest.trim_end_matches('/').split('/');
    if parts.next()? != "join" {
        return None;
    }
    let (host, code) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let host_ok = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b));
    let code_ok = !code.is_empty() && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !host_ok || !code_ok {
        return None;
    }
    // Local test instances run without TLS.
    let local = host.starts_with("localhost") || host.starts_with("127.") || host.starts_with("[::1]");
    let scheme = if local { "http" } else { "https" };
    Some(Invite { instance: format!("{scheme}://{host}"), code: code.to_owned() })
}

/// The code in what someone pasted: an invite link (`https://host/CODE`,
/// `…/invite/CODE`) or the code itself.
pub fn code_from(text: &str) -> &str {
    let text = text.trim().trim_end_matches('/');
    if text.contains("://") { text.rsplit('/').next().unwrap_or(text) } else { text }
}

/// The link to share for `code` on the instance at `base_url`.
pub fn invite_url(base_url: &str, code: &str) -> String {
    format!("{}/{code}", base_url.trim_end_matches('/'))
}

/// The `sivryn://` link this process was started with, if any.
pub fn from_args() -> Option<String> {
    std::env::args().skip(1).find(|a| a.starts_with("sivryn:"))
}

static PENDING: Mutex<Option<Invite>> = Mutex::new(None);

/// Remembers an invite for the app to pick up ([`take`]).
pub fn set_pending(invite: Invite) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(invite);
}

/// The invite that arrived last, once.
pub fn take() -> Option<Invite> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Registers the scheme for this installed copy (each start: the path may change).
pub fn register() {
    if !crate::updater::is_installed() {
        return;
    }
    #[cfg(windows)]
    if let Err(e) = imp::register() {
        tracing::warn!(error = %e, "could not register sivryn:// links");
    }
}

/// Removes the scheme (when uninstalling).
pub fn remove() {
    #[cfg(windows)]
    imp::remove();
}

#[cfg(windows)]
mod imp {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegDeleteTreeW, RegSetValueExW,
    };
    use windows::core::PCWSTR;

    const KEY: &str = "Software\\Classes\\sivryn";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Sets `value` (the default one for `None`) of `HKCU\{path}`.
    fn set(path: &str, name: Option<&str>, value: &str) -> windows::core::Result<()> {
        let path = wide(path);
        let name = name.map(wide);
        let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
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
            let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
            let result = RegSetValueExW(key, name, None, REG_SZ, Some(&data)).ok();
            let _ = RegCloseKey(key);
            result
        }
    }

    pub fn register() -> windows::core::Result<()> {
        let Ok(exe) = std::env::current_exe() else { return Ok(()) };
        // Velopack's launcher stays put across updates and passes the link on.
        let launcher = exe.parent().and_then(|p| p.parent()).map(|p| p.join("Sivryn.exe"));
        let target = launcher.filter(|l| l.exists()).unwrap_or(exe.clone());
        set(KEY, None, "URL:Sivryn")?;
        set(KEY, Some("URL Protocol"), "")?;
        set(&format!("{KEY}\\DefaultIcon"), None, &format!("\"{}\",0", exe.display()))?;
        set(&format!("{KEY}\\shell\\open\\command"), None, &format!("\"{}\" \"%1\"", target.display()))
    }

    pub fn remove() {
        let path = wide(KEY);
        // SAFETY: NUL-terminated string; a missing key is fine.
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_join_links() {
        let invite = parse("sivryn://join/chat.sivryn.com/DSvyu9HY4Y").unwrap();
        assert_eq!(invite.instance, "https://chat.sivryn.com");
        assert_eq!(invite.code, "DSvyu9HY4Y");
        assert_eq!(parse("sivryn://join/localhost:8080/ab-c_d/").unwrap().instance, "http://localhost:8080");
        assert!(parse("sivryn://join/chat.sivryn.com").is_none());
        assert!(parse("sivryn://join/chat.sivryn.com/a/b").is_none());
        assert!(parse("sivryn://join/evil\".com/x").is_none());
        assert!(parse("https://chat.sivryn.com/x").is_none());
    }

    #[test]
    fn finds_codes_in_pasted_links() {
        assert_eq!(code_from(" https://chat.sivryn.com/DSvyu9HY4Y "), "DSvyu9HY4Y");
        assert_eq!(code_from("https://chat.sivryn.com/invite/DSvyu9HY4Y/"), "DSvyu9HY4Y");
        assert_eq!(code_from("DSvyu9HY4Y"), "DSvyu9HY4Y");
    }

    #[test]
    fn matches_instances_by_host() {
        let invite = parse("sivryn://join/Chat.Sivryn.com/x").unwrap();
        assert!(invite.is_for("https://chat.sivryn.com"));
        assert!(invite.is_for("https://chat.sivryn.com/"));
        assert!(!invite.is_for("https://other.example.com"));
    }
}
