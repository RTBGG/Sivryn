//! What Sivryn leaves behind when something goes wrong: a log file of the
//! current run (`%LOCALAPPDATA%\Sivryn\sivryn.log`, the run before in
//! `sivryn.old.log`), crash reports in `crash.log` (Rust panics and native
//! crashes alike), and a message box when Sivryn fails before its window
//! appeared, so a failed start isn't just a spinning cursor.
//!
//! Release builds have no console: without these, a start that dies early
//! leaves no trace at all.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::i18n::t;

/// Set once the window has been shown: from then on errors show in the app.
static WINDOW_UP: AtomicBool = AtomicBool::new(false);

/// `%LOCALAPPDATA%\Sivryn` (or the temp directory).
pub fn dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Sivryn")
}

fn log_path() -> PathBuf {
    dir().join("sivryn.log")
}

/// Logs to stdout and the log file, and records crashes. Call first.
pub fn init() {
    let _ = std::fs::create_dir_all(dir());
    let path = log_path();
    // A second start (handing over to the running Sivryn) appends its few
    // lines to that one's log instead of moving it away.
    if !another_instance_runs() {
        let _ = std::fs::rename(&path, dir().join("sivryn.old.log"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok().map(Mutex::new);
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,sivryn_desktop=info,sivryn_media=info,sivryn_client_core=info".into()),
        )
        .with_ansi(false)
        .with_writer(Tee(file.map(|f| &*Box::leak(Box::new(f)))))
        .init();
    install_panic_hook();
    #[cfg(windows)]
    imp::install_exception_filter();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = %os_version(),
        args = ?std::env::args().skip(1).collect::<Vec<_>>(),
        "starting"
    );
}

/// The window is up (called from the app's first render).
pub fn window_shown() {
    if !WINDOW_UP.swap(true, Ordering::Relaxed) {
        tracing::info!("window shown");
    }
}

/// Tells the user that Sivryn couldn't start, with `reason` and where the log is.
pub fn startup_failed(reason: &str) {
    tracing::error!(reason, "start failed");
    let text = format!("{}\n\n{reason}\n\n{} {}", t("startup.failed"), t("startup.log_hint"), dir().display());
    message_box(t("startup.failed_title"), &text);
}

pub fn message_box(title: &str, text: &str) {
    #[cfg(windows)]
    imp::message_box(title, text);
    #[cfg(not(windows))]
    eprintln!("{title}: {text}");
}

/// Writes every log line to stdout and the log file.
struct Tee(Option<&'static Mutex<std::fs::File>>);

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TeeWriter(self.0)
    }
}

struct TeeWriter(Option<&'static Mutex<std::fs::File>>);

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stdout().write_all(buf);
        if let Some(file) = self.0 {
            // Written right away: the next moment may be a crash.
            let _ = file.lock().unwrap_or_else(|e| e.into_inner()).write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn append_crash_report(report: &str) {
    let path = dir().join("crash.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(report.as_bytes());
    }
}

fn unix_time() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = format!(
            "--- sivryn-desktop {} crashed (unix time {})\n{info}\n{}\n",
            env!("CARGO_PKG_VERSION"),
            unix_time(),
            std::backtrace::Backtrace::force_capture(),
        );
        append_crash_report(&report);
        tracing::error!("{info}");
        default_hook(info);
        if !WINDOW_UP.load(Ordering::Relaxed) {
            let reason = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| info.to_string());
            startup_failed(&reason);
        }
    }));
}

/// Whether a Sivryn already runs (its single-instance mutex exists, see `tray.rs`).
fn another_instance_runs() -> bool {
    #[cfg(windows)]
    return imp::another_instance_runs();
    #[cfg(not(windows))]
    false
}

fn os_version() -> String {
    #[cfg(windows)]
    return imp::os_version();
    #[cfg(not(windows))]
    std::env::consts::OS.to_owned()
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Diagnostics::Debug::{
        EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
    };
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};
    use windows::core::PCWSTR;

    use super::{WINDOW_UP, append_crash_report, unix_time};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn message_box(title: &str, text: &str) {
        let (title, text) = (wide(title), wide(text));
        // SAFETY: NUL-terminated strings that outlive the call.
        unsafe {
            MessageBoxW(
                None::<HWND>,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            );
        }
    }

    /// Native crashes (e.g. in a graphics driver) skip the panic hook: write
    /// what is known to `crash.log`, tell the user if there's no window yet,
    /// and let Windows Error Reporting go on.
    pub fn install_exception_filter() {
        unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
            // SAFETY: Windows passes valid exception pointers to the filter.
            let (code, address) = unsafe {
                info.as_ref()
                    .and_then(|i| i.ExceptionRecord.as_ref())
                    .map_or((0, 0), |r| (r.ExceptionCode.0 as u32, r.ExceptionAddress as usize))
            };
            let module = module_of(address);
            append_crash_report(&format!(
                "--- sivryn-desktop {} crashed natively (unix time {}): exception {code:#010x} at {address:#x} in {module}\n",
                env!("CARGO_PKG_VERSION"),
                unix_time(),
            ));
            if !WINDOW_UP.load(std::sync::atomic::Ordering::Relaxed) {
                super::startup_failed(&format!("Exception {code:#010x} in {module}"));
            }
            // Windows Error Reporting takes it from here.
            EXCEPTION_CONTINUE_SEARCH
        }
        // SAFETY: installs a process-wide filter function with the right signature.
        unsafe {
            SetUnhandledExceptionFilter(Some(filter));
        }
    }

    /// The file name of the module containing `address` (e.g. a driver DLL).
    fn module_of(address: usize) -> String {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
            GetModuleHandleExW,
        };
        let mut module = HMODULE::default();
        let mut name = [0u16; 260];
        // SAFETY: looks up the module of an address without changing its refcount.
        let len = unsafe {
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(address as *const u16),
                &mut module,
            )
            .is_err()
            {
                return "an unknown module".into();
            }
            GetModuleFileNameW(Some(module), &mut name)
        };
        let path = String::from_utf16_lossy(&name[..len as usize]);
        path.rsplit('\\').next().unwrap_or(&path).to_owned()
    }

    pub fn another_instance_runs() -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
        // SAFETY: opens (and closes again) a named mutex if it exists.
        unsafe {
            match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, windows::core::w!("Local\\Sivryn.Desktop.Instance")) {
                Ok(handle) => {
                    let _ = CloseHandle(handle);
                    true
                }
                Err(_) => false,
            }
        }
    }

    pub fn os_version() -> String {
        use windows::Wdk::System::SystemServices::RtlGetVersion;
        use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
        let mut info = OSVERSIONINFOW { dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
        // SAFETY: a correctly sized struct; RtlGetVersion doesn't lie about the version like GetVersionEx.
        let _ = unsafe { RtlGetVersion(&mut info) };
        // Windows 11 still reports 10.0; the build tells them apart (22000+ is 11).
        format!("Windows {}.{} build {}", info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber)
    }
}
