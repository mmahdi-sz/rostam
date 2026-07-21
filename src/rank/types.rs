/// مقام‌های بات
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Dalavar,    // دلاور — رایگان
    Sepahbod,   // سپهبد
    Esfandyar,  // اسفندیار
    Sohrab,     // سهراب
    Rostam,     // رستم
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

    /// وزن رتبه برای مقایسه‌ی ارتقا/تنزل (کد هدیه و زیرمجموعه‌گیری از این استفاده می‌کنند).
    pub fn weight(&self) -> i64 {
        match self {
            Self::Dalavar => 0,
            Self::Sepahbod => 3,
            Self::Esfandyar => 5,
            Self::Sohrab => 5,
            Self::Rostam => 10,
        }
    }

    /// حداقل رتبه لازم برای دانلود یه کیفیت خاص
    pub fn min_for_quality(height: u32) -> Self {
        if height <= 500 {
            Self::Dalavar
        } else if height <= 1150 {
            Self::Sepahbod
        } else {
            Self::Esfandyar
        }
    }

    /// سقف حذف نویز روزانه (ثانیه)
    pub fn denoise_daily_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 30 * 60,
            Self::Sohrab => 2 * 3600,
            Self::Rostam => 10 * 3600,
        }
    }

    /// سقف حذف نویز هفتگی (ثانیه)
    pub fn denoise_weekly_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 2 * 3600,
            Self::Sohrab => 10 * 3600,
            Self::Rostam => 99 * 3600,
        }
    }

    /// رتبه بعدی که سقف denoise بیشتری داره
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

    /// محدودیت کیفیت YouTube (None = بدون محدودیت)
    pub fn max_yt_quality(&self) -> Option<u32> {
        match self {
            Self::Dalavar => Some(500),
            Self::Sepahbod => Some(1150),
            Self::Esfandyar | Self::Rostam => None,
            // AI-only plan — برای یوتیوب از دلاور ارث‌بری می‌کنه
            Self::Sohrab => Self::Dalavar.max_yt_quality(),
        }
    }

    /// ترافیک روزانه (bytes)
    pub fn daily_traffic_bytes(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod => 5 * 1024 * 1024 * 1024,
            Self::Esfandyar | Self::Rostam => 40 * 1024 * 1024 * 1024,
            // AI-only plan — برای ترافیک از دلاور ارث‌بری می‌کنه
            Self::Sohrab => Self::Dalavar.daily_traffic_bytes(),
        }
    }

    /// ترافیک ماهانه (bytes)
    pub fn monthly_traffic_bytes(&self) -> u64 {
        match self {
            Self::Dalavar => 15 * 1024 * 1024 * 1024,
            Self::Sepahbod => 60 * 1024 * 1024 * 1024,
            Self::Esfandyar | Self::Rostam => 400 * 1024 * 1024 * 1024,
            // AI-only plan — برای ترافیک از دلاور ارث‌بری می‌کنه
            Self::Sohrab => Self::Dalavar.monthly_traffic_bytes(),
        }
    }

    /// رتبه‌ی بعدی با ترافیک روزانه‌ی بیشتر (۵گ → ۴۰گ در اسفندیار)
    pub fn traffic_daily_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Sohrab => Some(Self::Esfandyar),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// رتبه‌ی بعدی با ترافیک ماهانه‌ی بیشتر
    pub fn traffic_monthly_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sohrab => Some(Self::Sepahbod),
            Self::Sepahbod => Some(Self::Esfandyar),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// پلی‌لیست YouTube (None = نامحدود)
    pub fn playlist_limit(&self) -> Option<u32> {
        match self {
            Self::Dalavar | Self::Sohrab => Some(0), // ممنوع
            Self::Sepahbod => Some(10),
            Self::Esfandyar | Self::Rostam => None,
        }
    }

    /// زیرنویس فایل جداگانه
    pub fn can_subtitle_file(&self) -> bool {
        matches!(self, Self::Sepahbod | Self::Esfandyar | Self::Rostam)
    }

    /// چسباندن زیرنویس به ویدیو
    pub fn can_subtitle_mux(&self) -> bool {
        matches!(self, Self::Sepahbod | Self::Esfandyar | Self::Rostam)
    }

    /// حک زیرنویس روی ویدیو (hardcode)
    #[allow(dead_code)]
    pub fn can_subtitle_hardcode(&self) -> bool {
        matches!(self, Self::Esfandyar | Self::Rostam)
    }

    /// سقف افزایش کیفیت تصویر در هفته (تعداد عکس) بر اساس scale factor.
    /// رستم = ۱۰ برابر سهراب. هرچی scale بزرگ‌تر، سقف کمتر (پردازش سنگین‌تر).
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
            // x4 و هر چیز دیگه
            _ => match self {
                Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 2,
                Self::Sohrab => 20,
                Self::Rostam => 200,
            },
        }
    }

    /// رتبه‌ی بعدی با سقف upscale بیشتر
    pub fn upscale_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(Self::Sohrab),
            Self::Sohrab => Some(Self::Rostam),
            Self::Rostam => None,
        }
    }

    /// اعتبار چت هوش مصنوعی در ماه — تومار (None = ممنوع)
    pub fn ai_chat_monthly_toomar(&self) -> Option<u32> {
        match self {
            Self::Esfandyar => Some(100),
            Self::Sohrab => Some(350),
            Self::Rostam => Some(1000),
            _ => None,
        }
    }

    /// denoise قبل از رونویسی — سهراب و رستم دیفالت فعال
    pub fn stt_denoise_default(&self) -> bool {
        matches!(self, Self::Sohrab | Self::Rostam)
    }

    /// آیا کاربر مجاز به فعال کردن denoise در STT هست؟
    pub fn can_stt_denoise(&self) -> bool {
        matches!(self, Self::Sohrab | Self::Rostam)
    }

    /// سقف جداسازی موزیک روزانه (ثانیه)
    pub fn separation_daily_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 6 * 60,
            Self::Sohrab => 20 * 60,
            Self::Rostam => 200 * 60,
        }
    }

    /// سقف جداسازی موزیک هفتگی (ثانیه)
    pub fn separation_weekly_secs(&self) -> u64 {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => 20 * 60,
            Self::Sohrab => 90 * 60,
            Self::Rostam => 999 * 60,
        }
    }

    /// رتبه بعدی که سقف جداسازی بیشتری داره
    pub fn separation_next_rank(&self) -> Option<Self> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(Self::Sohrab),
            Self::Sohrab => Some(Self::Rostam),
            Self::Rostam => None,
        }
    }

    /// مدل دقیق (Large) فقط سهراب و رستم
    pub fn can_stt_accurate(&self) -> bool {
        matches!(self, Self::Sohrab | Self::Rostam)
    }

    /// سقف رونویسی سریع روزانه (ثانیه) — None یعنی ممنوع
    pub fn stt_fast_daily_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(30 * 60),
            Self::Sohrab => Some(3 * 3600),
            Self::Rostam => Some(30 * 3600),
        }
    }

    /// سقف رونویسی سریع هفتگی (ثانیه)
    pub fn stt_fast_weekly_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => Some(2 * 3600),
            Self::Sohrab => Some(15 * 3600),
            Self::Rostam => Some(150 * 3600),
        }
    }

    /// سقف رونویسی دقیق روزانه (ثانیه) — None یعنی ممنوع
    pub fn stt_accurate_daily_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => None,
            Self::Sohrab => Some(1 * 3600),
            Self::Rostam => Some(10 * 3600),
        }
    }

    /// سقف رونویسی دقیق هفتگی (ثانیه)
    pub fn stt_accurate_weekly_secs(&self) -> Option<u64> {
        match self {
            Self::Dalavar | Self::Sepahbod | Self::Esfandyar => None,
            Self::Sohrab => Some(5 * 3600),
            Self::Rostam => Some(50 * 3600),
        }
    }
}

/// تقسیم صحیح گرد به بالا — مشترک بین کد هدیه و زیرمجموعه‌گیری برای تبدیل وزنی روزهای باقیمانده.
pub fn ceil_div(a: i64, b: i64) -> i64 {
    if b <= 0 { return 0; }
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
}
