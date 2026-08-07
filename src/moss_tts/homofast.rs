use std::collections::{HashMap, HashSet};

/// Candidate pronunciation for a homograph, containing context words that trigger this reading.
#[derive(Debug, Clone)]
pub struct HomographCandidate {
    pub pronunciation: &'static str,
    pub context_words: &'static [&'static str],
}

/// HomoFast Persian Homograph Disambiguator
pub struct HomoFastResolver {
    homographs: HashMap<&'static str, Vec<HomographCandidate>>,
    stopwords: HashSet<&'static str>,
}

impl Default for HomoFastResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl HomoFastResolver {
    pub fn new() -> Self {
        let mut homographs = HashMap::new();

        // Top Persian Homographs from HomoFast / HomoRich
        homographs.insert(
            "شیر",
            vec![
                HomographCandidate {
                    pronunciation: "شیر", // milk / lion (shir)
                    context_words: &[
                        "جنگل",
                        "حیوان",
                        "درنده",
                        "سلطان",
                        "پاکت",
                        "خوردن",
                        "نوشیدن",
                        "سفید",
                        "گاوداری",
                        "لبنیات",
                        "مادر",
                        "پستان",
                    ],
                },
                HomographCandidate {
                    pronunciation: "شِیر", // tap / valve (sher)
                    context_words: &[
                        "آب",
                        "بستن",
                        "باز",
                        "لوله",
                        "حمام",
                        "آشپزخانه",
                        "چکه",
                        "فلکه",
                        "توالت",
                    ],
                },
            ],
        );

        homographs.insert(
            "بار",
            vec![
                HomographCandidate {
                    pronunciation: "بار", // load / cargo / time (bār)
                    context_words: &[
                        "کامیون",
                        "حمل",
                        "سنگین",
                        "دفعه",
                        "نوبت",
                        "چندین",
                        "یک",
                        "اول",
                        "دوم",
                        "سفر",
                    ],
                },
                HomographCandidate {
                    pronunciation: "بَر", // fruit / ON (bar)
                    context_words: &["درخت", "میوه", "نتیجه", "ثمر", "روی", "دوش", "تن"],
                },
            ],
        );

        homographs.insert(
            "سر",
            vec![
                HomographCandidate {
                    pronunciation: "سَر", // head / chief (sar)
                    context_words: &[
                        "صورت", "بدن", "مو", "کلاه", "درد", "گوش", "چشم", "رئیس", "بزرگ",
                    ],
                },
                HomographCandidate {
                    pronunciation: "سِر", // secret / numb (ser)
                    context_words: &["مخفی", "راز", "پنهان", "بی‌حس", "دارو", "بی‌حسی", "دندانپزشک"],
                },
            ],
        );

        homographs.insert(
            "گل",
            vec![
                HomographCandidate {
                    pronunciation: "گُل", // flower (gol)
                    context_words: &[
                        "گیاه",
                        "رز",
                        "سرخ",
                        "باغ",
                        "بوستان",
                        "گلدان",
                        "زیبا",
                        "عطر",
                        "بو",
                    ],
                },
                HomographCandidate {
                    pronunciation: "گِل", // mud (gel)
                    context_words: &["خاک", "آب", "شلی", "باران", "کوچه‌", "زمین", "ساختن", "آجر"],
                },
            ],
        );

        homographs.insert(
            "ماه",
            vec![HomographCandidate {
                pronunciation: "ماه", // moon / month (māh)
                context_words: &[
                    "آسمان",
                    "شب",
                    "سیاره",
                    "سال",
                    "روز",
                    "فروردین",
                    "اردیبهشت",
                    "خرداد",
                    "تیر",
                    "مرداد",
                    "شهریور",
                    "مهر",
                    "آبان",
                    "آذر",
                    "دی",
                    "بهمن",
                    "اسفند",
                    "شمسی",
                    "قمری",
                ],
            }],
        );

        let stopwords = HashSet::from([
            "از", "به", "در", "با", "که", "این", "آن", "و", "یا", "برای", "را", "است", "بود", "شد",
            "کرد", "بر", "تا", "نیز", "هم", "چون", "بی",
        ]);

        Self {
            homographs,
            stopwords,
        }
    }

    /// Preprocesses and resolves homograph pronunciations in a Persian sentence.
    pub fn disambiguate(&self, text: &str) -> String {
        let raw_tokens: Vec<&str> = text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| c.is_ascii_punctuation() || "،؛؟!«»()".contains(c)))
            .filter(|w| !w.is_empty())
            .collect();

        let context_tokens: Vec<&str> = raw_tokens
            .iter()
            .cloned()
            .filter(|w| !self.stopwords.contains(w))
            .collect();

        let words: Vec<&str> = text.split_whitespace().collect();
        let mut result = Vec::with_capacity(words.len());

        for word in words {
            let clean_word =
                word.trim_matches(|c: char| c.is_ascii_punctuation() || "،؛؟!«»()".contains(c));
            if let Some(candidates) = self.homographs.get(clean_word) {
                let mut best_candidate: Option<&str> = None;
                let mut max_score = 0.0f32;

                for candidate in candidates {
                    let mut hits = 0;
                    for ctx_word in candidate.context_words {
                        if context_tokens.contains(ctx_word) {
                            hits += 1;
                        }
                    }

                    if !candidate.context_words.is_empty() {
                        let score = (hits as f32) / (candidate.context_words.len() as f32);
                        if score > max_score {
                            max_score = score;
                            best_candidate = Some(candidate.pronunciation);
                        }
                    }
                }

                if let Some(resolved) = best_candidate {
                    if max_score >= 0.05 {
                        result.push(word.replace(clean_word, resolved));
                        continue;
                    }
                }
            }
            result.push(word.to_string());
        }

        result.join(" ")
    }
}
