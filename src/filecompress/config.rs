#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressFmt {
    Zip,
    SevenZ,
    Rar,
    Zstd,
}

impl CompressFmt {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "zip" => Some(Self::Zip),
            "7z" => Some(Self::SevenZ),
            "rar" => Some(Self::Rar),
            "zstd" => Some(Self::Zstd),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZ => "7z",
            Self::Rar => "rar",
            Self::Zstd => "zstd",
        }
    }

    #[allow(dead_code)]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Zip => "ZIP",
            Self::SevenZ => "7Z",
            Self::Rar => "RAR",
            Self::Zstd => "ZSTD",
        }
    }

    #[allow(dead_code)]
    pub fn output_extension(&self) -> &'static str {
        match self {
            Self::Zip => ".zip",
            Self::SevenZ => ".7z",
            Self::Rar => ".rar",
            Self::Zstd => ".tar.zst",
        }
    }

    /// Max compression level per format. RAR up to 5, zstd up to 19 (without `--ultra`).
    pub fn max_level(&self) -> u8 {
        match self {
            Self::Rar => 5,
            Self::Zstd => 19,
            _ => 9,
        }
    }

    /// zstd does not support encryption.
    pub fn supports_password(&self) -> bool {
        !matches!(self, Self::Zstd)
    }

    /// tar.zst is always a single stream, so solid mode is not selectable.
    pub fn supports_solid(&self) -> bool {
        matches!(self, Self::SevenZ | Self::Rar)
    }

    /// zstd has no `-v` equivalent; splitting supported only on 7z/zip/rar.
    pub fn supports_split(&self) -> bool {
        !matches!(self, Self::Zstd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressAlgo {
    Lzma2,
    Ppmd,
    Bzip2,
}

impl CompressAlgo {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "lzma2" => Some(Self::Lzma2),
            "ppmd" => Some(Self::Ppmd),
            "bzip2" => Some(Self::Bzip2),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lzma2 => "lzma2",
            Self::Ppmd => "ppmd",
            Self::Bzip2 => "bzip2",
        }
    }

    #[allow(dead_code)]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Lzma2 => "LZMA2",
            Self::Ppmd => "PPMd",
            Self::Bzip2 => "BZip2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressConfig {
    pub fmt: CompressFmt,
    pub algo: CompressAlgo,
    pub level: u8,
    pub password: Option<String>,
    pub split_mb: Option<u32>,
    pub obfuscate: bool,
    pub solid: bool,
}

impl Default for CompressConfig {
    fn default() -> Self {
        Self {
            fmt: CompressFmt::SevenZ,
            algo: CompressAlgo::Lzma2,
            level: 5,
            password: None,
            split_mb: None,
            obfuscate: false,
            solid: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_fmt_str() {
        assert_eq!(CompressFmt::from_str("zip"), Some(CompressFmt::Zip));
        assert_eq!(CompressFmt::from_str("7z"), Some(CompressFmt::SevenZ));
        assert_eq!(CompressFmt::from_str("rar"), Some(CompressFmt::Rar));
        assert_eq!(CompressFmt::from_str("zstd"), Some(CompressFmt::Zstd));
        assert_eq!(CompressFmt::from_str("invalid"), None);
        assert_eq!(CompressFmt::SevenZ.as_str(), "7z");
    }

    #[test]
    fn test_fmt_capabilities() {
        // zstd: no password, no solid option, no split — max level 19.
        assert!(!CompressFmt::Zstd.supports_password());
        assert!(!CompressFmt::Zstd.supports_solid());
        assert!(!CompressFmt::Zstd.supports_split());
        assert_eq!(CompressFmt::Zstd.max_level(), 19);
        assert_eq!(CompressFmt::Rar.max_level(), 5);
        assert_eq!(CompressFmt::SevenZ.max_level(), 9);
        assert!(CompressFmt::SevenZ.supports_password());
        assert!(CompressFmt::Zip.supports_split());
        assert!(!CompressFmt::Zip.supports_solid());
    }

    #[test]
    fn test_compress_algo_str() {
        assert_eq!(CompressAlgo::from_str("lzma2"), Some(CompressAlgo::Lzma2));
        assert_eq!(CompressAlgo::from_str("ppmd"), Some(CompressAlgo::Ppmd));
        assert_eq!(CompressAlgo::from_str("bzip2"), Some(CompressAlgo::Bzip2));
        assert_eq!(CompressAlgo::Lzma2.display_name(), "LZMA2");
    }
}
