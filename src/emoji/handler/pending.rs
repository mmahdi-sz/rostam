use crate::i18n::{t, tf};
use crate::emoji::{PendingEmoji, import as emoji_import};

pub(super) fn apply_edit_ops(collected: &mut Vec<PendingEmoji>, text: &str) -> Result<(), &'static str> {
    let mut plus: Vec<usize> = Vec::new();
    let mut minus: Vec<usize> = Vec::new();
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix('+') {
            if let Ok(idx) = rest.parse::<usize>() { plus.push(idx); continue; }
        }
        if let Some(rest) = token.strip_prefix('-') {
            if let Ok(idx) = rest.parse::<usize>() { minus.push(idx); continue; }
        }
    }
    if !plus.is_empty() && !minus.is_empty() { return Err("mixed"); }
    if !plus.is_empty() {
        let snapshot = collected.clone();
        collected.clear();
        for idx in plus {
            if idx >= 1 && idx <= snapshot.len() {
                let candidate = snapshot[idx - 1].clone();
                if !collected.iter().any(|e| e.custom_emoji_id == candidate.custom_emoji_id) {
                    collected.push(candidate);
                }
            }
        }
    } else if !minus.is_empty() {
        let mut to_remove: Vec<usize> = minus
            .into_iter().filter(|i| *i >= 1 && *i <= collected.len()).map(|i| i - 1).collect();
        to_remove.sort_unstable();
        to_remove.dedup();
        for idx in to_remove.into_iter().rev() { collected.remove(idx); }
    }
    Ok(())
}

pub(super) fn build_import_report(a: &emoji_import::ImportAnalysis) -> String {
    if a.db_empty {
        format!(
            "{}\n\n{}\n\n{}",
            tf("emoji.import.file_stats", &[("packs", &a.file_packs.to_string()), ("items", &a.file_items.to_string())]),
            t("emoji.import.db_empty"),
            t("emoji.import.hint_empty"),
        )
    } else {
        format!(
            "{}\n\n{}\n\n{}\n{}\n{}",
            tf("emoji.import.file_stats", &[("packs", &a.file_packs.to_string()), ("items", &a.file_items.to_string())]),
            tf("emoji.import.db_stats", &[("packs", &a.db_packs.to_string()), ("items", &a.db_items.to_string()), ("dupes", &a.duplicate_items.to_string())]),
            t("emoji.import.hint_replace"),
            t("emoji.import.hint_merge"),
            t("emoji.import.hint_smart"),
        )
    }
}
