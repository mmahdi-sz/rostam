/// Bot ranks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Dalavar,   // Dalavar — Free
    Sepahbod,  // Sepahbod
    Esfandyar, // Esfandyar
    Sohrab,    // Sohrab
    Rostam,    // Rostam
}

impl Rank {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "dalavar" => Some(Self::Dalavar),
            "sepahbod" => Some(Self::Sepahbod),
            "esfandyar" => Some(Self::Esfandyar),
            "sohrab" => Some(Self::Sohrab),
            "rostam" => Some(Self::Rostam),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dalavar => "dalavar",
            Self::Sepahbod => "sepahbod",
            Self::Esfandyar => "esfandyar",
            Self::Sohrab => "sohrab",
            Self::Rostam => "rostam",
        }
    }

    /// Rank weight for upgrade/downgrade calculation (used by gift code & referral).
    pub fn weight(&self) -> i64 {
        match self {
            Self::Dalavar => 0,
            Self::Sepahbod => 3,
            Self::Esfandyar => 5,
            Self::Sohrab => 5,
            Self::Rostam => 10,
        }
    }

    /// Minimum rank required for a specific video resolution.
    pub fn min_for_quality(height: u32) -> Self {
        if height <= 500 {
            Self::Dalavar
        } else if height <= 1150 {
            Self::Sepahbod
        } else {
            Self::Esfandyar
        }
    }

    /// Daily denoise quota (seconds).
    pub fn denoise_daily_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 30 * 60,
            Self::Sohrab => 2 * 3600,
            Self::Rostam => 10 * 3600,
        }
    }

    /// Weekly denoise quota (seconds).
    pub fn denoise_weekly_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 2 * 3600,
            Self::Sohrab => 10 * 3600,
            Self::Rostam => 99 * 3600,
        }
    }

    /// Next rank with higher denoise quota.
    pub fn denoise_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(Self::Sohrab),
            Self::Sohrab => Some(Self::Rostam),
            Self::Rostam => None,
        }
    }

    pub fn display_name(&self) -> String {
        let key = match self {
            Self::Dalavar => "rank.dalavar",
            Self::Sepahbod => "rank.sepahbod",
            Self::Esfandyar => "rank.esfandyar",
            Self::Sohrab => "rank.sohrab",
            Self::Rostam => "rank.rostam",
        };
        crate::i18n::t(key)
    }

    /// Max YouTube quality limit (`None` = unlimited).
    pub fn max_yt_quality(&self) -> Option<u32> {
        match self {
            Self::Dalavar => Some(500),
            Self::Sepahbod => Some(1150),
            Self::Esfandyar | Self::Rostam => None,
            // AI-only plan — inherits YouTube limit from Dalavar
            Self::Sohrab => Self::Dalavar.max_yt_quality(),
        }
    }

    /// Daily traffic quota (bytes).
    pub fn daily_traffic_bytes(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod => 5 * 1024 * 1024 * 1024,
            Self::Esfandyar | Self::Rostam => 40 * 1024 * 1024 * 1024,
            // AI-only plan — inherits traffic limit from Dalavar
            Self::Sohrab => Self::Dalavar.daily_traffic_bytes(),
        }
    }

    /// Monthly traffic quota (bytes).
    pub fn monthly_traffic_bytes(&self) -> u64 {
        match self {
            Self::Dalavar => 15 * 1024 * 1024 * 1024,
            Self::Sepahbod => 60 * 1024 * 1024 * 1024,
            Self::Esfandyar | Self::Rostam => 400 * 1024 * 1024 * 1024,
            // AI-only plan — inherits traffic limit from Dalavar
            Self::Sohrab => Self::Dalavar.monthly_traffic_bytes(),
        }
    }

    /// Next rank with higher daily traffic.
    pub fn traffic_daily_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Sohrab => Some(Self::Esfandyar),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// Next rank with higher monthly traffic.
    pub fn traffic_monthly_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sohrab => Some(Self::Sepahbod),
            Self::Sepahbod => Some(Self::Esfandyar),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// YouTube playlist item limit (`None` = unlimited).
    pub fn playlist_limit(&self) -> Option<u32> {
        match self {
            Self::Dalavar | Self::Sohrab => Some(0), // Disallowed
            Self::Sepahbod => Some(10),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// Spotify/SoundCloud album/playlist limit (`Some(0)` = disallowed, `None` = unlimited).
    pub fn music_set_limit(&self) -> Option<u32> {
        match self {
            Self::Dalavar | Self::Sohrab => Some(0), // Disallowed
            Self::Sepahbod => Some(20),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// Archive 7z for music sets — Sepahbod gets individual tracks only.
    pub fn can_music_set_archive(&self) -> bool {
        matches!(self, Self::Esfandyar | Self::Rostam)
    }

    /// Separate subtitle file download.
    pub fn can_subtitle_file(&self) -> bool {
        matches!(self, Self::Sepahbod | Self::Esfandyar | Self::Rostam)
    }

    /// Mux subtitle into video.
    pub fn can_subtitle_mux(&self) -> bool {
        matches!(self, Self::Sepahbod | Self::Esfandyar | Self::Rostam)
    }

    /// Hardcode subtitle into video.
    #[allow(dead_code)]
    pub fn can_subtitle_hardcode(&self) -> bool {
        matches!(self, Self::Esfandyar | Self::Rostam)
    }

    /// Weekly image upscale quota (image count) by scale factor.
    pub fn upscale_weekly_quota(&self, scale: u32) -> u32 {
        match scale {
            2 => match self {
                Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 5,
                Self::Sohrab => 50,
                Self::Rostam => 500,
            },
            3 => match self {
                Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 3,
                Self::Sohrab => 30,
                Self::Rostam => 300,
            },
            // x4 and any other factor
            _ => match self {
                Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 2,
                Self::Sohrab => 20,
                Self::Rostam => 200,
            },
        }
    }

    /// Next rank with higher upscale quota.
    pub fn upscale_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(Self::Sohrab),
            Self::Sohrab => Some(Self::Rostam),
            Self::Rostam => None,
        }
    }

    /// Weekly background removal quota by rank.
    pub fn nobg_weekly_quota(&self) -> u32 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 3,
            Self::Sohrab => 30,
            Self::Rostam => 150,
        }
    }

    /// Next rank with higher background removal quota.
    pub fn nobg_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(Self::Sohrab),
            Self::Sohrab => Some(Self::Rostam),
            Self::Rostam => None,
        }
    }

    /// Weekly image colorization (DeOldify) quota.
    pub fn deoldify_weekly_quota(&self) -> u32 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 3,
            Self::Sohrab => 15,
            Self::Rostam => 100,
        }
    }

    /// Weekly text-to-speech (Moss TTS) quota in seconds.
    pub fn tts_weekly_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 30 * 60,
            Self::Sohrab => 100 * 60,
            Self::Rostam => 600 * 60,
        }
    }

    /// Denoise enabled by default for STT.
    pub fn stt_denoise_default(&self) -> bool {
        matches!(self, Self::Sohrab | Self::Rostam)
    }

    /// Whether user is allowed to enable STT denoise.
    pub fn can_stt_denoise(&self) -> bool {
        matches!(self, Self::Sohrab | Self::Rostam)
    }

    /// Daily audio separation quota (seconds).
    pub fn separation_daily_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 6 * 60,
            Self::Sohrab => 20 * 60,
            Self::Rostam => 200 * 60,
        }
    }

    /// Weekly audio separation quota (seconds).
    pub fn separation_weekly_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 20 * 60,
            Self::Sohrab => 90 * 60,
            Self::Rostam => 999 * 60,
        }
    }

    /// Next rank with higher audio separation quota.
    pub fn separation_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(Self::Sohrab),
            Self::Sohrab => Some(Self::Rostam),
            Self::Rostam => None,
        }
    }

    /// Accurate STT model (Large) allowed for Sohrab and Rostam.
    pub fn can_stt_accurate(&self) -> bool {
        matches!(self, Self::Sohrab | Self::Rostam)
    }

    /// Daily fast transcription quota (seconds) — `None` means disallowed.
    pub fn stt_fast_daily_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(30 * 60),
            Self::Sohrab => Some(3 * 3600),
            Self::Rostam => Some(30 * 3600),
        }
    }

    /// Weekly fast transcription quota (seconds).
    pub fn stt_fast_weekly_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(2 * 3600),
            Self::Sohrab => Some(15 * 3600),
            Self::Rostam => Some(150 * 3600),
        }
    }

    /// Daily accurate transcription quota (seconds) — `None` means disallowed.
    pub fn stt_accurate_daily_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => None,
            Self::Sohrab => Some(1 * 3600),
            Self::Rostam => Some(10 * 3600),
        }
    }

    /// Weekly accurate transcription quota (seconds).
    pub fn stt_accurate_weekly_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => None,
            Self::Sohrab => Some(5 * 3600),
            Self::Rostam => Some(50 * 3600),
        }
    }

    /// Daily compression CPU-time quota (seconds).
    pub fn compress_cpu_daily_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sohrab => 10 * 60,
            Self::Sepahbod => 30 * 60,
            Self::Esfandyar | Self::Rostam => 200 * 60,
        }
    }

    /// Monthly compression CPU-time quota (seconds).
    pub fn compress_cpu_monthly_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sohrab => 100 * 60,
            Self::Sepahbod => 400 * 60,
            Self::Esfandyar | Self::Rostam => 3000 * 60,
        }
    }

    /// Next rank with higher compression quota.
    #[allow(dead_code)]
    pub fn compress_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sohrab => Some(Self::Sepahbod),
            Self::Sepahbod => Some(Self::Esfandyar),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// Daily package conversion count (0 = feature blocked for this rank).
    pub fn pkgconvert_daily_count(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sohrab => 0,
            Self::Sepahbod => 5,
            Self::Esfandyar | Self::Rostam => 20,
        }
    }

    /// Next rank with higher package conversion quota.
    pub fn pkgconvert_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sohrab => Some(Self::Sepahbod),
            Self::Sepahbod => Some(Self::Esfandyar),
            Self::Esfandyar | Self::Rostam => None,
        }
    }
}

/// Integer division rounded up. Shared between gift code and referral modules for weighted remaining day calculation.
pub fn ceil_div(a: i64, b: i64) -> i64 {
    if b <= 0 {
        return 0;
    }
    (a + b - 1) / b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playlist_limit() {
        assert_eq!(Rank::Dalavar.playlist_limit(), Some(0));
        assert_eq!(Rank::Sohrab.playlist_limit(), Some(0));
        assert_eq!(Rank::Sepahbod.playlist_limit(), Some(10));
        assert_eq!(Rank::Esfandyar.playlist_limit(), None);
        assert_eq!(Rank::Rostam.playlist_limit(), None);
    }

    #[test]
    fn test_music_set_limit() {
        assert_eq!(Rank::Dalavar.music_set_limit(), Some(0));
        assert_eq!(Rank::Sohrab.music_set_limit(), Some(0));
        assert_eq!(Rank::Sepahbod.music_set_limit(), Some(20));
        assert_eq!(Rank::Esfandyar.music_set_limit(), None);
        assert_eq!(Rank::Rostam.music_set_limit(), None);
        assert!(!Rank::Sepahbod.can_music_set_archive());
        assert!(Rank::Esfandyar.can_music_set_archive());
    }

    #[test]
    fn test_rank_from_str_and_as_str() {
        assert_eq!(Rank::from_str("dalavar"), Some(Rank::Dalavar));
        assert_eq!(Rank::from_str("rostam"), Some(Rank::Rostam));
        assert_eq!(Rank::from_str("invalid"), None);
        assert_eq!(Rank::Rostam.as_str(), "rostam");
    }

    #[test]
    fn test_rank_weights() {
        assert!(Rank::Rostam.weight() > Rank::Esfandyar.weight());
        assert!(Rank::Esfandyar.weight() > Rank::Dalavar.weight());
    }

    #[test]
    fn test_pkgconvert_limits() {
        assert_eq!(Rank::Dalavar.pkgconvert_daily_count(), 0);
        assert_eq!(Rank::Sohrab.pkgconvert_daily_count(), 0);
        assert_eq!(Rank::Sepahbod.pkgconvert_daily_count(), 5);
        assert_eq!(Rank::Esfandyar.pkgconvert_daily_count(), 20);
        assert_eq!(Rank::Rostam.pkgconvert_daily_count(), 20);

        assert_eq!(Rank::Dalavar.pkgconvert_next_rank(), Some(Rank::Sepahbod));
        assert_eq!(Rank::Sepahbod.pkgconvert_next_rank(), Some(Rank::Esfandyar));
        assert_eq!(Rank::Esfandyar.pkgconvert_next_rank(), None);
    }

    #[test]
    fn test_min_for_quality() {
        assert_eq!(Rank::min_for_quality(480), Rank::Dalavar);
        assert_eq!(Rank::min_for_quality(720), Rank::Sepahbod);
        assert_eq!(Rank::min_for_quality(1080), Rank::Sepahbod);
        assert_eq!(Rank::min_for_quality(1440), Rank::Esfandyar);
        assert_eq!(Rank::min_for_quality(2160), Rank::Esfandyar);
    }

    #[test]
    fn test_ceil_div() {
        assert_eq!(ceil_div(10, 3), 4);
        assert_eq!(ceil_div(9, 3), 3);
        assert_eq!(ceil_div(0, 5), 0);
        assert_eq!(ceil_div(5, 0), 0);
    }
}
