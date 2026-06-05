#[derive(Debug, Clone)]
pub struct SttConfig {
    pub lang: SttLang,
    pub model_size: SttModelSize,
    pub denoise: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttLang {
    Fa,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttModelSize {
    Large,
    Small,
}

impl SttConfig {
    pub fn model_path(&self, base: &str) -> String {
        let name = match (self.lang, self.model_size) {
            (SttLang::Fa, SttModelSize::Large) => "vosk-model-fa-0.42",
            (SttLang::Fa, SttModelSize::Small) => "vosk-model-small-fa-0.42",
            (SttLang::En, SttModelSize::Large) => "vosk-model-en-us-0.22-lgraph",
            (SttLang::En, SttModelSize::Small) => "vosk-model-small-en-us-0.15",
        };
        format!("{}/{}", base, name)
    }

    pub fn label_key(&self) -> &'static str {
        match (self.lang, self.model_size) {
            (SttLang::Fa, SttModelSize::Large) => "stt.language.fa_big",
            (SttLang::Fa, SttModelSize::Small) => "stt.language.fa_small",
            (SttLang::En, SttModelSize::Large) => "stt.language.en_big",
            (SttLang::En, SttModelSize::Small) => "stt.language.en_small",
        }
    }

    pub fn lang_label_fa(&self) -> &'static str {
        match self.lang {
            SttLang::Fa => "فارسی",
            SttLang::En => "انگلیسی",
        }
    }

    pub fn model_label_fa(&self) -> &'static str {
        match self.model_size {
            SttModelSize::Large => "دقیق",
            SttModelSize::Small => "سریع",
        }
    }
}
