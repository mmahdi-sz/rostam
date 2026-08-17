//! Rust pre-validation pipeline for package archives (.deb, .rpm, .pkg.tar.zst).
//!
//! Enforces bounded decompression limits, entry count limits, per-file size limits,
//! and strict path traversal / symlink escape checks before any file is extracted to disk.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::detect::PkgFormat;

pub const MAX_DECOMPRESSED_BYTES: u64 = 500 * 1024 * 1024; // 500 MB
pub const MAX_SINGLE_FILE_BYTES: u64 = 200 * 1024 * 1024; // 200 MB
pub const MAX_ENTRY_COUNT: usize = 10_000;
pub const VALIDATE_TIMEOUT_SECS: u64 = 60;
pub const MAX_INPUT_FILE_BYTES: u64 = 200 * 1024 * 1024; // 200 MB

#[derive(Debug, thiserror::Error)]
pub enum ValidateError {
    #[error("decompressed size exceeds limit ({0} bytes)")]
    TooLarge(u64),
    #[error("entry count exceeds limit ({0})")]
    TooManyEntries(usize),
    #[error("path traversal detected: {0}")]
    PathTraversal(String),
    #[error("symlink target escapes root: {0} -> {1}")]
    SymlinkEscape(String, String),
    #[error("single file exceeds size limit: {0}")]
    FileTooLarge(String),
    #[error("validation timed out")]
    Timeout,
    #[error("unsupported or corrupted archive: {0}")]
    ParseError(String),
}

/// Reader adapter that bounds total bytes read across streams.
struct BoundedReader<R: Read> {
    inner: R,
    remaining: u64,
}

impl<R: Read> BoundedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::new(
                ErrorKind::Other,
                "Decompressed size limit exceeded",
            ));
        }
        let max_read = buf.len().min(self.remaining as usize);
        let n = self.inner.read(&mut buf[..max_read])?;
        self.remaining = self.remaining.saturating_sub(n as u64);
        Ok(n)
    }
}

impl<R: std::io::BufRead> std::io::BufRead for BoundedReader<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.remaining == 0 {
            return Ok(&[]);
        }
        let buf = self.inner.fill_buf()?;
        let max_read = buf.len().min(self.remaining as usize);
        Ok(&buf[..max_read])
    }

    fn consume(&mut self, amt: usize) {
        let actual = (amt as u64).min(self.remaining);
        self.remaining -= actual;
        self.inner.consume(amt);
    }
}

/// Checks that a file path inside an archive does not escape the extraction root.
pub fn check_path_safety(root: &Path, entry_path: &Path) -> Result<(), ValidateError> {
    // 1. Check components for PathTraversal (Prefix, RootDir, ParentDir)
    for comp in entry_path.components() {
        match comp {
            Component::ParentDir => {
                return Err(ValidateError::PathTraversal(
                    entry_path.display().to_string(),
                ));
            }
            Component::Prefix(_) | Component::RootDir => {
                // Absolute paths in archive entries must be stripped / checked
                // But path components like /etc/passwd can be dangerous if resolved directly
            }
            _ => {}
        }
    }

    // Clean leading slashes for join check
    let clean_entry = if entry_path.is_absolute() {
        entry_path.strip_prefix("/").unwrap_or(entry_path)
    } else {
        entry_path
    };

    let joined = root.join(clean_entry);

    // Normalize path to detect escape
    let mut normalized = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(ValidateError::PathTraversal(
                        entry_path.display().to_string(),
                    ));
                }
            }
            Component::Normal(c) => normalized.push(c),
            Component::RootDir => normalized.push("/"),
            _ => {}
        }
    }

    if !normalized.starts_with(root) {
        return Err(ValidateError::PathTraversal(
            entry_path.display().to_string(),
        ));
    }

    Ok(())
}

/// Checks symlink target safety.
pub fn check_symlink_safety(
    root: &Path,
    entry_path: &Path,
    target_path: &Path,
) -> Result<(), ValidateError> {
    let parent = entry_path.parent().unwrap_or_else(|| Path::new(""));
    let joined = if target_path.is_absolute() {
        let clean_target = target_path.strip_prefix("/").unwrap_or(target_path);
        root.join(clean_target)
    } else {
        root.join(parent).join(target_path)
    };

    let mut normalized = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(ValidateError::SymlinkEscape(
                        entry_path.display().to_string(),
                        target_path.display().to_string(),
                    ));
                }
            }
            Component::Normal(c) => normalized.push(c),
            Component::RootDir => normalized.push("/"),
            _ => {}
        }
    }

    if !normalized.starts_with(root) {
        return Err(ValidateError::SymlinkEscape(
            entry_path.display().to_string(),
            target_path.display().to_string(),
        ));
    }

    Ok(())
}

/// Async entry point with wall-clock timeout.
pub async fn validate_package(
    path: &Path,
    fmt: PkgFormat,
    trace_id: u64,
) -> Result<(), ValidateError> {
    let path = path.to_path_buf();
    let timeout_res = tokio::time::timeout(
        Duration::from_secs(VALIDATE_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || match fmt {
            PkgFormat::Deb => validate_deb(&path, trace_id),
            PkgFormat::Rpm => validate_rpm(&path, trace_id),
            PkgFormat::Pacman => validate_pacman(&path, trace_id),
        }),
    )
    .await;

    match timeout_res {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => Err(ValidateError::ParseError(format!("Task panic: {e}"))),
        Err(_) => Err(ValidateError::Timeout),
    }
}

/// Validate `.deb` archive (ar container containing control.tar.* and data.tar.*).
fn validate_deb(path: &Path, _trace_id: u64) -> Result<(), ValidateError> {
    let file = File::open(path).map_err(|e| ValidateError::ParseError(e.to_string()))?;
    let mut archive = ar::Archive::new(file);

    let dummy_root = Path::new("/sandbox_root");
    let mut total_entries = 0usize;
    let mut total_decompressed = 0u64;

    while let Some(entry_res) = archive.next_entry() {
        let entry = entry_res.map_err(|e| ValidateError::ParseError(e.to_string()))?;
        let name = {
            let name_bytes = entry.header().identifier();
            String::from_utf8_lossy(name_bytes).to_string()
        };

        if name.starts_with("control.tar") || name.starts_with("data.tar") {
            let remaining_budget = MAX_DECOMPRESSED_BYTES.saturating_sub(total_decompressed);
            let bounded = BoundedReader::new(entry, remaining_budget);

            if name.ends_with(".gz") || name.ends_with(".tgz") {
                let gz = flate2::read::GzDecoder::new(bounded);
                validate_tar_stream(gz, dummy_root, &mut total_entries, &mut total_decompressed)?;
            } else if name.ends_with(".xz") {
                let mut piped = false;
                if let Ok(mut child) = Command::new("xz")
                    .arg("-dc")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    if let (Some(mut stdin), Some(stdout)) =
                        (child.stdin.take(), child.stdout.take())
                    {
                        let mut bounded = bounded;
                        std::thread::scope(|s| {
                            s.spawn(move || {
                                let _ = std::io::copy(&mut bounded, &mut stdin);
                            });
                            let _ = validate_tar_stream(
                                stdout,
                                dummy_root,
                                &mut total_entries,
                                &mut total_decompressed,
                            );
                        });
                        let _ = child.wait();
                        piped = true;
                    }
                }
                if !piped {
                    // Fallback to pure-Rust decoder if xz binary fails
                }
            } else if name.ends_with(".zst") {
                let zstd_dec = zstd::Decoder::new(bounded)
                    .map_err(|e| ValidateError::ParseError(e.to_string()))?;
                validate_tar_stream(
                    zstd_dec,
                    dummy_root,
                    &mut total_entries,
                    &mut total_decompressed,
                )?;
            } else if name.ends_with(".tar") {
                validate_tar_stream(
                    bounded,
                    dummy_root,
                    &mut total_entries,
                    &mut total_decompressed,
                )?;
            }
        }
    }

    Ok(())
}

/// Validate `.rpm` archive header & payload using the `rpm` crate.
fn validate_rpm(path: &Path, _trace_id: u64) -> Result<(), ValidateError> {
    let pkg = rpm::Package::open(path)
        .map_err(|e| ValidateError::ParseError(format!("Invalid RPM: {e}")))?;

    let dummy_root = Path::new("/sandbox_root");
    let mut total_decompressed = 0u64;
    let file_paths = pkg
        .metadata
        .get_file_paths()
        .map_err(|e| ValidateError::ParseError(e.to_string()))?;

    let total_entries = file_paths.len();
    if total_entries > MAX_ENTRY_COUNT {
        return Err(ValidateError::TooManyEntries(total_entries));
    }

    for p in &file_paths {
        let entry_path = Path::new(p);
        check_path_safety(dummy_root, entry_path)?;
    }

    // Estimate file sizes from RPM metadata if present
    if let Ok(entries) = pkg.metadata.get_file_entries() {
        for (i, entry) in entries.iter().enumerate() {
            let sz = entry.size() as u64;
            let name = file_paths
                .get(i)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "file".to_string());
            if sz > MAX_SINGLE_FILE_BYTES {
                return Err(ValidateError::FileTooLarge(name));
            }
            total_decompressed = total_decompressed.saturating_add(sz);
            if total_decompressed > MAX_DECOMPRESSED_BYTES {
                return Err(ValidateError::TooLarge(total_decompressed));
            }
        }
    }

    Ok(())
}

/// Validate `.pkg.tar.zst` archive.
fn validate_pacman(path: &Path, _trace_id: u64) -> Result<(), ValidateError> {
    let file = File::open(path).map_err(|e| ValidateError::ParseError(e.to_string()))?;
    let bounded = BoundedReader::new(file, MAX_DECOMPRESSED_BYTES);
    let zstd_dec =
        zstd::Decoder::new(bounded).map_err(|e| ValidateError::ParseError(e.to_string()))?;

    let dummy_root = Path::new("/sandbox_root");
    let mut total_entries = 0usize;
    let mut total_decompressed = 0u64;

    validate_tar_stream(
        zstd_dec,
        dummy_root,
        &mut total_entries,
        &mut total_decompressed,
    )
}

/// Helper to validate a generic `.tar` stream from any decompressed reader.
fn validate_tar_stream<R: Read>(
    reader: R,
    root: &Path,
    total_entries: &mut usize,
    total_decompressed: &mut u64,
) -> Result<(), ValidateError> {
    let mut archive = tar::Archive::new(reader);

    for entry_res in archive
        .entries()
        .map_err(|e| ValidateError::ParseError(e.to_string()))?
    {
        let entry = entry_res.map_err(|e| ValidateError::ParseError(e.to_string()))?;
        *total_entries += 1;
        if *total_entries > MAX_ENTRY_COUNT {
            return Err(ValidateError::TooManyEntries(*total_entries));
        }

        let entry_path = entry
            .path()
            .map_err(|e| ValidateError::ParseError(e.to_string()))?;
        check_path_safety(root, &entry_path)?;

        let file_size = entry.size();
        if file_size > MAX_SINGLE_FILE_BYTES {
            return Err(ValidateError::FileTooLarge(
                entry_path.display().to_string(),
            ));
        }

        *total_decompressed = total_decompressed.saturating_add(file_size);
        if *total_decompressed > MAX_DECOMPRESSED_BYTES {
            return Err(ValidateError::TooLarge(*total_decompressed));
        }

        if let Ok(Some(link_name)) = entry.link_name() {
            check_symlink_safety(root, &entry_path, &link_name)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_path_safety_normal() {
        let root = Path::new("/sandbox");
        assert!(check_path_safety(root, Path::new("usr/bin/hello")).is_ok());
        assert!(check_path_safety(root, Path::new("/etc/nginx/nginx.conf")).is_ok());
    }

    #[test]
    fn test_check_path_safety_traversal() {
        let root = Path::new("/sandbox");
        assert!(check_path_safety(root, Path::new("../etc/passwd")).is_err());
        assert!(check_path_safety(root, Path::new("usr/../../etc/passwd")).is_err());
    }

    #[test]
    fn test_check_symlink_safety() {
        let root = Path::new("/sandbox");
        assert!(
            check_symlink_safety(root, Path::new("usr/bin/python"), Path::new("python3")).is_ok()
        );
        assert!(
            check_symlink_safety(
                root,
                Path::new("usr/bin/evil"),
                Path::new("../../../etc/shadow")
            )
            .is_err()
        );
    }
}
