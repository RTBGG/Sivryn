//! Window chrome: the app icon and a title bar colored like the app.

/// The Sivryn mark for the title bar and taskbar (64×64 RGBA).
pub fn icon() -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(include_bytes!("../assets/icons/icon-64.rgba").to_vec(), 64, 64).ok()
}

/// Colors the title bar to match the server rail (`--bg-0`) on Windows 11.
/// The window may not exist yet right after launch, so this retries briefly.
pub async fn style_title_bar(dark: bool) {
    #[cfg(windows)]
    for _ in 0..50 {
        if imp::apply(dark) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    #[cfg(not(windows))]
    let _ = dark;
}

/// Whether Sivryn's window has the keyboard focus right now.
pub fn is_focused() -> bool {
    #[cfg(windows)]
    return imp::is_focused();
    #[cfg(not(windows))]
    true
}

/// Flashes the taskbar button until the window gets focus.
pub fn flash() {
    #[cfg(windows)]
    imp::flash();
}

/// Brings the window to the front (e.g. after clicking a notification),
/// also when it was hidden to the notification area.
pub fn bring_to_front() {
    blitz_shell::window_control::show_and_focus();
    #[cfg(windows)]
    imp::bring_to_front();
}

/// The window's inner size in CSS pixels, e.g. to keep menus on screen.
pub fn client_size() -> Option<(f64, f64)> {
    #[cfg(windows)]
    return imp::client_size();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    };
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClientRect, GetWindowThreadProcessId, IsWindowVisible,
    };
    use windows::core::BOOL;

    /// 0x00BBGGRR from a #RRGGBB value.
    fn colorref(rgb: u32) -> COLORREF {
        COLORREF(((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff))
    }

    extern "system" fn collect(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut pid = 0;
        // SAFETY: plain window queries; `found` points to a live Vec below.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() && IsWindowVisible(hwnd).as_bool() {
                (*(found.0 as *mut Vec<HWND>)).push(hwnd);
            }
        }
        BOOL(1)
    }

    fn own_windows() -> Vec<HWND> {
        let mut windows: Vec<HWND> = Vec::new();
        // SAFETY: the callback only runs during this call.
        let _ = unsafe { EnumWindows(Some(collect), LPARAM(&raw mut windows as isize)) };
        windows
    }

    pub fn is_focused() -> bool {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        // SAFETY: plain window queries.
        unsafe {
            let fg = GetForegroundWindow();
            let mut pid = 0;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            pid == std::process::id()
        }
    }

    pub fn flash() {
        use windows::Win32::UI::WindowsAndMessaging::{FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx};
        for hwnd in own_windows() {
            let info = FLASHWINFO {
                cbSize: size_of::<FLASHWINFO>() as u32,
                hwnd,
                dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
                uCount: 0,
                dwTimeout: 0,
            };
            // SAFETY: a correctly sized struct for our own window.
            unsafe {
                let _ = FlashWindowEx(&info);
            }
        }
    }

    pub fn bring_to_front() {
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow};
        if let Some(hwnd) = own_windows().into_iter().next() {
            // SAFETY: plain calls on our own window.
            unsafe {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    pub fn client_size() -> Option<(f64, f64)> {
        let hwnd = *own_windows().first()?;
        let mut rect = RECT::default();
        // SAFETY: plain queries on our own window.
        let (ok, dpi) = unsafe { (GetClientRect(hwnd, &mut rect).is_ok(), GetDpiForWindow(hwnd)) };
        if !ok || dpi == 0 {
            return None;
        }
        let scale = f64::from(dpi) / 96.0;
        Some((f64::from(rect.right - rect.left) / scale, f64::from(rect.bottom - rect.top) / scale))
    }

    pub fn apply(dark: bool) -> bool {
        let windows = own_windows();
        let (caption, text) = if dark { (0x0e0f13, 0xebecf0) } else { (0xe4e6ec, 0x15161c) };
        for hwnd in &windows {
            let dark_mode = BOOL(dark as i32);
            // SAFETY: attributes are passed with their exact sizes.
            unsafe {
                let _ = DwmSetWindowAttribute(*hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const dark_mode).cast(), 4);
                let c = colorref(caption);
                let t = colorref(text);
                let _ = DwmSetWindowAttribute(*hwnd, DWMWA_CAPTION_COLOR, (&raw const c).cast(), 4);
                let _ = DwmSetWindowAttribute(*hwnd, DWMWA_TEXT_COLOR, (&raw const t).cast(), 4);
            }
        }
        !windows.is_empty()
    }
}
