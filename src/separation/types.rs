pub enum SeparationMode {
    Quality,
    Fast,
}

impl SeparationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Quality => "quality",
            Self::Fast => "fast",
        }
    }
}

pub struct SeparationResult {
    pub vocals_wav: Vec<u8>,
    pub instrumental_wav: Vec<u8>,
    pub duration_seconds: f64,
}
