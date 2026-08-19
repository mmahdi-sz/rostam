//! RAII temporary working directory management.
//!
//! Provides `TempDirGuard` to ensure unique temporary staging folders
//! are cleaned up on error, cancellation, or job finish.

use std::path::{Path, PathBuf};

/// RAII guard that creates a unique working directory on initialization
/// and guarantees recursive deletion upon drop (or explicit `.cleanup()`).
#[derive(Debug)]
pub struct TempDirGuard {
    path: PathBuf,
    persist: bool,
}

impl TempDirGuard {
    /// Creates a unique temporary directory inside `std::env::temp_dir()` prefixed with `prefix`.
    pub fn create(prefix: &str, trace_id: u64) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("{prefix}_{trace_id}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self {
            path,
            persist: false,
        })
    }

    /// Wraps an existing path in an RAII deletion guard.
    pub fn from_path(path: PathBuf) -> Self {
        Self {
            path,
            persist: false,
        }
    }

    /// Alias for `from_path`.
    pub fn new(path: PathBuf) -> Self {
        Self::from_path(path)
    }

    /// Returns a reference to the temporary directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Disarms the guard so the directory is preserved on drop (useful for debugging/testing).
    pub fn persist(&mut self) {
        self.persist = true;
    }

    /// Explicitly removes the directory immediately.
    pub fn cleanup(&mut self) -> std::io::Result<()> {
        if !self.persist && self.path.exists() {
            std::fs::remove_dir_all(&self.path)?;
        }
        Ok(())
    }
}

impl AsRef<Path> for TempDirGuard {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if !self.persist && self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temp_dir_guard_lifecycle() {
        let path;
        {
            let guard = TempDirGuard::create("test_temp_guard", 999999).expect("create temp dir");
            path = guard.path().to_path_buf();
            assert!(path.exists());
            assert!(path.is_dir());
        }
        assert!(!path.exists());
    }

    #[test]
    fn test_temp_dir_guard_persist() {
        let path;
        {
            let mut guard =
                TempDirGuard::create("test_temp_guard_persist", 888888).expect("create temp dir");
            path = guard.path().to_path_buf();
            assert!(path.exists());
            guard.persist();
        }
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path);
    }
}
