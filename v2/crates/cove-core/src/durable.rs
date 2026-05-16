//! Cove Format (COVE) v2.0 — Durable replace publisher (Spec §75).
//!
//! The durable-replace protocol is the recommended publication strategy for
//! mutable filesystems. A writer:
//!
//! 1. Writes the new file under a sibling temporary path
//!    (`<final>.cove.tmp.<unique>`).
//! 2. Calls `fsync` on the temporary file.
//! 3. Renames atomically over the final path.
//! 4. Calls `fsync` on the parent directory so the rename is durable. On
//!    Windows, the rename uses `MoveFileExW` with write-through semantics
//!    because Rust does not expose Unix-style parent-directory fsync there.
//!
//! The COVE specification permits readers to assume that any file at the final
//! path is structurally complete; partial writes are confined to the `.tmp.`
//! suffix. This module provides a portable helper that implements the
//! protocol on top of `std::fs`.
//!
//! Parent directory fsync, or the Windows write-through rename, is fallible and
//! is part of the success condition: callers must not treat the final path as
//! durably published unless this module returns `Ok`.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::CoveError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(not(windows))]
fn open_parent_dir_for_sync(path: &Path) -> Result<File, CoveError> {
    Ok(File::open(path)?)
}

#[cfg(windows)]
fn rename_durable(from: &Path, to: &Path) -> Result<(), CoveError> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x00000001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x00000008;

    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn rename_durable(from: &Path, to: &Path) -> Result<(), CoveError> {
    fs::rename(from, to)?;
    Ok(())
}

/// Compute the temporary path used by the durable-replace protocol.
pub fn temp_path_for(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(".cove.tmp.");
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    s.push(format!("{}.{}.{}", std::process::id(), nanos, counter));
    PathBuf::from(s)
}

/// Atomically publish `bytes` at `final_path` via the durable-replace
/// protocol.
///
/// Returns the [`PathBuf`] of the temporary file used (already renamed away
/// on success; cleaned up best-effort on error).
pub fn durable_replace(final_path: &Path, bytes: &[u8]) -> Result<PathBuf, CoveError> {
    let mut backend = StdDurableReplaceBackend;
    durable_replace_with_backend(&mut backend, final_path, bytes)
}

/// Atomically publish bytes produced by `write` at `final_path` via the
/// durable-replace protocol.
///
/// This avoids forcing callers to build the complete file in memory before
/// publication. The supplied writer callback runs against a newly created
/// sibling temporary file. The result is not considered durable until the file
/// and parent directory sync steps complete.
pub fn durable_replace_with_writer<F>(final_path: &Path, write: F) -> Result<PathBuf, CoveError>
where
    F: FnOnce(&mut File) -> Result<(), CoveError>,
{
    let tmp = temp_path_for(final_path);
    let result = (|| -> Result<(), CoveError> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        write(&mut file)?;
        file.sync_all()?;
        rename_durable(&tmp, final_path)?;
        let parent = final_path.parent().ok_or_else(|| {
            CoveError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "cannot fsync parent directory for {}: no parent path",
                    final_path.display()
                ),
            ))
        })?;
        StdDurableReplaceBackend::sync_parent_path(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    Ok(tmp)
}

trait DurableReplaceBackend {
    fn write_new_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), CoveError>;
    fn sync_file(&mut self, path: &Path) -> Result<(), CoveError>;
    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), CoveError>;
    fn remove_file(&mut self, path: &Path) -> Result<(), CoveError>;
    fn sync_parent_dir(&mut self, final_path: &Path) -> Result<(), CoveError>;
}

struct StdDurableReplaceBackend;

impl StdDurableReplaceBackend {
    #[cfg(windows)]
    fn sync_parent_path(_parent: &Path) -> Result<(), CoveError> {
        Ok(())
    }

    #[cfg(not(windows))]
    fn sync_parent_path(parent: &Path) -> Result<(), CoveError> {
        open_parent_dir_for_sync(parent)?.sync_all()?;
        Ok(())
    }
}

impl DurableReplaceBackend for StdDurableReplaceBackend {
    fn write_new_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), CoveError> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        Ok(())
    }

    fn sync_file(&mut self, path: &Path) -> Result<(), CoveError> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?
            .sync_all()?;
        Ok(())
    }

    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), CoveError> {
        rename_durable(from, to)
    }

    fn remove_file(&mut self, path: &Path) -> Result<(), CoveError> {
        fs::remove_file(path)?;
        Ok(())
    }

    fn sync_parent_dir(&mut self, final_path: &Path) -> Result<(), CoveError> {
        let parent = final_path.parent().ok_or_else(|| {
            CoveError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "cannot fsync parent directory for {}: no parent path",
                    final_path.display()
                ),
            ))
        })?;
        Self::sync_parent_path(parent)
    }
}

fn durable_replace_with_backend<B: DurableReplaceBackend>(
    backend: &mut B,
    final_path: &Path,
    bytes: &[u8],
) -> Result<PathBuf, CoveError> {
    let tmp = temp_path_for(final_path);
    let result = (|| -> Result<(), CoveError> {
        backend.write_new_file(&tmp, bytes)?;
        backend.sync_file(&tmp)?;
        backend.rename(&tmp, final_path)?;
        // INVARIANT: the replacement is not durable until the parent
        // directory entry is synced. A failure here is returned even though
        // the rename may already be visible.
        backend.sync_parent_dir(final_path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = backend.remove_file(&tmp);
    }
    result?;
    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FailStep {
        WriteNewFile,
        SyncFile,
        Rename,
        SyncParentDir,
        RemoveFile,
    }

    struct FailingBackend {
        inner: StdDurableReplaceBackend,
        fail_at: Option<FailStep>,
    }

    impl DurableReplaceBackend for FailingBackend {
        fn write_new_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), CoveError> {
            if self.fail_at == Some(FailStep::WriteNewFile) {
                return Err(std::io::Error::other("injected write failure").into());
            }
            self.inner.write_new_file(path, bytes)
        }

        fn sync_file(&mut self, path: &Path) -> Result<(), CoveError> {
            if self.fail_at == Some(FailStep::SyncFile) {
                return Err(std::io::Error::other("injected sync failure").into());
            }
            self.inner.sync_file(path)
        }

        fn rename(&mut self, from: &Path, to: &Path) -> Result<(), CoveError> {
            if self.fail_at == Some(FailStep::Rename) {
                return Err(std::io::Error::other("injected rename failure").into());
            }
            self.inner.rename(from, to)
        }

        fn remove_file(&mut self, path: &Path) -> Result<(), CoveError> {
            if self.fail_at == Some(FailStep::RemoveFile) {
                return Err(std::io::Error::other("injected remove failure").into());
            }
            self.inner.remove_file(path)
        }

        fn sync_parent_dir(&mut self, final_path: &Path) -> Result<(), CoveError> {
            if self.fail_at == Some(FailStep::SyncParentDir) {
                return Err(std::io::Error::other("injected parent sync failure").into());
            }
            self.inner.sync_parent_dir(final_path)
        }
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cove-durable-{name}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_siblings_for(target: &Path) -> Vec<PathBuf> {
        let parent = target.parent().unwrap();
        let prefix = format!(
            "{}.cove.tmp.",
            target.file_name().unwrap().to_string_lossy()
        );
        std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let file_name = path.file_name()?.to_string_lossy();
                file_name.starts_with(&prefix).then_some(path)
            })
            .collect()
    }

    #[test]
    fn temp_path_is_sibling_with_cove_tmp_suffix() {
        let p = Path::new("/tmp/example.cove");
        let t = temp_path_for(p);
        let s = t.to_string_lossy().into_owned();
        assert!(s.starts_with("/tmp/example.cove.cove.tmp."));
    }

    #[test]
    fn temp_path_is_unique_within_process() {
        let p = Path::new("/tmp/example.cove");
        assert_ne!(temp_path_for(p), temp_path_for(p));
    }

    #[test]
    fn durable_replace_writes_full_payload() {
        let dir = test_dir("writes-full-payload");
        let target = dir.join("example.cove");
        // Pre-existing content must be replaced atomically.
        std::fs::write(&target, b"old").unwrap();
        let payload = b"new content";
        durable_replace(&target, payload).unwrap();
        let read_back = std::fs::read(&target).unwrap();
        assert_eq!(read_back, payload);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn durable_replace_with_writer_streams_to_temp_file() {
        let dir = test_dir("writer");
        let target = dir.join("example.cove");
        let tmp = durable_replace_with_writer(&target, |file| {
            file.write_all(b"streamed ")?;
            file.write_all(b"bytes")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"streamed bytes");
        assert!(
            !tmp.exists(),
            "temporary path should have been renamed away on success"
        );
        assert!(
            temp_siblings_for(&target).is_empty(),
            "no durable temp siblings should remain"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn parent_sync_open_failures_are_reported() {
        let path = Path::new("/definitely-missing-parent-for-cove-tests/output.cove");
        let mut backend = StdDurableReplaceBackend;
        assert!(backend.sync_parent_dir(path).is_err());
    }

    #[test]
    fn durable_replace_preserves_old_file_when_temp_sync_fails() {
        let dir = test_dir("sync-fail");
        let target = dir.join("sync-fail.cove");
        std::fs::write(&target, b"old").unwrap();
        let mut backend = FailingBackend {
            inner: StdDurableReplaceBackend,
            fail_at: Some(FailStep::SyncFile),
        };

        assert!(durable_replace_with_backend(&mut backend, &target, b"new").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(temp_siblings_for(&target).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn durable_replace_preserves_old_file_when_rename_fails() {
        let dir = test_dir("rename-fail");
        let target = dir.join("rename-fail.cove");
        std::fs::write(&target, b"old").unwrap();
        let mut backend = FailingBackend {
            inner: StdDurableReplaceBackend,
            fail_at: Some(FailStep::Rename),
        };

        assert!(durable_replace_with_backend(&mut backend, &target, b"new").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(temp_siblings_for(&target).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn durable_replace_reports_parent_sync_failure() {
        let dir = test_dir("parent-sync-fail");
        let target = dir.join("parent-sync-fail.cove");
        std::fs::write(&target, b"old").unwrap();
        let mut backend = FailingBackend {
            inner: StdDurableReplaceBackend,
            fail_at: Some(FailStep::SyncParentDir),
        };

        assert!(durable_replace_with_backend(&mut backend, &target, b"new").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn durable_replace_cleans_temp_file_on_failure() {
        let dir = test_dir("write-fail");
        let target = dir.join("write-fail.cove");
        let mut backend = FailingBackend {
            inner: StdDurableReplaceBackend,
            fail_at: Some(FailStep::WriteNewFile),
        };

        assert!(durable_replace_with_backend(&mut backend, &target, b"new").is_err());
        assert!(temp_siblings_for(&target).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
