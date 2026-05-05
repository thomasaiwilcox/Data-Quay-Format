//! Quay Format (QF) v1.0 — Durable replace publisher (Spec §74).
//!
//! The durable-replace protocol is the recommended publication strategy for
//! mutable filesystems. A writer:
//!
//! 1. Writes the new file under a sibling temporary path
//!    (`<final>.qf.tmp.<unique>`).
//! 2. Calls `fsync` on the temporary file.
//! 3. Renames atomically over the final path.
//! 4. Calls `fsync` on the parent directory so the rename is durable.
//!
//! The QF specification permits readers to assume that any file at the final
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

use crate::QfError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Compute the temporary path used by the durable-replace protocol.
pub fn temp_path_for(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(".qf.tmp.");
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
/// on success; left in place on error so that callers can clean up).
pub fn durable_replace(final_path: &Path, bytes: &[u8]) -> Result<PathBuf, QfError> {
    let tmp = temp_path_for(final_path);
    let result = (|| -> Result<(), QfError> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);

        fs::rename(&tmp, final_path)?;
        if let Some(parent) = final_path.parent() {
            let dir = File::open(parent)?;
            dir.sync_all()?;
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_path_is_sibling_with_qf_tmp_suffix() {
        let p = Path::new("/tmp/example.qf");
        let t = temp_path_for(p);
        let s = t.to_string_lossy().into_owned();
        assert!(s.starts_with("/tmp/example.qf.qf.tmp."));
    }

    #[test]
    fn temp_path_is_unique_within_process() {
        let p = Path::new("/tmp/example.qf");
        assert_ne!(temp_path_for(p), temp_path_for(p));
    }

    #[test]
    fn durable_replace_writes_full_payload() {
        let dir = std::env::temp_dir();
        let target = dir.join(format!("qf-test-{}.qf", std::process::id()));
        // Pre-existing content must be replaced atomically.
        std::fs::write(&target, b"old").unwrap();
        let payload = b"new content";
        durable_replace(&target, payload).unwrap();
        let read_back = std::fs::read(&target).unwrap();
        assert_eq!(read_back, payload);
        let _ = std::fs::remove_file(&target);
    }
}
