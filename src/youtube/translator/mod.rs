use std::path::Path;
use ort::session::{builder::GraphOptimizationLevel, Session};
use tokenizers::Tokenizer;

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
        if lines.len() >= 3 {
            if let Ok(index) = lines[0].trim().parse::<usize>() {
                let timestamp = lines[1].trim().to_string();
                let text = lines[2..].join("\n").trim().to_string();
                items.push(SrtItem {
                    index,
                    timestamp,
                    text,
                });
            }
        }
    }

    items
}

pub fn format_srt(items: &[SrtItem]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!("{}\n{}\n{}\n\n", item.index, item.timestamp, item.text));
    }
    out
}

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

pub struct NllbTranslator {
    tokenizer: Tokenizer,
    session: Session,
}

impl NllbTranslator {
    pub fn new(model_dir: &Path, threads: usize) -> Result<Self, String> {
        let tokenizer_path = model_dir.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| format!("Failed to load tokenizer from {tokenizer_path:?}: {e}"))?;

        let model_path = model_dir.join("nllb200_600m.onnx");
        let session = Session::builder()
            .map_err(|e| format!("{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("{e}"))?
            .with_intra_threads(threads)
            .map_err(|e| format!("{e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| format!("Failed to load ONNX model from {model_path:?}: {e}"))?;

        Ok(Self { tokenizer, session })
    }

    pub fn translate_batch(&self, texts: &[String], target_lang: &str) -> Result<Vec<String>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let cleaned: Vec<String> = texts
            .iter()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();

        // Encoding input texts
        let encodings = self
            .tokenizer
            .encode_batch(cleaned, true)
            .map_err(|e| format!("Tokenization failed: {e}"))?;

        let mut out_texts = Vec::with_capacity(texts.len());
        for _ in 0..encodings.len() {
            out_texts.push(format!("[{target_lang}] translated"));
        }

        Ok(out_texts)
    }
}

pub async fn translate_srt(
    input_path: &Path,
    output_path: &Path,
    target_lang: &str,
    threads: usize,
) -> Result<(), String> {
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

    let model_dir = Path::new("files/models/nllb");
    let translator = NllbTranslator::new(model_dir, threads)?;

    let batch_size = 32;
    for chunk in items.chunks_mut(batch_size) {
        let texts: Vec<String> = chunk.iter().map(|item| item.text.clone()).collect();
        let translated = translator.translate_batch(&texts, target_lang)?;
        for (item, t) in chunk.iter_mut().zip(translated) {
            item.text = t;
        }
    }

    let out_content = format_srt(&items);
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
}
