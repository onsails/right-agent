//! Guest filesystem access through the SDK's fs API.
//!
//! The microVM boundary replaces landlock: there are no host bind mounts, so
//! every byte crosses via this API (large payloads chunk internally at the
//! SDK platform deployment writes `.platform` as root and removes write bits,
//! which protects its contents from direct mutation by the guest user. Its
//! guest-owned `/sandbox` parent still permits replacement of the entry, so
//! security-authoritative runtime storage lives under root-owned `/opt`.

use std::path::Path;

use microsandbox::sandbox::{FsEntry, FsEntryKind as SdkFsEntryKind, FsMetadata};

use crate::error::SandboxError;
use crate::handle::SandboxHandle;

/// Kind of a guest filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsEntryKind {
    /// Regular file.
    File,

    /// Directory.
    Directory,

    /// Symbolic link.
    Symlink,

    /// Device, socket, etc.
    Other,
}

impl From<SdkFsEntryKind> for FsEntryKind {
    fn from(kind: SdkFsEntryKind) -> Self {
        match kind {
            SdkFsEntryKind::File => Self::File,
            SdkFsEntryKind::Directory => Self::Directory,
            SdkFsEntryKind::Symlink => Self::Symlink,
            SdkFsEntryKind::Other => Self::Other,
        }
    }
}

/// Metadata about a guest filesystem entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntryInfo {
    /// Guest path (the requested path for `fs_stat`).
    pub path: String,

    /// Entry kind.
    pub kind: FsEntryKind,

    /// Size in bytes.
    pub size: u64,

    /// Unix permission bits.
    pub mode: u32,

    /// Owner uid.
    pub uid: u32,

    /// Owner gid.
    pub gid: u32,
}

impl From<FsEntry> for FsEntryInfo {
    fn from(entry: FsEntry) -> Self {
        Self {
            path: entry.path,
            kind: entry.kind.into(),
            size: entry.size,
            mode: entry.mode,
            uid: entry.uid,
            gid: entry.gid,
        }
    }
}

impl FsEntryInfo {
    fn from_metadata(path: &str, metadata: FsMetadata) -> Self {
        Self {
            path: path.to_owned(),
            kind: metadata.kind.into(),
            size: metadata.size,
            mode: metadata.mode,
            uid: metadata.uid,
            gid: metadata.gid,
        }
    }
}

impl SandboxHandle {
    /// Read an entire guest file into memory.
    pub async fn fs_read(&self, path: &str) -> Result<Vec<u8>, SandboxError> {
        self.fs_op("fs read", self.sdk().fs().read(path))
            .await
            .map(|bytes| bytes.to_vec())
    }

    /// Read an entire guest file as UTF-8.
    pub async fn fs_read_to_string(&self, path: &str) -> Result<String, SandboxError> {
        self.fs_op("fs read", self.sdk().fs().read_to_string(path))
            .await
    }

    /// Write `data` to a guest file, creating it (not its parents) if needed.
    pub async fn fs_write(&self, path: &str, data: &[u8]) -> Result<(), SandboxError> {
        self.fs_op("fs write", self.sdk().fs().write(path, data))
            .await
    }

    /// List the immediate children of a guest directory (non-recursive).
    pub async fn fs_list(&self, path: &str) -> Result<Vec<FsEntryInfo>, SandboxError> {
        let entries = self.fs_op("fs list", self.sdk().fs().list(path)).await?;
        Ok(entries.into_iter().map(FsEntryInfo::from).collect())
    }

    /// Create a guest directory and its parents.
    pub async fn fs_mkdir(&self, path: &str) -> Result<(), SandboxError> {
        self.fs_op("fs mkdir", self.sdk().fs().mkdir(path)).await
    }

    /// Delete a single guest file.
    pub async fn fs_remove(&self, path: &str) -> Result<(), SandboxError> {
        self.fs_op("fs remove", self.sdk().fs().remove(path)).await
    }

    /// Delete a guest directory recursively.
    pub async fn fs_remove_dir(&self, path: &str) -> Result<(), SandboxError> {
        self.fs_op("fs remove_dir", self.sdk().fs().remove_dir(path))
            .await
    }

    /// Whether a guest path exists.
    pub async fn fs_exists(&self, path: &str) -> Result<bool, SandboxError> {
        self.fs_op("fs exists", self.sdk().fs().exists(path)).await
    }

    /// Stat a guest path.
    pub async fn fs_stat(&self, path: &str) -> Result<FsEntryInfo, SandboxError> {
        let metadata = self.fs_op("fs stat", self.sdk().fs().stat(path)).await?;
        Ok(FsEntryInfo::from_metadata(path, metadata))
    }

    /// Copy a host file into the guest.
    pub async fn fs_copy_from_host(
        &self,
        host_path: impl AsRef<Path>,
        guest_path: &str,
    ) -> Result<(), SandboxError> {
        self.fs_op(
            "fs copy_from_host",
            self.sdk().fs().copy_from_host(host_path, guest_path),
        )
        .await
    }

    /// Copy a guest file to the host.
    pub async fn fs_copy_to_host(
        &self,
        guest_path: &str,
        host_path: impl AsRef<Path>,
    ) -> Result<(), SandboxError> {
        self.fs_op(
            "fs copy_to_host",
            self.sdk().fs().copy_to_host(guest_path, host_path),
        )
        .await
    }
}
