//! Admin-only panel: statistics, error monitoring, and bot management.
//!
//! Accessible only to `ADMIN_USER_ID`. Callbacks prefixed `admin:`.
//! Renders: stats, errors-1d, force-join controls, redeem code generation.

pub mod broadcast;

use crate::bot::constants::CB_ADMIN_SECTION;
use crate::i18n::{t, tf, to_fa_digits};
use crate::stats::{
    FeatureStats, count_recent_errors, fmt_bytes, fmt_secs, get_action_breakdown_multi,
    get_active_users, get_download_stats, get_feature_stats_multi, get_recent_errors,
    get_user_stats,
};
use tokio_postgres::Client;

/// One tracked `stats_events` feature. `timed` = `amount` holds seconds, not a count.
pub struct Feat {
    pub key: &'static str,
    pub name_key: &'static str,
    pub timed: bool,
}

const fn feat(key: &'static str, name_key: &'static str, timed: bool) -> Feat {
    Feat {
        key,
        name_key,
        timed,
    }
}

const F_AI: &[Feat] = &[
    feat("stt", "admin.stats.names.stt", true),
    feat("denoise", "admin.stats.names.denoise", true),
    feat("upscale", "admin.stats.names.upscale", false),
    feat("gwm", "admin.stats.names.gwm", false),
    feat("nobg", "admin.stats.names.nobg", false),
    feat("deoldify", "admin.stats.names.deoldify", false),
    feat("tts", "admin.stats.names.tts", false),
];

const F_MUSIC: &[Feat] = &[
    feat("spotify", "admin.stats.names.spotify", false),
    feat("soundcloud", "admin.stats.names.soundcloud", false),
    feat("musicset", "admin.stats.names.musicset", false),
    feat("separation", "admin.stats.names.separation", true),
];

const F_FILES: &[Feat] = &[
    feat("pdfcompress", "admin.stats.names.pdfcompress", false),
    feat("surge_dl", "admin.stats.names.surge_dl", false),
    feat("emoji", "admin.stats.names.emoji", false),
    feat("ip_lookup", "admin.stats.names.ip_lookup", false),
];

const F_MONEY: &[Feat] = &[
    feat("paywall", "admin.stats.names.paywall", false),
    feat("rank", "admin.stats.names.rank", false),
    feat("referral", "admin.stats.names.referral", false),
];

const F_SYS: &[Feat] = &[
    feat("cpu", "admin.stats.names.cpu", false),
    feat("cookie", "admin.stats.names.cookie", false),
    feat("broadcast", "admin.stats.names.broadcast", false),
];

/// A navigable page of the stats panel. `feats` empty = hand-rolled renderer.
pub struct Section {
    pub key: &'static str,
    pub label_key: &'static str,
    pub feats: &'static [Feat],
}

pub const SEC_OVERVIEW: &str = "ov";
pub const SEC_USERS: &str = "users";
pub const SEC_YOUTUBE: &str = "yt";
pub const SEC_ERRORS: &str = "err";

pub const SECTIONS: &[Section] = &[
    Section {
        key: SEC_OVERVIEW,
        label_key: "admin.stats.nav.overview",
        feats: &[],
    },
    Section {
        key: SEC_USERS,
        label_key: "admin.stats.nav.users",
        feats: &[],
    },
    Section {
        key: SEC_YOUTUBE,
        label_key: "admin.stats.nav.youtube",
        feats: &[],
    },
    Section {
        key: "ai",
        label_key: "admin.stats.nav.ai",
        feats: F_AI,
    },
    Section {
        key: "music",
        label_key: "admin.stats.nav.music",
        feats: F_MUSIC,
    },
    Section {
        key: "files",
        label_key: "admin.stats.nav.files",
        feats: F_FILES,
    },
    Section {
        key: "money",
        label_key: "admin.stats.nav.money",
        feats: F_MONEY,
    },
    Section {
        key: "sys",
        label_key: "admin.stats.nav.system",
        feats: F_SYS,
    },
    Section {
        key: SEC_ERRORS,
        label_key: "admin.stats.nav.errors",
        feats: &[],
    },
];

/// Max action rows printed per feature.
const MAX_ROWS: usize = 8;

/// Success-rate alert threshold, and the sample floor below which the rate is noise.
const ALERT_RATE: i64 = 80;
const ALERT_MIN_SAMPLES: i64 = 5;

pub fn section(key: &str) -> Option<&'static Section> {
    SECTIONS.iter().find(|s| s.key == key)
}

fn all_feats() -> impl Iterator<Item = &'static Feat> {
    SECTIONS.iter().flat_map(|s| s.feats.iter())
}

/// Label for a raw `stats_events` feature key; falls back to the key itself.
fn feature_label(raw: &str) -> String {
    if raw == "youtube" {
        return t("admin.stats.names.youtube");
    }
    all_feats()
        .find(|f| f.key == raw)
        .map(|f| t(f.name_key))
        .unwrap_or_else(|| raw.to_string())
}

/// Success rate over 30 days, `None` when there is nothing to divide.
fn success_rate(fs: &FeatureStats) -> Option<i64> {
    let total = fs.ok.d30 + fs.fail.d30;
    (total > 0).then(|| fs.ok.d30 * 100 / total)
}

fn render_feature_block(name: &str, fs: &FeatureStats, timed: bool, rows: &[ActionRow]) -> String {
    let mut block = format!("\n━━ {name} ━━\n");
    block.push_str(&tf(
        "admin.stats.feat_ok",
        &[
            ("ok_1d", &fs.ok.d1.to_string()),
            ("ok_7d", &fs.ok.d7.to_string()),
            ("ok_30d", &fs.ok.d30.to_string()),
            ("fail_30d", &fs.fail.d30.to_string()),
            (
                "rate",
                &success_rate(fs).map_or("—".to_string(), |r| format!("{r}%")),
            ),
        ],
    ));
    if timed && fs.amount.d30 > 0 {
        let avg = if fs.ok.d30 > 0 {
            fs.amount.d30 / fs.ok.d30
        } else {
            0
        };
        block.push_str(&tf(
            "admin.stats.feat_time",
            &[("t_30d", &fmt_secs(fs.amount.d30)), ("avg", &fmt_secs(avg))],
        ));
    }
    block.push_str(&render_action_rows(rows));
    block
}

/// One `(action, status)` breakdown row, already labelled.
struct ActionRow {
    label: String,
    d1: i64,
    d7: i64,
    d30: i64,
}

fn render_action_rows(rows: &[ActionRow]) -> String {
    let mut out = String::new();
    for r in rows.iter().take(MAX_ROWS) {
        out.push_str(&format!(
            "• {}:  1d {}  |  7d {}  |  30d {}\n",
            r.label, r.d1, r.d7, r.d30
        ));
    }
    if rows.len() > MAX_ROWS {
        out.push_str(&tf(
            "admin.stats_more_rows",
            &[("n", &(rows.len() - MAX_ROWS).to_string())],
        ));
    }
    out
}

async fn action_rows(
    client: &Client,
    features: &[&str],
) -> std::collections::HashMap<String, Vec<ActionRow>> {
    let raw = match get_action_breakdown_multi(client, features).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[admin event=stats_breakdown_failed] err={e}");
            return std::collections::HashMap::new();
        }
    };
    raw.into_iter()
        .map(|(feature, rows)| {
            let rows = rows
                .into_iter()
                .map(|r| ActionRow {
                    label: if r.status.is_empty() || r.status == "ok" {
                        r.action
                    } else {
                        format!("{} · {}", r.action, r.status)
                    },
                    d1: r.d1,
                    d7: r.d7,
                    d30: r.d30,
                })
                .collect();
            (feature, rows)
        })
        .collect()
}

/// Rendered stats page. `html` = must be sent with `ParseMode::Html`.
pub struct SectionView {
    pub text: String,
    pub html: bool,
}

/// Renders one stats page. Unknown key falls back to the overview.
pub async fn render_section(client: &Client, key: &str) -> SectionView {
    match key {
        SEC_ERRORS => SectionView {
            text: render_errors_1d(client).await,
            html: true,
        },
        SEC_USERS => SectionView {
            text: render_users(client).await,
            html: false,
        },
        SEC_YOUTUBE => SectionView {
            text: render_youtube(client).await,
            html: false,
        },
        _ => match section(key).filter(|s| !s.feats.is_empty()) {
            Some(s) => SectionView {
                text: render_feats(client, s).await,
                html: false,
            },
            None => SectionView {
                text: render_overview(client).await,
                html: false,
            },
        },
    }
}

/// Section nav keyboard: 2 per row, current one marked, then back to the panel.
pub fn stats_keyboard(current: &str) -> frankenstein::types::InlineKeyboardMarkup {
    use crate::emoji::panel::{btn_icon, btn_icon_success};
    let mut rows: Vec<Vec<frankenstein::types::InlineKeyboardButton>> = Vec::new();
    for pair in SECTIONS.chunks(2) {
        rows.push(
            pair.iter()
                .map(|s| {
                    let cb = format!("{CB_ADMIN_SECTION}{}", s.key);
                    let label = t(s.label_key);
                    if s.key == current {
                        btn_icon_success(&label, &cb, "stats")
                    } else {
                        btn_icon(&label, &cb, "stats")
                    }
                })
                .collect(),
        );
    }
    rows.push(vec![btn_icon(
        &t("admin.back"),
        crate::bot::CB_ADMIN_PANEL,
        "back",
    )]);
    frankenstein::types::InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

// ── overview ─────────────────────────────────────────────────────────────

async fn render_overview(client: &Client) -> String {
    let mut out = t("admin.stats.hub_title");

    match get_user_stats(client).await {
        Ok(u) => out.push_str(&tf(
            "admin.stats.overview_users",
            &[
                ("total", &u.total.to_string()),
                ("new_1d", &u.new_1d.to_string()),
                ("new_7d", &u.new_7d.to_string()),
                ("new_30d", &u.new_30d.to_string()),
            ],
        )),
        Err(e) => {
            eprintln!("[admin event=stats_users_failed] err={e}");
            out.push_str(&t("admin.stats_error"));
        }
    }

    if let Ok(a) = get_active_users(client).await {
        out.push_str(&tf(
            "admin.stats.overview_active",
            &[
                ("dau", &a.dau.to_string()),
                ("wau", &a.wau.to_string()),
                ("top_name", &feature_label(&a.top_feature)),
                ("top_count", &a.top_feature_count.to_string()),
            ],
        ));
    }

    let keys: Vec<&str> = all_feats().map(|f| f.key).collect();
    let stats = get_feature_stats_multi(client, &keys)
        .await
        .unwrap_or_default();
    let mut alerts = String::new();
    for f in all_feats() {
        let Some(fs) = stats.get(f.key) else { continue };
        let total = fs.ok.d30 + fs.fail.d30;
        let Some(rate) = success_rate(fs) else {
            continue;
        };
        if total >= ALERT_MIN_SAMPLES && rate < ALERT_RATE {
            alerts.push_str(&tf(
                "admin.stats.alert_rate",
                &[
                    ("name", &t(f.name_key)),
                    ("rate", &rate.to_string()),
                    ("ok", &fs.ok.d30.to_string()),
                    ("fail", &fs.fail.d30.to_string()),
                ],
            ));
        }
    }
    let errors = count_recent_errors(client).await.unwrap_or(0);
    if errors > 0 {
        alerts.push_str(&tf(
            "admin.stats.alert_errors",
            &[("count", &errors.to_string())],
        ));
    }
    out.push_str(&t("admin.stats.alerts_title"));
    out.push_str(
        if alerts.is_empty() {
            t("admin.stats.alerts_none")
        } else {
            alerts
        }
        .as_str(),
    );

    out
}

// ── users ────────────────────────────────────────────────────────────────

async fn render_users(client: &Client) -> String {
    let users = match get_user_stats(client).await {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[admin event=stats_users_failed] err={e}");
            return t("admin.stats_error");
        }
    };
    let active = match get_active_users(client).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[admin event=stats_active_failed] err={e}");
            return t("admin.stats_users_error");
        }
    };

    let mut out = tf(
        "admin.stats.users",
        &[
            ("total", &users.total.to_string()),
            ("new_1d", &users.new_1d.to_string()),
            ("new_3d", &users.new_3d.to_string()),
            ("new_7d", &users.new_7d.to_string()),
            ("new_30d", &users.new_30d.to_string()),
        ],
    );
    out.push_str(&tf(
        "admin.stats.active",
        &[
            ("dau", &active.dau.to_string()),
            ("wau", &active.wau.to_string()),
            ("returning", &active.returning_1d.to_string()),
            ("top_name", &feature_label(&active.top_feature)),
            ("top_count", &active.top_feature_count.to_string()),
        ],
    ));
    out
}

// ── youtube ──────────────────────────────────────────────────────────────

async fn render_youtube(client: &Client) -> String {
    let dl = match get_download_stats(client).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[admin event=stats_downloads_failed] err={e}");
            return t("admin.stats_dl_error");
        }
    };
    let mut out = tf(
        "admin.stats.youtube",
        &[
            ("req_1d", &dl.requests_1d.to_string()),
            ("req_3d", &dl.requests_3d.to_string()),
            ("req_7d", &dl.requests_7d.to_string()),
            ("req_30d", &dl.requests_30d.to_string()),
            ("dl_1d", &fmt_bytes(dl.bytes_downloaded_1d)),
            ("dl_3d", &fmt_bytes(dl.bytes_downloaded_3d)),
            ("dl_7d", &fmt_bytes(dl.bytes_downloaded_7d)),
            ("dl_30d", &fmt_bytes(dl.bytes_downloaded_30d)),
            ("up_1d", &dl.uploads_ok_1d.to_string()),
            ("up_3d", &dl.uploads_ok_3d.to_string()),
            ("up_7d", &dl.uploads_ok_7d.to_string()),
            ("up_30d", &dl.uploads_ok_30d.to_string()),
            ("upb_1d", &fmt_bytes(dl.bytes_uploaded_1d)),
            ("upb_3d", &fmt_bytes(dl.bytes_uploaded_3d)),
            ("upb_7d", &fmt_bytes(dl.bytes_uploaded_7d)),
            ("upb_30d", &fmt_bytes(dl.bytes_uploaded_30d)),
        ],
    );

    let rows = action_rows(client, &["youtube"]).await;
    out.push_str(&format!("\n━━ {} ━━\n", t("admin.stats.more.youtube")));
    match rows.get("youtube") {
        Some(r) if !r.is_empty() => out.push_str(&render_action_rows(r)),
        _ => out.push_str(&t("admin.stats_no_data")),
    }
    out
}

// ── feature sections ─────────────────────────────────────────────────────

async fn render_feats(client: &Client, sec: &Section) -> String {
    let keys: Vec<&str> = sec.feats.iter().map(|f| f.key).collect();
    let stats = get_feature_stats_multi(client, &keys)
        .await
        .unwrap_or_else(|e| {
            eprintln!(
                "[admin event=stats_feature_failed] section={} err={e}",
                sec.key
            );
            Default::default()
        });
    let rows = action_rows(client, &keys).await;

    let mut out = format!("📊 {}\n", t(sec.label_key));
    let empty: Vec<ActionRow> = Vec::new();
    for f in sec.feats {
        let name = t(f.name_key);
        match stats.get(f.key) {
            Some(fs) => out.push_str(&render_feature_block(
                &name,
                fs,
                f.timed,
                rows.get(f.key).unwrap_or(&empty),
            )),
            None => {
                out.push_str(&format!("\n━━ {name} ━━\n"));
                out.push_str(&t("admin.stats_no_data"));
            }
        }
    }
    out
}

// ── 24-Hour Errors ───────────────────────────────────────────────────

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// HTML output (expandable blockquote) sent with ParseMode::Html
pub async fn render_errors_1d(client: &Client) -> String {
    let count = count_recent_errors(client).await.unwrap_or(0);
    if count == 0 {
        return to_fa_digits(&t("admin.no_errors"));
    }

    let rows = match get_recent_errors(client, 40).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[admin event=errors_failed] err={e}");
            return t("admin.errors_fetch_error");
        }
    };

    let header = tf("admin.errors_header", &[("count", &count.to_string())]);
    let mut body = String::new();
    for r in &rows {
        let line = format!("{}m · {}: {}", r.minutes_ago, r.feature, r.message);
        body.push_str(&html_escape(&line));
        body.push('\n');
    }
    // Expandable blockquote
    format!(
        "{}<blockquote expandable>{}</blockquote>",
        html_escape(&header),
        body.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_feature_label() {
        let label = feature_label("stt");
        assert!(!label.is_empty());
        let unknown = feature_label("unknown_feat");
        assert_eq!(unknown, "unknown_feat");
    }

    #[test]
    fn test_admin_html_escape() {
        assert_eq!(
            html_escape("<script>&</script>"),
            "&lt;script&gt;&amp;&lt;/script&gt;"
        );
    }

    #[test]
    fn test_admin_render_feature_block() {
        let fs = FeatureStats {
            ok: crate::stats::Periods {
                d1: 1,
                d3: 2,
                d7: 3,
                d30: 4,
            },
            fail: crate::stats::Periods {
                d1: 0,
                d3: 0,
                d7: 0,
                d30: 1,
            },
            amount: crate::stats::Periods {
                d1: 10,
                d3: 20,
                d7: 30,
                d30: 40,
            },
        };
        let rows = vec![ActionRow {
            label: "run".to_string(),
            d1: 1,
            d7: 2,
            d30: 3,
        }];
        let block = render_feature_block("STT", &fs, true, &rows);
        assert!(block.contains("STT"));
        assert!(block.contains("run"));
        // 4 ok / 1 fail over 30d
        assert_eq!(success_rate(&fs), Some(80));
    }

    #[test]
    fn test_admin_sections_unique_and_navigable() {
        for s in SECTIONS {
            assert!(section(s.key).is_some());
            assert_eq!(SECTIONS.iter().filter(|o| o.key == s.key).count(), 1);
        }
        // spotify + soundcloud must be tracked somewhere
        for key in ["spotify", "soundcloud", "musicset", "referral", "tts"] {
            assert!(all_feats().any(|f| f.key == key), "missing {key}");
        }
    }

    #[test]
    fn test_admin_action_rows_cap() {
        let rows: Vec<ActionRow> = (0..MAX_ROWS + 3)
            .map(|i| ActionRow {
                label: format!("a{i}"),
                d1: 0,
                d7: 0,
                d30: 0,
            })
            .collect();
        let out = render_action_rows(&rows);
        assert_eq!(out.matches('•').count(), MAX_ROWS);
    }
}
