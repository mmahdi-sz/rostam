//! ترجمه‌ی زیرنویس SRT با مدل محلی NLLB از طریق CTranslate2 (کریت `ct2rs`).
//!
//! مدل به فرمت CTranslate2 در `files/models/nllb/` است و باید کنارش
//! `tokenizer.json` داشته باشد (tokenizer داخل خود ct2rs بارگذاری می‌شود).
//! مدل ~۶۲۲MB است، پس یک بار لود و با `OnceLock` کش می‌شود.

use std::path::Path;
use std::sync::OnceLock;

use ct2rs::{Config, Translator};

/// نوع مترجم با tokenizer پیش‌فرض ct2rs (که `tokenizer.json` را می‌خواند).
type NllbTranslator = Translator<ct2rs::tokenizers::auto::Tokenizer>;

/// مسیر پوشه‌ی مدل NLLB (نسبت به دایرکتوری کاری ربات، مثل بقیه‌ی مدل‌ها).
const MODEL_DIR: &str = "files/models/nllb";

/// مترجم کش‌شده — لود مدل گران است، پس فقط بار اول ساخته می‌شود.
/// `Translator` داخل ct2rs از نظر thread امن است (`Send + Sync`).
static TRANSLATOR: OnceLock<Result<NllbTranslator, String>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtItem {
    pub index: usize,
    pub timestamp: String,
    pub text: String,
}

pub fn parse_srt(content: &str) -> Vec<SrtItem> {
    let mut items = Vec::new();
    let normalized = content.replace("\r\n", "\n");
    let blocks = normalized.split("\n\n");

    for block in blocks {
        let lines: Vec<&str> = block.lines().collect();
        if lines.len() >= 3
            && let Ok(index) = lines[0].trim().parse::<usize>()
        {
            let timestamp = lines[1].trim().to_string();
            let text = lines[2..].join("\n").trim().to_string();
            items.push(SrtItem {
                index,
                timestamp,
                text,
            });
        }
    }

    items
}

pub fn format_srt(items: &[SrtItem]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!(
            "{}\n{}\n{}\n\n",
            item.index, item.timestamp, item.text
        ));
    }
    out
}

/// نگاشت کد زبانِ زیرنویس (مثل `fa`) به کد NLLB (مثل `pes_Arab`).
pub fn map_language_code(tgt: &str) -> Option<&'static str> {
    match tgt {
        "fa" => Some("pes_Arab"),
        "en" => Some("eng_Latn"),
        "it" => Some("ita_Latn"),
        "fr" => Some("fra_Latn"),
        "de" => Some("deu_Latn"),
        "ru" => Some("rus_Cyrl"),
        "es" => Some("spa_Latn"),
        "ar" => Some("arb_Arab"),
        "hi" => Some("hin_Deva"),
        "tr" => Some("tur_Latn"),
        _ => None,
    }
}

/// مترجم کش‌شده را برمی‌گرداند (بار اول مدل را لود می‌کند).
fn translator() -> Result<&'static NllbTranslator, String> {
    TRANSLATOR
        .get_or_init(|| {
            let dir = Path::new(MODEL_DIR);
            if !dir.join("model.bin").exists() {
                return Err(format!(
                    "NLLB model not found at {MODEL_DIR}/model.bin (deploy copies files/models/)"
                ));
            }
            Translator::new(dir, &Config::default())
                .map_err(|e| format!("Failed to load NLLB translator from {MODEL_DIR}: {e}"))
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// دسته‌ای از متن‌ها را به `target_lang` (کد NLLB مثل `pes_Arab`) ترجمه می‌کند.
/// بلوکینگ است (CPU-bound)، پس باید داخل `spawn_blocking` صدا زده شود.
fn translate_batch_blocking(texts: &[String], target_lang: &str) -> Result<Vec<String>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let t = translator()?;

    // زیرنویس‌های چندخطی: خط‌ها را با فاصله یکی می‌کنیم (مثل نسخه‌ی پایتونی).
    let sources: Vec<String> = texts
        .iter()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();

    // NLLB زبان مقصد را به‌صورت target-prefix می‌گیرد.
    let prefixes: Vec<Vec<String>> = sources
        .iter()
        .map(|_| vec![target_lang.to_string()])
        .collect();

    let results = t
        .translate_batch_with_target_prefix(&sources, &prefixes, &Default::default(), None)
        .map_err(|e| format!("translate_batch failed: {e}"))?;

    Ok(results
        .into_iter()
        .map(|(text, _score)| clean_special_tokens(&text))
        .collect())
}

/// توکن‌های ویژه‌ی NLLB را که گاهی در خروجی decode باقی می‌مانند حذف می‌کند
/// (مثل `<unk>`, `</s>`, تگ زبان `pes_Arab`). tokenizer داخلی ct2rs در این نسخه
/// special tokenها را skip نمی‌کند، پس دستی تمیز می‌کنیم.
fn clean_special_tokens(text: &str) -> String {
    let mut out = text.to_string();
    for tok in ["<unk>", "<s>", "</s>", "<pad>", "<mask>"] {
        out = out.replace(tok, "");
    }
    // تگ‌های زبان NLLB به شکل xxx_Yyyy هستند (مثل pes_Arab, eng_Latn).
    out = out
        .split_whitespace()
        .filter(|w| !is_nllb_lang_tag(w))
        .collect::<Vec<_>>()
        .join(" ");
    out.trim().to_string()
}

/// آیا کلمه یک تگ زبان NLLB است (سه حرف کوچک + `_` + چهار حرف با حرف اول بزرگ).
fn is_nllb_lang_tag(w: &str) -> bool {
    let bytes = w.as_bytes();
    bytes.len() == 8
        && bytes[3] == b'_'
        && bytes[..3].iter().all(|c| c.is_ascii_lowercase())
        && bytes[4].is_ascii_uppercase()
        && bytes[5..].iter().all(|c| c.is_ascii_lowercase())
}

/// یک فایل SRT انگلیسی را می‌خواند، متن‌ها را به `target_lang` (کد NLLB) ترجمه
/// می‌کند، و با حفظ تایم‌کدها در `output_path` می‌نویسد.
///
/// `threads` نگه داشته شده برای سازگاری امضا؛ ct2rs تعداد thread را از Config
/// می‌گیرد (فعلاً پیش‌فرض). کار CPU-bound داخل `spawn_blocking` اجرا می‌شود.
pub async fn translate_srt<F>(
    input_path: &Path,
    output_path: &Path,
    target_lang: &str,
    _threads: usize,
    mut progress_cb: F,
) -> Result<(), String>
where
    F: FnMut(usize, usize, u64, u64),
{
    let content = tokio::fs::read_to_string(input_path)
        .await
        .map_err(|e| format!("Failed to read srt input file: {e}"))?;

    let mut items = parse_srt(&content);
    if items.is_empty() {
        tokio::fs::write(output_path, "")
            .await
            .map_err(|e| format!("Failed to write empty srt output: {e}"))?;
        return Ok(());
    }

    let total_items = items.len();
    let target_lang = target_lang.to_string();
    let start_time = std::time::Instant::now();

    progress_cb(0, total_items, 0, 0);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(usize, usize, u64, u64)>(16);

    let handle = tokio::task::spawn_blocking(move || -> Result<Vec<SrtItem>, String> {
        let batch_size = 32;
        let mut processed = 0;
        for chunk in items.chunks_mut(batch_size) {
            let texts: Vec<String> = chunk.iter().map(|item| item.text.clone()).collect();
            let translated = translate_batch_blocking(&texts, &target_lang)?;
            for (item, t) in chunk.iter_mut().zip(translated) {
                item.text = t;
            }
            processed += chunk.len();
            let elapsed = start_time.elapsed().as_secs();
            let eta = if processed > 0 && processed < total_items {
                let per_item = elapsed as f64 / processed as f64;
                (per_item * (total_items - processed) as f64) as u64
            } else {
                0
            };
            let _ = tx.blocking_send((processed, total_items, elapsed, eta));
        }
        Ok(items)
    });

    while let Some((done, total, elapsed, eta)) = rx.recv().await {
        progress_cb(done, total, elapsed, eta);
    }

    let translated_items = handle
        .await
        .map_err(|e| format!("translate task panicked: {e}"))??;

    let out_content = format_srt(&translated_items);
    tokio::fs::write(output_path, out_content)
        .await
        .map_err(|e| format!("Failed to write srt output file: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_srt_parse_and_format() {
        let input = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n\n2\n00:00:04,500 --> 00:00:07,000\nSecond subtitle line\n\n";
        let parsed = parse_srt(input);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].index, 1);
        assert_eq!(parsed[0].timestamp, "00:00:01,000 --> 00:00:04,000");
        assert_eq!(parsed[0].text, "Hello world");
        assert_eq!(parsed[1].index, 2);
        assert_eq!(parsed[1].timestamp, "00:00:04,500 --> 00:00:07,000");
        assert_eq!(parsed[1].text, "Second subtitle line");

        let formatted = format_srt(&parsed);
        assert_eq!(formatted, input);
    }

    #[test]
    fn test_language_code_mapping() {
        assert_eq!(map_language_code("fa"), Some("pes_Arab"));
        assert_eq!(map_language_code("en"), Some("eng_Latn"));
        assert_eq!(map_language_code("es"), Some("spa_Latn"));
        assert_eq!(map_language_code("ar"), Some("arb_Arab"));
        assert_eq!(map_language_code("hi"), Some("hin_Deva"));
        assert_eq!(map_language_code("tr"), Some("tur_Latn"));
        assert_eq!(map_language_code("unknown"), None);
    }

    #[test]
    fn test_clean_special_tokens() {
        assert_eq!(
            clean_special_tokens("این یک آزمایش است.<unk>"),
            "این یک آزمایش است."
        );
        assert_eq!(clean_special_tokens("pes_Arab سلام دنیا"), "سلام دنیا");
        assert_eq!(clean_special_tokens("متن</s> بعدی"), "متن بعدی");
        assert_eq!(
            clean_special_tokens("متن عادی بدون توکن"),
            "متن عادی بدون توکن"
        );
        // کلمه‌ی معمولی نباید اشتباهاً تگ زبان تشخیص داده شود.
        assert_eq!(clean_special_tokens("testing"), "testing");
    }

    /// تست واقعیِ ترجمه با مدل NLLB. نیاز به files/models/nllb روی دیسک دارد و
    /// کند است، پس ignored — با `cargo test real_translation -- --ignored` اجرا شود.
    #[tokio::test]
    #[ignore]
    async fn test_real_translation_en_to_fa() {
        let dir = std::env::temp_dir().join(format!("nllb_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.srt");
        let output = dir.join("out.srt");
        std::fs::write(
            &input,
            "1\n00:00:01,000 --> 00:00:03,000\nHello, how are you?\n\n2\n00:00:04,000 --> 00:00:06,000\nThis is a test.\n\n",
        )
        .unwrap();

        translate_srt(&input, &output, "pes_Arab", 4, |_, _, _, _| {})
            .await
            .expect("translation failed");

        let result = std::fs::read_to_string(&output).unwrap();
        let items = parse_srt(&result);
        assert_eq!(items.len(), 2, "should keep both entries");
        assert_eq!(
            items[0].timestamp, "00:00:01,000 --> 00:00:03,000",
            "timing preserved"
        );
        // خروجی باید فارسی (غیر لاتین) و بدون توکن ویژه باشد و با انگلیسی فرق کند.
        assert!(
            items[0]
                .text
                .chars()
                .any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)),
            "expected Persian text, got: {}",
            items[0].text
        );
        assert!(
            !items[0].text.contains("<unk>") && !items[0].text.contains("</s>"),
            "special tokens must be stripped: {}",
            items[0].text
        );
        assert_ne!(items[0].text, "Hello, how are you?", "must be translated");
        // خروجی باید فارسی (غیر لاتین) باشد و با انگلیسی ورودی فرق کند.
        assert!(
            items[0]
                .text
                .chars()
                .any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)),
            "expected Persian text, got: {}",
            items[0].text
        );
        assert_ne!(items[0].text, "Hello, how are you?", "must be translated");

        std::fs::remove_dir_all(&dir).ok();
    }
}
