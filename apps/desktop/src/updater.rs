//! Installation and updates through Velopack.
//!
//! The release workflow packs each version with Velopack (a `Setup.exe`, a
//! full package and a delta package per release) and signs every release
//! feed (`releases.win.json`) with Sivryn's own Ed25519 key. The client only
//! takes feeds whose signature checks out with the public key below; Velopack
//! then verifies each downloaded package against the SHA-256 in that feed. So
//! nobody who can merely change the GitHub release can ship an update.
//!
//! Pre-releases are offered too (everything is alpha for now). A downloaded
//! update is installed when the user restarts from the app, or at the next
//! start at the latest.

use std::path::Path;
use std::sync::mpsc::Sender;

use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use velopack::sources::{GithubSource, UpdateSource};
use velopack::{UpdateCheck, UpdateManager, VelopackAsset, VelopackAssetFeed, bundle::Manifest};

const REPO: &str = "https://github.com/RTBGG/Sivryn";
const RELEASES_API: &str = "https://api.github.com/repos/RTBGG/Sivryn/releases?per_page=10";
/// Public half of the key the release workflow signs feeds with.
const PUBLIC_KEY: [u8; 32] = [
    135, 229, 253, 100, 168, 8, 217, 0, 55, 18, 197, 229, 51, 227, 4, 193, 241, 207, 202, 164, 196, 180, 122, 126, 67,
    180, 91, 85, 68, 122, 52, 232,
];

/// Runs Velopack's install, update and uninstall hooks. Must come first in
/// `main`: during those hooks the process exits here.
pub fn run_hooks() {
    velopack::VelopackApp::build().on_before_uninstall_fast_callback(|_| crate::autostart::remove()).run();
}

/// The version that is running.
pub fn current_version() -> String {
    manager().map(|m| m.get_current_version_as_string()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

/// Whether this copy was installed (only then it can update itself; a build
/// run from `target` can't).
pub fn is_installed() -> bool {
    manager().is_some()
}

fn manager() -> Option<UpdateManager> {
    UpdateManager::new(SignedSource { downloads: GithubSource::new(REPO, None, true) }, None, None).ok()
}

/// At start: an update downloaded earlier is installed now (the app restarts
/// into the new version right away).
pub fn apply_pending() {
    let Some(manager) = manager() else { return };
    if let Some(pending) = manager.get_update_pending_restart() {
        tracing::info!(version = %pending.Version, "installing the downloaded update");
        if let Err(e) = manager.apply_updates_and_restart(&pending) {
            tracing::warn!(error = %e, "could not install the downloaded update");
        }
    }
}

/// What a check found.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    UpToDate,
    /// Downloaded and ready; installed on restart.
    Ready(String),
}

/// Looks for a newer version and downloads it, reporting progress in percent.
/// Blocking: run it off the UI thread.
pub fn check_and_download(progress: impl Fn(u8) + Send + 'static) -> Result<Found, String> {
    let manager = manager().ok_or("not installed")?;
    if let Some(pending) = manager.get_update_pending_restart() {
        return Ok(Found::Ready(pending.Version));
    }
    let update = match manager.check_for_updates().map_err(|e| e.to_string())? {
        UpdateCheck::UpdateAvailable(update) => update,
        UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => return Ok(Found::UpToDate),
    };
    let version = update.TargetFullRelease.Version.clone();
    tracing::info!(%version, delta = !update.DeltasToTarget.is_empty(), "downloading update");
    let (tx, rx) = std::sync::mpsc::channel::<i16>();
    let reporter = std::thread::spawn(move || {
        for p in rx {
            progress(p.clamp(0, 100) as u8);
        }
    });
    let result = manager.download_updates(&update, Some(tx)).map_err(|e| e.to_string());
    let _ = reporter.join();
    result.map(|()| Found::Ready(version))
}

/// Restarts into the downloaded update.
pub fn restart_now() -> Result<(), String> {
    let manager = manager().ok_or("not installed")?;
    let pending = manager.get_update_pending_restart().ok_or("no update downloaded")?;
    manager.apply_updates_and_restart(&pending).map_err(|e| e.to_string())
}

/// GitHub releases, but only feeds signed with Sivryn's key count.
struct SignedSource {
    downloads: GithubSource,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn get(url: &str) -> Result<Vec<u8>, velopack::Error> {
    let mut response = ureq::get(url)
        .header("User-Agent", concat!("sivryn-desktop/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| velopack::Error::Other(format!("{url}: {e}")))?;
    response.body_mut().read_to_vec().map_err(|e| velopack::Error::Other(format!("{url}: {e}")))
}

/// Whether `signature` (64 raw bytes) signs `data` with Sivryn's key.
fn signed(data: &[u8], signature: &[u8]) -> bool {
    let (Ok(key), Ok(signature)) = (VerifyingKey::from_bytes(&PUBLIC_KEY), Signature::from_slice(signature)) else {
        return false;
    };
    key.verify_strict(data, &signature).is_ok()
}

/// The assets of a signed feed; packages without a SHA-256 can't be checked
/// against it and are left out.
fn verified_assets(feed: &[u8], signature: &[u8]) -> Option<Vec<VelopackAsset>> {
    if !signed(feed, signature) {
        return None;
    }
    let feed: VelopackAssetFeed = serde_json::from_slice(feed).ok()?;
    Some(feed.Assets.into_iter().filter(|a| !a.SHA256.is_empty()).collect())
}

impl UpdateSource for SignedSource {
    fn get_release_feed(
        &self,
        channel: &str,
        _app: &Manifest,
        _user: &str,
    ) -> Result<VelopackAssetFeed, velopack::Error> {
        let releases: Vec<Release> = serde_json::from_slice(&get(RELEASES_API)?)?;
        let feed_name = format!("releases.{channel}.json");
        let signature_name = format!("{feed_name}.sig");
        let mut assets = Vec::new();
        for release in releases.iter().filter(|r| !r.draft) {
            let find = |name: &str| release.assets.iter().find(|a| a.name.eq_ignore_ascii_case(name));
            let (Some(feed), Some(signature)) = (find(&feed_name), find(&signature_name)) else { continue };
            let (feed, signature) = (get(&feed.browser_download_url)?, get(&signature.browser_download_url)?);
            match verified_assets(&feed, &signature) {
                Some(verified) => assets.extend(verified),
                None => tracing::warn!("ignoring a release feed with an invalid signature"),
            }
        }
        Ok(VelopackAssetFeed { Assets: assets })
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        file: &Path,
        progress: Option<Sender<i16>>,
    ) -> Result<(), velopack::Error> {
        // Velopack checks the package against the SHA-256 of the signed feed.
        self.downloads.download_release_entry(asset, file, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Signed with the release key: `openssl pkeyutl -sign -rawin`, as in the
    // release workflow.
    const FEED: &[u8] = include_bytes!("../tests/fixtures/releases.win.json");
    const SIGNATURE: &[u8] = include_bytes!("../tests/fixtures/releases.win.json.sig");

    #[test]
    fn only_feeds_signed_with_the_release_key_count() {
        let assets = verified_assets(FEED, SIGNATURE).expect("signature checks out");
        // The fixture's asset without a SHA-256 is left out.
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].Version, "0.1.0-alpha.12");

        let mut tampered = FEED.to_vec();
        let at = tampered.iter().position(|b| *b == b'1').unwrap();
        tampered[at] = b'9';
        assert!(verified_assets(&tampered, SIGNATURE).is_none());
        assert!(verified_assets(FEED, &[0u8; 64]).is_none());
        assert!(verified_assets(FEED, b"short").is_none());
    }
}
