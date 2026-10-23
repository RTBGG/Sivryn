//! Call buttons in the taskbar's window preview (camera, microphone,
//! headphones, hang up), like Discord's. They exist only during a call.
//!
//! Windows sends their clicks to the main window, so its window procedure
//! is subclassed; clicks go to the app as `tray::ShellEvent`s. The buttons
//! must be added again whenever the taskbar button is recreated (the window
//! was hidden to the notification area and shown again, or Explorer
//! restarted).

/// What the buttons show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallButtons {
    pub camera: bool,
    pub muted: bool,
    pub deafened: bool,
}

/// Labels of the buttons (in the app's language).
#[derive(Clone)]
pub struct Labels {
    pub camera_on: String,
    pub camera_off: String,
    pub mute: String,
    pub unmute: String,
    pub deafen: String,
    pub undeafen: String,
    pub hang_up: String,
}

/// Shows the buttons for a call (or hides them with `None`). Call it on the
/// UI thread; it is cheap when nothing changed.
pub fn update(call: Option<CallButtons>, labels: &Labels) {
    #[cfg(windows)]
    imp::update(call, labels);
    #[cfg(not(windows))]
    let _ = (call, labels);
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{
        DefSubclassProc, ITaskbarList3, SetWindowSubclass, THB_FLAGS, THB_ICON, THB_TOOLTIP, THBF_ENABLED, THBF_HIDDEN,
        THBN_CLICKED, THUMBBUTTON, TaskbarList,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, GetSystemMetrics, HICON, RegisterWindowMessageW, SM_CXSMICON, WM_COMMAND,
    };
    use windows::core::w;

    use super::{CallButtons, Labels};
    use crate::native_icon;
    use crate::tray::{ShellEvent, send};

    const CAMERA: u32 = 1;
    const MIC: u32 = 2;
    const HEADPHONES: u32 = 3;
    const HANG_UP: u32 = 4;
    const WHITE: [u8; 3] = [240, 240, 244];
    const RED: [u8; 3] = [242, 63, 67];

    struct Taskbar {
        list: ITaskbarList3,
        window: HWND,
        /// Whether the buttons were added to the current taskbar button.
        added: bool,
        shown: Option<Option<CallButtons>>,
        call: Option<CallButtons>,
        labels: Option<Labels>,
        icons: Vec<HICON>,
        created_message: u32,
    }

    thread_local! {
        static TASKBAR: RefCell<Option<Taskbar>> = const { RefCell::new(None) };
    }

    fn init() -> Option<Taskbar> {
        let window = HWND(*blitz_shell::window_control::native_handles().first()? as *mut _);
        // SAFETY: COM is set up on the UI thread by winit; the subclass lives
        // as long as the window.
        unsafe {
            let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?;
            list.HrInit().ok()?;
            let created_message = RegisterWindowMessageW(w!("TaskbarButtonCreated"));
            let _ = SetWindowSubclass(window, Some(subclass), 0x5157, 0);
            Some(Taskbar {
                list,
                window,
                added: false,
                shown: None,
                call: None,
                labels: None,
                icons: Vec::new(),
                created_message,
            })
        }
    }

    fn icon(symbol: char, color: [u8; 3], off: bool) -> HICON {
        // SAFETY: a plain system metric.
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 48) as u32;
        let mut image = native_icon::glyph(symbol, size, color);
        if off {
            native_icon::strike(&mut image, color);
        }
        native_icon::to_hicon(&image).unwrap_or_default()
    }

    fn tip(text: &str) -> [u16; 260] {
        let mut tip = [0u16; 260];
        for (t, c) in tip.iter_mut().zip(text.encode_utf16().take(259)) {
            *t = c;
        }
        tip
    }

    /// Puts the current state on the buttons (adding them if needed).
    fn apply(tb: &mut Taskbar) {
        let Some(labels) = tb.labels.clone() else { return };
        let call = tb.call;
        if tb.added && tb.shown == Some(call) {
            return;
        }
        let state = call.unwrap_or(CallButtons { camera: false, muted: false, deafened: false });
        let flags = if call.is_some() { THBF_ENABLED } else { THBF_ENABLED | THBF_HIDDEN };
        let icons = [
            icon('\u{E714}', WHITE, !state.camera),
            icon(if state.muted { '\u{F781}' } else { '\u{E720}' }, WHITE, false),
            icon('\u{E7F6}', WHITE, state.deafened),
            icon('\u{E778}', RED, false),
        ];
        let tips = [
            if state.camera { &labels.camera_off } else { &labels.camera_on },
            if state.muted { &labels.unmute } else { &labels.mute },
            if state.deafened { &labels.undeafen } else { &labels.deafen },
            &labels.hang_up,
        ];
        let buttons: Vec<THUMBBUTTON> = [CAMERA, MIC, HEADPHONES, HANG_UP]
            .iter()
            .zip(icons)
            .zip(tips)
            .map(|((id, hicon), text)| THUMBBUTTON {
                dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
                iId: *id,
                hIcon: hicon,
                szTip: tip(text),
                dwFlags: flags,
                ..Default::default()
            })
            .collect();
        // SAFETY: our own window; the taskbar copies the icons.
        let result = unsafe {
            if tb.added {
                tb.list.ThumbBarUpdateButtons(tb.window, &buttons)
            } else {
                tb.list.ThumbBarAddButtons(tb.window, &buttons)
            }
        };
        // The previous icons are no longer shown.
        for old in tb.icons.drain(..) {
            // SAFETY: icons made here and replaced on the buttons.
            unsafe {
                let _ = DestroyIcon(old);
            }
        }
        tb.icons = buttons.iter().map(|b| b.hIcon).collect();
        match result {
            Ok(()) => {
                tb.added = true;
                tb.shown = Some(call);
            }
            // No taskbar button yet (hidden window): "TaskbarButtonCreated" comes later.
            Err(e) => tracing::debug!(error = %e, "taskbar buttons not added yet"),
        }
    }

    pub fn update(call: Option<CallButtons>, labels: &Labels) {
        TASKBAR.with(|t| {
            let mut t = t.borrow_mut();
            if t.is_none() {
                *t = init();
            }
            let Some(tb) = t.as_mut() else { return };
            tb.call = call;
            if tb.labels.is_none() {
                tb.labels = Some(labels.clone());
            }
            apply(tb);
        });
    }

    extern "system" fn subclass(
        window: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if msg == WM_COMMAND && (wparam.0 >> 16) as u32 == THBN_CLICKED {
            match (wparam.0 & 0xFFFF) as u32 {
                CAMERA => send(ShellEvent::ToggleCamera),
                MIC => send(ShellEvent::ToggleMute),
                HEADPHONES => send(ShellEvent::ToggleDeafen),
                HANG_UP => send(ShellEvent::HangUp),
                _ => {}
            }
            return LRESULT(0);
        }
        // A new taskbar button: the buttons have to be added to it again.
        let recreated = TASKBAR.with(|t| {
            t.try_borrow_mut().ok().and_then(|mut t| {
                let tb = t.as_mut()?;
                (msg == tb.created_message).then(|| {
                    tb.added = false;
                    apply(tb);
                })
            })
        });
        let _ = recreated;
        // SAFETY: everything else goes on to the window's own procedure.
        unsafe { DefSubclassProc(window, msg, wparam, lparam) }
    }
}
