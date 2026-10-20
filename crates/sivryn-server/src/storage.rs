//! Uploaded files on disk, one file per ID under `files_dir`.

use std::path::{Path, PathBuf};

use sivryn_protocol::FileId;

fn path(dir: &Path, id: FileId) -> PathBuf {
    // UUIDs are safe file names; two-level fan-out keeps directories small.
    let name = id.0.simple().to_string();
    dir.join(&name[..2]).join(name)
}

pub async fn save(dir: &Path, id: FileId, bytes: &[u8]) -> std::io::Result<()> {
    let path = path(dir, id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("part");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, &path).await
}

pub async fn load(dir: &Path, id: FileId) -> std::io::Result<Vec<u8>> {
    tokio::fs::read(path(dir, id)).await
}

pub async fn remove(dir: &Path, id: FileId) {
    let _ = tokio::fs::remove_file(path(dir, id)).await;
}
