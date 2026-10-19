//! Windows "N" editions ship without Media Foundation. Delay-loading its DLLs
//! lets the app start there anyway; `sivryn_media::media_foundation_available`
//! guards every call into them.

fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target == "windows" && env == "msvc" {
        for dll in ["mfplat.dll", "mfreadwrite.dll", "mf.dll"] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
