//! Package format detection based on filename extension and magic bytes.

use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkgFormat {
    Deb,
    Rpm,
    Pacman, // .pkg.tar.zst
}

impl PkgFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Pacman => "pacman",
        }
    }

    pub fn display_ext(&self) -> &'static str {
        match self {
            Self::Deb => ".deb",
            Self::Rpm => ".rpm",
            Self::Pacman => ".pkg.tar.zst",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "deb" => Some(Self::Deb),
            "rpm" => Some(Self::Rpm),
            "pacman" | "pkg.tar.zst" => Some(Self::Pacman),
            _ => None,
        }
    }
}

/// Detect format from filename extension first, then verify against magic bytes.
/// Returns `None` if extension is unsupported or magic bytes mismatch.
pub fn detect_pkg_format(path: &Path, filename: &str) -> Option<PkgFormat> {
    let fn_lower = filename.to_lowercase();
    let ext_fmt = if fn_lower.ends_with(".deb") {
        Some(PkgFormat::Deb)
    } else if fn_lower.ends_with(".rpm") {
        Some(PkgFormat::Rpm)
    } else if fn_lower.ends_with(".pkg.tar.zst") || fn_lower.ends_with(".tar.zst") {
        Some(PkgFormat::Pacman)
    } else {
        None
    }?;

    let magic_fmt = detect_by_magic(path)?;

    if ext_fmt == magic_fmt {
        Some(ext_fmt)
    } else {
        None
    }
}

/// Magic byte inspection of the first 8 bytes of the file.
pub fn detect_by_magic(path: &Path) -> Option<PkgFormat> {
    let mut file = File::open(path).ok()?;
    let mut buf = [0u8; 8];
    let n = file.read(&mut buf).ok()?;
    if n < 4 {
        return None;
    }

    // .deb: ar(1) archive magic "!" "<" "a" "r" "c" "h" ">" "\n"
    if n >= 8 && &buf[..8] == b"!<arch>\n" {
        return Some(PkgFormat::Deb);
    }

    // .rpm: RPM lead magic 0xED 0xAB 0xEE 0xDB
    if buf[..4] == [0xED, 0xAB, 0xEE, 0xDB] {
        return Some(PkgFormat::Rpm);
    }

    // .pkg.tar.zst: zstd frame magic 0x28 0xB5 0x2F 0xFD (little-endian: 0xFD2FB528)
    if buf[..4] == [0x28, 0xB5, 0x2F, 0xFD] {
        return Some(PkgFormat::Pacman);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkg_format_str() {
        assert_eq!(PkgFormat::Deb.as_str(), "deb");
        assert_eq!(PkgFormat::Rpm.as_str(), "rpm");
        assert_eq!(PkgFormat::Pacman.as_str(), "pacman");
        assert_eq!(PkgFormat::from_str("deb"), Some(PkgFormat::Deb));
    }
}
