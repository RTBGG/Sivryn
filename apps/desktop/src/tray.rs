//! The icon in the notification area: the Sivryn mark with the own status
//! (or, in a call, a green bubble that fills while you speak), a menu with
//! Open, Check for updates and Quit, and a click to bring the window back.
//!
//! It runs on a thread of its own with a hidden window for the shell's
//! messages. The app sets what it shows ([`set_badge`]) and gets clicks as
//! [`ShellEvent`]s, as from the taskbar buttons. A second start of Sivryn
//! asks the running one to show its window instead ([`already_running`]).

use std::sync::{Mutex, OnceLock};

/// Something the user did outside the window (tray menu, taskbar buttons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    Open,
    CheckForUpdates,
    Quit,
    ToggleMute,
    ToggleDeafen,
    ToggleCamera,
    HangUp,
    /// A `sivryn://` invite arrived from another start (see `links::take`).
    Invite,
}

/// What the tray icon's corner shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    Online,
    Idle,
    Busy,
    Offline,
    /// In a call: a green bubble, filled while speaking.
    Voice {
        speaking: bool,
    },
}

impl Badge {
    fn color(self) -> [u8; 3] {
        match self {
            Badge::Online | Badge::Voice { .. } => [35, 165, 90],
            Badge::Idle => [240, 178, 50],
            Badge::Busy => [242, 63, 67],
            Badge::Offline => [128, 132, 142],
        }
    }
}

/// Labels of the tray menu (in the app's language).
#[derive(Clone)]
pub struct Labels {
    pub open: String,
    pub check_updates: String,
    pub quit: String,
}

static EVENTS: OnceLock<tokio::sync::mpsc::UnboundedSender<ShellEvent>> = OnceLock::new();
static BADGE: Mutex<Badge> = Mutex::new(Badge::Online);
static LABELS: OnceLock<Labels> = OnceLock::new();

/// Hands an event to the app.
pub fn send(event: ShellEvent) {
    tracing::debug!(?event, "tray event sent");
    if let Some(tx) = EVENTS.get() {
        let _ = tx.send(event);
    }
}

/// Sets what the icon shows (picked up within a tenth of a second).
pub fn set_badge(badge: Badge) {
    *BADGE.lock().unwrap_or_else(|e| e.into_inner()) = badge;
}

/// Starts the tray icon; events arrive on the returned channel.
pub fn start(labels: Labels) -> tokio::sync::mpsc::UnboundedReceiver<ShellEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let _ = EVENTS.set(tx);
    let _ = LABELS.set(labels);
    #[cfg(windows)]
    if let Err(e) = std::thread::Builder::new().name("sivryn-tray".into()).spawn(imp::run) {
        tracing::warn!(error = %e, "could not start the tray icon");
    }
    rx
}

/// Takes the icon away (before quitting: an icon left behind stays until
/// the mouse passes over it).
pub fn remove() {
    #[cfg(windows)]
    imp::remove();
}

/// Whether Sivryn already runs; if so, it was asked to show its window and
/// this process should end.
pub fn already_running(link: Option<&str>) -> bool {
    #[cfg(windows)]
    return imp::already_running(link);
    #[cfg(not(windows))]
    {
        let _ = link;
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        ASFW_ANY, AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
        DestroyMenu, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics, HICON, MF_SEPARATOR,
        MF_STRING, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SendMessageW,
        SetForegroundWindow, SetTimer, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
        WINDOW_EX_STYLE, WM_APP, WM_COPYDATA, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    use super::{BADGE, Badge, LABELS, ShellEvent, send};
    use crate::native_icon;

    const CLASS: PCWSTR = w!("SivrynTray");
    /// The shell's notifications about the icon.
    const WM_TRAY: u32 = WM_APP + 1;
    /// Another Sivryn start asks to show the window.
    const WM_SHOW: u32 = WM_APP + 2;
    /// `WM_COPYDATA` tag of a `sivryn://` link from another start.
    const COPY_LINK: usize = 0x5356_4c4b;
    const MENU_OPEN: usize = 1;
    const MENU_UPDATES: usize = 2;
    const MENU_QUIT: usize = 3;

    /// "TaskbarCreated": Explorer restarted and the icon must be added again.
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
    /// The hidden window the icon belongs to.
    static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    pub fn remove() {
        let window = WINDOW.load(Ordering::Relaxed);
        if window != 0 {
            // SAFETY: removes our own icon; harmless if it is already gone.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data(HWND(window as *mut _)));
            }
        }
    }

    pub fn already_running(link: Option<&str>) -> bool {
        // SAFETY: a named mutex held for the life of the process; the shell
        // window of the running instance gets a posted message.
        unsafe {
            let _mutex = CreateMutexW(None, true, w!("Local\\Sivryn.Desktop.Instance"));
            if GetLastError() != ERROR_ALREADY_EXISTS {
                // Kept on purpose: it marks this process until it ends.
                std::mem::forget(_mutex);
                return false;
            }
            if let Ok(window) = FindWindowW(CLASS, PCWSTR::null()) {
                // The running one may bring its window to the front.
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                if let Some(link) = link {
                    // Copied into the receiving process while this call waits.
                    let data = COPYDATASTRUCT {
                        dwData: COPY_LINK,
                        cbData: link.len() as u32,
                        lpData: link.as_ptr() as *mut _,
                    };
                    SendMessageW(window, WM_COPYDATA, Some(WPARAM(0)), Some(LPARAM(&data as *const _ as isize)));
                }
                let _ = PostMessageW(Some(window), WM_SHOW, WPARAM(0), LPARAM(0));
            }
            true
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn icon(badge: Badge) -> Option<HICON> {
        // SAFETY: a plain system metric.
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as u32;
        let mut image = native_icon::logo(size);
        match badge {
            Badge::Voice { speaking } => native_icon::badge(&mut image, badge.color(), size as f32 * 0.3, speaking),
            _ => native_icon::badge(&mut image, badge.color(), size as f32 * 0.22, true),
        }
        native_icon::to_hicon(&image)
    }

    fn data(window: HWND) -> NOTIFYICONDATAW {
        let mut tip = [0u16; 128];
        for (t, c) in tip.iter_mut().zip("Sivryn".encode_utf16()) {
            *t = c;
        }
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            szTip: tip,
            ..Default::default()
        }
    }

    /// Adds the icon (or replaces its picture) with the current badge.
    /// Returns the badge shown, or `None` if the shell didn't take it.
    fn show(window: HWND, add: bool) -> Option<Badge> {
        let badge = *BADGE.lock().unwrap_or_else(|e| e.into_inner());
        let mut nid = data(window);
        let hicon = icon(badge)?;
        nid.hIcon = hicon;
        // SAFETY: a filled-in structure for our own window; the icon is
        // copied by the shell and can be destroyed right after.
        unsafe {
            let ok = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &nid).as_bool();
            let _ = DestroyIcon(hicon);
            ok.then_some(badge)
        }
    }

    fn menu(window: HWND) {
        let Some(labels) = LABELS.get() else { return };
        // SAFETY: a popup menu owned and destroyed here; the window comes to
        // the front first so the menu closes when clicking elsewhere.
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let (open, updates, quit) = (wide(&labels.open), wide(&labels.check_updates), wide(&labels.quit));
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, PCWSTR(open.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, MENU_UPDATES, PCWSTR(updates.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, PCWSTR(quit.as_ptr()));
            let mut at = POINT::default();
            let _ = GetCursorPos(&mut at);
            let _ = SetForegroundWindow(window);
            let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, window, None);
            let _ = DestroyMenu(menu);
            match chosen.0 as usize {
                MENU_OPEN => send(ShellEvent::Open),
                MENU_UPDATES => send(ShellEvent::CheckForUpdates),
                MENU_QUIT => send(ShellEvent::Quit),
                _ => {}
            }
        }
    }

    thread_local! {
        static SHOWN: std::cell::Cell<Option<Badge>> = const { std::cell::Cell::new(None) };
    }

    extern "system" fn proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TRAY => match lparam.0 as u32 {
                WM_LBUTTONUP => send(ShellEvent::Open),
                WM_RBUTTONUP => menu(window),
                _ => {}
            },
            WM_SHOW => send(ShellEvent::Open),
            WM_COPYDATA => {
                // SAFETY: WM_COPYDATA's lParam points to a COPYDATASTRUCT valid during this call.
                let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
                if data.dwData == COPY_LINK && !data.lpData.is_null() {
                    // SAFETY: cbData bytes at lpData, as sent by `already_running`.
                    let bytes = unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
                    if let Some(invite) = std::str::from_utf8(bytes).ok().and_then(crate::links::parse) {
                        crate::links::set_pending(invite);
                        send(ShellEvent::Invite);
                    }
                }
                return LRESULT(1);
            }
            WM_TIMER => {
                let wanted = *BADGE.lock().unwrap_or_else(|e| e.into_inner());
                match SHOWN.with(|s| s.get()) {
                    // Not in the notification area yet (the shell wasn't
                    // ready, e.g. right after signing in): try again.
                    None => SHOWN.with(|s| s.set(show(window, true))),
                    Some(shown) if shown != wanted => {
                        if let Some(shown) = show(window, false) {
                            SHOWN.with(|s| s.set(Some(shown)));
                        }
                    }
                    Some(_) => {}
                }
            }
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                SHOWN.with(|s| s.set(show(window, true)));
            }
            // SAFETY: default handling for everything else.
            _ => return unsafe { DefWindowProcW(window, msg, wparam, lparam) },
        }
        LRESULT(0)
    }

    pub fn run() {
        // SAFETY: a window class and hidden window owned by this thread, with
        // its message loop until the process ends.
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else { return };
            TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: module.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassW(&class);
            let Ok(window) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                w!("Sivryn"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module.into()),
                None,
            ) else {
                return;
            };
            WINDOW.store(window.0 as isize, Ordering::Relaxed);
            SHOWN.with(|s| s.set(show(window, true)));
            SetTimer(Some(window), 1, 100, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = Shell_NotifyIconW(NIM_DELETE, &data(window));
        }
    }
}
