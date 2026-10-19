//! Global hotkeys (push-to-talk, mute, deafen) that work while a game has
//! focus.
//!
//! A thread polls the key state every 10 ms instead of installing a system
//! keyboard hook: no hook means no input lag if Sivryn is busy and nothing for
//! anti-cheat software to object to. Mouse side buttons work too.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sivryn_media::AudioEngine;
use tokio::sync::mpsc;

use crate::settings::Keybinds;

/// A key or mouse button, as a Windows virtual-key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Keybind {
    pub vk: u16,
}

impl Keybind {
    pub fn label(self) -> String {
        match self.vk {
            0x04 => "Mausrad".into(),
            0x05 => "Maus 4".into(),
            0x06 => "Maus 5".into(),
            vk => key_name(vk).unwrap_or_else(|| format!("Taste {vk:#04x}")),
        }
    }
}

#[cfg(windows)]
fn key_name(vk: u16) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyNameTextW, MAPVK_VK_TO_VSC, MapVirtualKeyW};
    // SAFETY: plain lookups into the keyboard layout.
    unsafe {
        let scan = MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC);
        if scan == 0 {
            return None;
        }
        // Navigation keys share scan codes with the numpad; the extended bit picks the right name.
        let extended = matches!(vk, 0x21..=0x2E | 0x5B | 0x5C | 0xA3 | 0xA5 | 0x6F | 0x90);
        let lparam = ((scan as i32) << 16) | if extended { 1 << 24 } else { 0 };
        let mut buf = [0u16; 64];
        let len = GetKeyNameTextW(lparam, &mut buf);
        (len > 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(not(windows))]
fn key_name(_vk: u16) -> Option<String> {
    None
}

#[cfg(windows)]
fn is_down(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    // SAFETY: reads the asynchronous key state; no side effects.
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

#[cfg(not(windows))]
fn is_down(_vk: u16) -> bool {
    false
}

/// What the UI needs to react to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    ToggleMute,
    ToggleDeafen,
    /// A key was pressed while recording a new binding (`None`: Escape, i.e. cancel/clear).
    Captured(Option<Keybind>),
}

struct Shared {
    binds: Mutex<Keybinds>,
    capturing: AtomicBool,
}

/// Handle to the hotkey thread.
#[derive(Clone)]
pub struct Hotkeys {
    shared: Arc<Shared>,
}

impl Hotkeys {
    /// Starts polling. Push-to-talk goes straight to the audio engine; the
    /// other events come out of the returned channel.
    pub fn start(binds: Keybinds, engine: AudioEngine) -> (Self, mpsc::UnboundedReceiver<HotkeyEvent>) {
        let shared = Arc::new(Shared { binds: Mutex::new(binds), capturing: AtomicBool::new(false) });
        let (tx, rx) = mpsc::unbounded_channel();
        let thread_shared = shared.clone();
        let spawned =
            std::thread::Builder::new().name("sivryn-hotkeys".into()).spawn(move || poll(&thread_shared, &engine, &tx));
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start hotkey thread");
        }
        (Self { shared }, rx)
    }

    pub fn set_binds(&self, binds: Keybinds) {
        *self.shared.binds.lock().unwrap_or_else(|e| e.into_inner()) = binds;
    }

    /// The next key pressed is reported as [`HotkeyEvent::Captured`] instead of acting.
    pub fn capture_next(&self) {
        self.shared.capturing.store(true, Ordering::Relaxed);
    }

    pub fn cancel_capture(&self) {
        self.shared.capturing.store(false, Ordering::Relaxed);
    }
}

fn poll(shared: &Shared, engine: &AudioEngine, tx: &mpsc::UnboundedSender<HotkeyEvent>) {
    const ESCAPE: u16 = 0x1B;
    let mut was_down = [false; 256];
    loop {
        let binds = shared.binds.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let capturing = shared.capturing.load(Ordering::Relaxed);
        let mut pressed = None;

        // While capturing, watch every key except the left/right mouse buttons
        // (they're needed to click around); otherwise only the bound ones.
        let keys: Vec<u16> = if capturing {
            (0x04..0xFF).collect()
        } else {
            [binds.push_to_talk, binds.toggle_mute, binds.toggle_deafen].into_iter().flatten().map(|k| k.vk).collect()
        };
        for vk in keys {
            let down = is_down(vk);
            let i = vk as usize & 0xFF;
            if down && !was_down[i] && pressed.is_none() {
                pressed = Some(vk);
            }
            was_down[i] = down;
        }

        if capturing {
            // Ignore generic modifier codes; their left/right variants come through too.
            if let Some(vk) = pressed.filter(|vk| !matches!(vk, 0x10..=0x12)) {
                shared.capturing.store(false, Ordering::Relaxed);
                let bind = (vk != ESCAPE).then_some(Keybind { vk });
                if tx.send(HotkeyEvent::Captured(bind)).is_err() {
                    return;
                }
            }
        } else {
            let ptt = binds.push_to_talk.is_some_and(|k| is_down(k.vk));
            engine.set_push_to_talk(ptt);
            let event = match pressed {
                Some(vk) if binds.toggle_mute.is_some_and(|k| k.vk == vk) => Some(HotkeyEvent::ToggleMute),
                Some(vk) if binds.toggle_deafen.is_some_and(|k| k.vk == vk) => Some(HotkeyEvent::ToggleDeafen),
                _ => None,
            };
            if let Some(event) = event
                && tx.send(event).is_err()
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_buttons_have_names() {
        assert_eq!(Keybind { vk: 0x05 }.label(), "Maus 4");
        assert_eq!(Keybind { vk: 0x06 }.label(), "Maus 5");
        // Letters resolve through the keyboard layout.
        #[cfg(windows)]
        assert_eq!(Keybind { vk: 0x56 }.label(), "V");
    }
}
