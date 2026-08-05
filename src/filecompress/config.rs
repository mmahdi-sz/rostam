#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressFmt {
    Zip,
    SevenZ,
    Rar,
}

impl CompressFmt {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "zip" => Some(Self::Zip),
            "7z" => Some(Self::SevenZ),
            "rar" => Some(Self::Rar),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZ => "7z",
            Self::Rar => "rar",
        }
    }

    #[allow(dead_code)]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Zip => "ZIP",
            Self::SevenZ => "7Z",
            Self::Rar => "RAR",
        }
    }

    #[allow(dead_code)]
    pub fn output_extension(&self) -> &'static str {
        match self {
            Self::Zip => ".zip",
            Self::SevenZ => ".7z",
            Self::Rar => ".rar",
        }
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
        assert_eq!(CompressFmt::from_str("invalid"), None);
        assert_eq!(CompressFmt::SevenZ.as_str(), "7z");
    }

    #[test]
    fn test_compress_algo_str() {
        assert_eq!(CompressAlgo::from_str("lzma2"), Some(CompressAlgo::Lzma2));
        assert_eq!(CompressAlgo::from_str("ppmd"), Some(CompressAlgo::Ppmd));
        assert_eq!(CompressAlgo::from_str("bzip2"), Some(CompressAlgo::Bzip2));
        assert_eq!(CompressAlgo::Lzma2.display_name(), "LZMA2");
    }
}
