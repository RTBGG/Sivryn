//! Starting Sivryn with Windows.
//!
//! A value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! points at the installed app with [`ARG`], so a start from there can open
//! hidden in the notification area. Only installed copies register
//! themselves: a build run from a project folder must not end up there. The
//! uninstaller removes the value again (`updater::run_hooks`).

/// Marks a start by Windows (and not by the user).
pub const ARG: &str = "--autostart";

/// Whether this process was started by Windows at sign-in.
pub fn launched_at_sign_in() -> bool {
    std::env::args().any(|a| a == ARG)
}

/// Registers or removes the autostart entry (installed copies only).
pub fn apply(enabled: bool) {
    if !crate::updater::is_installed() {
        return;
    }
    #[cfg(windows)]
    {
        let result = if enabled { imp::register() } else { imp::remove() };
        if let Err(e) = result {
            tracing::warn!(error = %e, enabled, "could not change the autostart entry");
        }
    }
    #[cfg(not(windows))]
    let _ = enabled;
}

/// Removes the entry (when uninstalling).
pub fn remove() {
    #[cfg(windows)]
    let _ = imp::remove();
}

#[cfg(windows)]
mod imp {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegDeleteKeyValueW, RegSetValueExW,
    };
    use windows::core::PCWSTR;

    const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const NAME: &str = "Sivryn";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// What to run: Velopack's launcher next to the versions (it stays put
    /// across updates and passes the arguments on), or this executable.
    fn command() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        let launcher = exe.parent()?.parent()?.join("Sivryn.exe");
        let target = if launcher.exists() { launcher } else { exe };
        Some(format!("\"{}\" {}", target.display(), super::ARG))
    }

    pub fn register() -> windows::core::Result<()> {
        let Some(command) = command() else { return Ok(()) };
        let path = wide(RUN);
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
            let data: Vec<u8> = wide(&command).iter().flat_map(|c| c.to_le_bytes()).collect();
            let result = RegSetValueExW(key, PCWSTR(wide(NAME).as_ptr()), None, REG_SZ, Some(&data)).ok();
            let _ = RegCloseKey(key);
            result
        }
    }

    pub fn remove() -> windows::core::Result<()> {
        let (path, name) = (wide(RUN), wide(NAME));
        // SAFETY: NUL-terminated strings; a missing value is not an error worth reporting.
        let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), PCWSTR(name.as_ptr())) };
        Ok(())
    }
}
