//! Windows build steps:
//! - Embed the app icon and version info, so Explorer, the taskbar and
//!   Alt+Tab show the Sivryn mark.
//! - Delay-load Media Foundation: Windows "N" editions ship without it, and
//!   `sivryn_media::media_foundation_available` guards every call into it.

fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target != "windows" {
        return;
    }
    println!("cargo:rerun-if-changed=assets/icons/sivryn.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/sivryn.ico");
        res.set("ProductName", "Sivryn");
        res.set("FileDescription", "Sivryn");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the app icon: {e}");
        }
    }
    if env == "msvc" {
        for dll in ["mfplat.dll", "mfreadwrite.dll", "mf.dll"] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
