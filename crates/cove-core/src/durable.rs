//! Cove Format (COVE) v1.0 — Durable replace publisher (Spec §74).
//!
//! The durable-replace protocol is the recommended publication strategy for
//! mutable filesystems. A writer:
//!
//! 1. Writes the new file under a sibling temporary path
//!    (`<final>.cove.tmp.<unique>`).
//! 2. Calls `fsync` on the temporary file.
//! 3. Renames atomically over the final path.
//! 4. Calls `fsync` on the parent directory so the rename is durable.
//!
//! The COVE specification permits readers to assume that any file at the final
//! path is structurally complete; partial writes are confined to the `.tmp.`
//! suffix. This module provides a portable helper that implements the
//! protocol on top of `std::fs`.
//!
//! On platforms whose filesystems do not support directory fsync (Windows
//! among others) the directory fsync step is skipped silently — the rename
//! itself is still atomic per platform semantics, so readers cannot observe
//! a partial file.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::CoveError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

trait DurableReplaceBackend {
    fn write_new_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), CoveError>;
    fn sync_file(&mut self, path: &Path) -> Result<(), CoveError>;
    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), CoveError>;
    fn remove_file(&mut self, path: &Path) -> Result<(), CoveError>;
    fn sync_parent_dir_best_effort(&mut self, final_path: &Path);
}

struct StdDurableReplaceBackend;

impl DurableReplaceBackend for StdDurableReplaceBackend {
    fn write_new_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), CoveError> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(bytes)?;
        Ok(())
    }

    fn sync_file(&mut self, path: &Path) -> Result<(), CoveError> {
        File::open(path)?.sync_all()?;
        Ok(())
    }

    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), CoveError> {
        fs::rename(from, to)?;
        Ok(())
    }

    fn remove_file(&mut self, path: &Path) -> Result<(), CoveError> {
        fs::remove_file(path)?;
        Ok(())
    }

    fn sync_parent_dir_best_effort(&mut self, final_path: &Path) {
        if let Some(parent) = final_path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
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
        backend.sync_parent_dir_best_effort(final_path);
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

        fn sync_parent_dir_best_effort(&mut self, final_path: &Path) {
            self.inner.sync_parent_dir_best_effort(final_path);
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
    fn parent_sync_open_failures_are_ignored() {
        let path = Path::new("/definitely-missing-parent-for-cove-tests/output.cove");
        let mut backend = StdDurableReplaceBackend;
        backend.sync_parent_dir_best_effort(path);
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
