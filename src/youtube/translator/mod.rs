//! SRT subtitle translation using local NLLB model via CTranslate2 (`ct2rs`).

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ct2rs::{Config, Tokenizer, Translator};

/// Translator type using ct2rs default tokenizer.
type NllbTranslator = Translator<NllbTokenizer>;

struct NllbTokenizer(ct2rs::tokenizers::auto::Tokenizer);

impl Tokenizer for NllbTokenizer {
    fn encode(&self, input: &str) -> anyhow::Result<Vec<String>> {
        let (lang, text) = input
            .split_once(' ')
            .ok_or_else(|| anyhow::anyhow!("missing source language"))?;
        let mut tokens = self.0.encode(text)?;
        // The legacy NLLB tokenizer template ends with EOS and a language placeholder.
        anyhow::ensure!(
            tokens.last().is_some_and(|token| token == "<unk>"),
            "unexpected NLLB source template"
        );
        tokens.pop();
        tokens.push(lang.to_owned());
        Ok(tokens)
    }

    fn decode(&self, tokens: Vec<String>) -> anyhow::Result<String> {
        self.0.decode(tokens)
    }
}

/// Path to NLLB model directory.
const MODEL_DIR: &str = "files/models/nllb";

const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

struct NllbHolder {
    translator: Option<NllbTranslator>,
    last_used: Instant,
}

impl NllbHolder {
    fn new() -> Self {
        Self {
            translator: None,
            last_used: Instant::now(),
        }
    }

    fn get_or_load(&mut self) -> Result<&NllbTranslator, String> {
        if self.translator.is_none() {
            let dir = Path::new(MODEL_DIR);
            if !dir.join("model.bin").exists() {
                return Err(format!(
                    "NLLB model not found at {MODEL_DIR}/model.bin (deploy copies files/models/)"
                ));
            }
            eprintln!("[translator] Loading NLLB model (first use / after idle unload)");
            let tokenizer =
                ct2rs::tokenizers::auto::Tokenizer::new(dir).map_err(|e| e.to_string())?;
            let t = Translator::with_tokenizer(dir, NllbTokenizer(tokenizer), &Config::default())
                .map_err(|e| format!("Failed to load NLLB translator from {MODEL_DIR}: {e}"))?;
            self.translator = Some(t);
        }
        self.last_used = Instant::now();
        Ok(self.translator.as_ref().unwrap())
    }

    fn unload(&mut self) -> bool {
        if self.translator.is_some() {
            self.translator = None;
            eprintln!("[translator] NLLB model unloaded (idle timeout)");
            return true;
        }
        false
    }

    fn is_idle(&self) -> bool {
        self.translator.is_some() && self.last_used.elapsed() >= SESSION_IDLE_TIMEOUT
    }
}

static HOLDER: OnceLock<Arc<Mutex<NllbHolder>>> = OnceLock::new();

fn holder() -> &'static Arc<Mutex<NllbHolder>> {
    HOLDER.get_or_init(|| Arc::new(Mutex::new(NllbHolder::new())))
}

/// Periodically unloads NLLB translator if idle for >180s.
pub fn spawn_session_reaper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }

    let h = holder().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let unloaded = {
                let mut guard = h.lock().unwrap_or_else(|e| e.into_inner());
                if guard.is_idle() {
                    guard.unload()
                } else {
                    false
                }
            };
            if unloaded {
                crate::moebius::cpu::trim_memory();
            }
        }
    });
}

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

/// Maps ISO language code to NLLB target code (e.g., `fa` -> `pes_Arab`).
pub fn map_language_code(tgt: &str) -> Option<&'static str> {
    match tgt.to_ascii_lowercase().trim_end_matches("-orig") {
        "ace" => Some("ace_Arab"),
        "acm" => Some("acm_Arab"),
        "acq" => Some("acq_Arab"),
        "aeb" => Some("aeb_Arab"),
        "af" => Some("afr_Latn"),
        "ajp" => Some("ajp_Arab"),
        "ak" | "tw" => Some("twi_Latn"),
        "als" | "sq" => Some("als_Latn"),
        "am" => Some("amh_Ethi"),
        "apc" => Some("apc_Arab"),
        "ar" | "arb" => Some("arb_Arab"),
        "ars" => Some("ars_Arab"),
        "ary" => Some("ary_Arab"),
        "arz" => Some("arz_Arab"),
        "as" => Some("asm_Beng"),
        "ast" => Some("ast_Latn"),
        "awa" => Some("awa_Deva"),
        "ay" | "ayr" => Some("ayr_Latn"),
        "az" | "azj" => Some("azj_Latn"),
        "azb" => Some("azb_Arab"),
        "ba" => Some("bak_Cyrl"),
        "ban" => Some("ban_Latn"),
        "be" => Some("bel_Cyrl"),
        "bem" => Some("bem_Latn"),
        "bg" => Some("bul_Cyrl"),
        "bh" | "bho" => Some("bho_Deva"),
        "bjn" => Some("bjn_Arab"),
        "bm" => Some("bam_Latn"),
        "bn" => Some("ben_Beng"),
        "bo" => Some("bod_Tibt"),
        "bs" => Some("bos_Latn"),
        "bug" => Some("bug_Latn"),
        "ca" => Some("cat_Latn"),
        "ceb" => Some("ceb_Latn"),
        "cjk" => Some("cjk_Latn"),
        "ckb" => Some("ckb_Arab"),
        "crh" => Some("crh_Latn"),
        "cs" => Some("ces_Latn"),
        "cy" => Some("cym_Latn"),
        "da" => Some("dan_Latn"),
        "de" => Some("deu_Latn"),
        "dik" => Some("dik_Latn"),
        "dyu" => Some("dyu_Latn"),
        "dz" => Some("dzo_Tibt"),
        "ee" => Some("ewe_Latn"),
        "el" => Some("ell_Grek"),
        "en" => Some("eng_Latn"),
        "eo" => Some("epo_Latn"),
        "es" => Some("spa_Latn"),
        "et" => Some("est_Latn"),
        "eu" => Some("eus_Latn"),
        "fa" | "pes" => Some("pes_Arab"),
        "ff" | "fuv" => Some("fuv_Latn"),
        "fi" => Some("fin_Latn"),
        "fil" | "tl" => Some("tgl_Latn"),
        "fj" => Some("fij_Latn"),
        "fo" => Some("fao_Latn"),
        "fon" => Some("fon_Latn"),
        "fr" => Some("fra_Latn"),
        "fur" => Some("fur_Latn"),
        "ga" => Some("gle_Latn"),
        "gaz" | "om" => Some("gaz_Latn"),
        "gd" => Some("gla_Latn"),
        "gl" => Some("glg_Latn"),
        "gn" => Some("grn_Latn"),
        "gu" => Some("guj_Gujr"),
        "ha" => Some("hau_Latn"),
        "he" | "iw" => Some("heb_Hebr"),
        "hi" => Some("hin_Deva"),
        "hne" => Some("hne_Deva"),
        "hr" => Some("hrv_Latn"),
        "ht" => Some("hat_Latn"),
        "hu" => Some("hun_Latn"),
        "hy" => Some("hye_Armn"),
        "id" => Some("ind_Latn"),
        "ig" => Some("ibo_Latn"),
        "ilo" => Some("ilo_Latn"),
        "is" => Some("isl_Latn"),
        "it" => Some("ita_Latn"),
        "ja" => Some("jpn_Jpan"),
        "jv" => Some("jav_Latn"),
        "ka" => Some("kat_Geor"),
        "kab" => Some("kab_Latn"),
        "kac" => Some("kac_Latn"),
        "kam" => Some("kam_Latn"),
        "kbp" => Some("kbp_Latn"),
        "kea" => Some("kea_Latn"),
        "kg" => Some("kon_Latn"),
        "khk" | "mn" => Some("khk_Cyrl"),
        "ki" => Some("kik_Latn"),
        "kk" => Some("kaz_Cyrl"),
        "km" => Some("khm_Khmr"),
        "kmb" => Some("kmb_Latn"),
        "kmr" | "ku" => Some("kmr_Latn"),
        "kn" => Some("kan_Knda"),
        "knc" => Some("knc_Arab"),
        "ko" => Some("kor_Hang"),
        "ks" => Some("kas_Arab"),
        "ky" => Some("kir_Cyrl"),
        "lb" => Some("ltz_Latn"),
        "lg" => Some("lug_Latn"),
        "li" => Some("lim_Latn"),
        "lij" => Some("lij_Latn"),
        "lmo" => Some("lmo_Latn"),
        "ln" => Some("lin_Latn"),
        "lo" => Some("lao_Laoo"),
        "lt" => Some("lit_Latn"),
        "ltg" => Some("ltg_Latn"),
        "lua" => Some("lua_Latn"),
        "luo" => Some("luo_Latn"),
        "lus" => Some("lus_Latn"),
        "lv" | "lvs" => Some("lvs_Latn"),
        "mag" => Some("mag_Deva"),
        "mai" => Some("mai_Deva"),
        "mg" | "plt" => Some("plt_Latn"),
        "mi" => Some("mri_Latn"),
        "min" => Some("min_Latn"),
        "mk" => Some("mkd_Cyrl"),
        "ml" => Some("mal_Mlym"),
        "mni" => Some("mni_Beng"),
        "mos" => Some("mos_Latn"),
        "mr" => Some("mar_Deva"),
        "ms" | "zsm" => Some("zsm_Latn"),
        "mt" => Some("mlt_Latn"),
        "my" => Some("mya_Mymr"),
        "nb" | "no" => Some("nob_Latn"),
        "ne" | "npi" => Some("npi_Deva"),
        "nl" => Some("nld_Latn"),
        "nn" => Some("nno_Latn"),
        "nso" => Some("nso_Latn"),
        "nus" => Some("nus_Latn"),
        "ny" => Some("nya_Latn"),
        "oc" => Some("oci_Latn"),
        "or" | "ory" => Some("ory_Orya"),
        "pa" => Some("pan_Guru"),
        "pag" => Some("pag_Latn"),
        "pap" => Some("pap_Latn"),
        "pbt" | "ps" => Some("pbt_Arab"),
        "pl" => Some("pol_Latn"),
        "prs" => Some("prs_Arab"),
        "pt" | "pt-br" | "pt-pt" => Some("por_Latn"),
        "qu" | "quy" => Some("quy_Latn"),
        "rn" => Some("run_Latn"),
        "ro" => Some("ron_Latn"),
        "ru" => Some("rus_Cyrl"),
        "rw" => Some("kin_Latn"),
        "sa" => Some("san_Deva"),
        "sat" => Some("sat_Beng"),
        "sc" => Some("srd_Latn"),
        "scn" => Some("scn_Latn"),
        "sd" => Some("snd_Arab"),
        "sg" => Some("sag_Latn"),
        "shn" => Some("shn_Mymr"),
        "si" => Some("sin_Sinh"),
        "sk" => Some("slk_Latn"),
        "sl" => Some("slv_Latn"),
        "sm" => Some("smo_Latn"),
        "sn" => Some("sna_Latn"),
        "so" => Some("som_Latn"),
        "sr" => Some("srp_Cyrl"),
        "ss" => Some("ssw_Latn"),
        "st" => Some("sot_Latn"),
        "su" => Some("sun_Latn"),
        "sv" => Some("swe_Latn"),
        "sw" | "swh" => Some("swh_Latn"),
        "szl" => Some("szl_Latn"),
        "ta" => Some("tam_Taml"),
        "taq" => Some("taq_Latn"),
        "te" => Some("tel_Telu"),
        "tg" => Some("tgk_Cyrl"),
        "th" => Some("tha_Thai"),
        "ti" => Some("tir_Ethi"),
        "tk" => Some("tuk_Latn"),
        "tn" => Some("tsn_Latn"),
        "tpi" => Some("tpi_Latn"),
        "tr" => Some("tur_Latn"),
        "ts" => Some("tso_Latn"),
        "tt" => Some("tat_Cyrl"),
        "tum" => Some("tum_Latn"),
        "tzm" => Some("tzm_Tfng"),
        "ug" => Some("uig_Arab"),
        "uk" => Some("ukr_Cyrl"),
        "umb" => Some("umb_Latn"),
        "ur" => Some("urd_Arab"),
        "uz" | "uzn" => Some("uzn_Latn"),
        "vec" => Some("vec_Latn"),
        "vi" => Some("vie_Latn"),
        "war" => Some("war_Latn"),
        "wo" => Some("wol_Latn"),
        "xh" => Some("xho_Latn"),
        "ydd" => Some("ydd_Hebr"),
        "yo" => Some("yor_Latn"),
        "yue" => Some("yue_Hant"),
        "zh" | "zh-cn" | "zh-hans" => Some("zho_Hans"),
        "zh-hant" | "zh-tw" => Some("zho_Hant"),
        "zu" => Some("zul_Latn"),
        _ => None,
    }
}

/// Translates batch of texts to target NLLB language code. Blocking (CPU-bound).
fn translate_batch_blocking(
    texts: &[String],
    source_lang: &str,
    target_lang: &str,
) -> Result<Vec<String>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    // Join multi-line texts with spaces.
    let sources: Vec<String> = texts
        .iter()
        .map(|s| {
            format!(
                "{source_lang} {}",
                s.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .collect();

    let prefixes: Vec<Vec<String>> = sources
        .iter()
        .map(|_| vec![target_lang.to_string()])
        .collect();

    let results = {
        let mut guard = holder().lock().unwrap_or_else(|e| e.into_inner());
        let t = guard.get_or_load()?;
        t.translate_batch_with_target_prefix(&sources, &prefixes, &Default::default(), None)
            .map_err(|e| format!("translate_batch failed: {e}"))?
    };

    Ok(results
        .into_iter()
        .map(|(text, _score)| clean_special_tokens(&text))
        .collect())
}

/// Strips NLLB special tokens and language tags from decoded text.
fn clean_special_tokens(text: &str) -> String {
    let mut out = text.to_string();
    for tok in ["<unk>", "<s>", "</s>", "<pad>", "<mask>"] {
        out = out.replace(tok, "");
    }
    out = out
        .split_whitespace()
        .filter(|w| !is_nllb_lang_tag(w))
        .collect::<Vec<_>>()
        .join(" ");
    out.trim().to_string()
}

/// Returns true if word is an NLLB language tag.
fn is_nllb_lang_tag(w: &str) -> bool {
    let bytes = w.as_bytes();
    bytes.len() == 8
        && bytes[3] == b'_'
        && bytes[..3].iter().all(|c| c.is_ascii_lowercase())
        && bytes[4].is_ascii_uppercase()
        && bytes[5..].iter().all(|c| c.is_ascii_lowercase())
}

/// Reads a source SRT file, translates texts to NLLB target_lang, preserving timestamps.
pub async fn translate_srt<F>(
    input_path: &Path,
    output_path: &Path,
    source_lang: &str,
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
    let source_lang = source_lang.to_string();
    let target_lang = target_lang.to_string();
    let start_time = std::time::Instant::now();

    progress_cb(0, total_items, 0, 0);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(usize, usize, u64, u64)>(16);

    let handle = tokio::task::spawn_blocking(move || -> Result<Vec<SrtItem>, String> {
        let batch_size = 32;
        let mut processed = 0;
        for chunk in items.chunks_mut(batch_size) {
            let texts: Vec<String> = chunk.iter().map(|item| item.text.clone()).collect();
            let translated = translate_batch_blocking(&texts, &source_lang, &target_lang)?;
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
        // Regular words should not be misidentified as language tags.
        assert_eq!(clean_special_tokens("testing"), "testing");
    }

    #[tokio::test]
    #[ignore]
    async fn test_real_translation_ja_to_en() {
        let dir = crate::common::dir::TempDirGuard::create("nllb_ja_test", 0).unwrap();
        let input = dir.path().join("ja.srt");
        let output = dir.path().join("en.srt");
        let timestamp = "00:00:01,000 --> 00:00:03,000";
        std::fs::write(&input, format!("1\n{timestamp}\n今日はいい天気です。\n")).unwrap();
        translate_srt(&input, &output, "jpn_Jpan", "eng_Latn", 4, |_, _, _, _| {})
            .await
            .unwrap();
        let cues = parse_srt(&std::fs::read_to_string(output).unwrap());
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].timestamp, timestamp);
        assert!(
            cues[0].text.to_lowercase().contains("weather"),
            "{}",
            cues[0].text
        );
    }

    /// Real translation test with NLLB model. Requires `files/models/nllb`.
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

        translate_srt(&input, &output, "eng_Latn", "pes_Arab", 4, |_, _, _, _| {})
            .await
            .expect("translation failed");

        let result = std::fs::read_to_string(&output).unwrap();
        let items = parse_srt(&result);
        assert_eq!(items.len(), 2, "should keep both entries");
        assert_eq!(
            items[0].timestamp, "00:00:01,000 --> 00:00:03,000",
            "timing preserved"
        );
        // Output must be non-Latin (Persian), without special tokens, differing from English input.
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
