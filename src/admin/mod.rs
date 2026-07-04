use crate::i18n::{t, tf, to_fa_digits};
use crate::stats::{
    get_user_stats, get_download_stats, get_active_users, get_feature_stats,
    get_action_breakdown, get_recent_errors, count_recent_errors,
    fmt_bytes, fmt_secs, FeatureStats,
};
use tokio_postgres::Client;

// فیچرهای AI که در پنل آمار اصلی نشون داده می‌شن: (feature_key, نام i18n, آیا مدت زمان داره).
// مدت‌دارها amount=ثانیه ثبت می‌کنن (stt/denoise/separation/asr)؛ بقیه amount=تعداد.
const AI_FEATURES: &[(&str, &str, bool)] = &[
    ("stt",        "admin.stats.names.stt",        true),
    ("denoise",    "admin.stats.names.denoise",    true),
    ("upscale",    "admin.stats.names.upscale",    false),
    ("separation", "admin.stats.names.separation", true),
    ("gwm",        "admin.stats.names.gwm",        false),
    ("asr",        "admin.stats.names.asr",        true),
];

// نام raw فیچر → نام فارسی برای «پرمصرف‌ترین فیچر». youtube جداست (در stats_downloads).
fn feature_label(raw: &str) -> String {
    let key = match raw {
        "stt"        => "admin.stats.names.stt",
        "denoise"    => "admin.stats.names.denoise",
        "upscale"    => "admin.stats.names.upscale",
        "separation" => "admin.stats.names.separation",
        "gwm"        => "admin.stats.names.gwm",
        "asr"        => "admin.stats.names.asr",
        "youtube"    => "admin.stats.names.youtube",
        _            => return raw.to_string(),
    };
    t(key)
}

fn render_feature_block(name: &str, fs: &FeatureStats, with_time: bool) -> String {
    let mut block = tf("admin.stats.feature_count", &[
        ("name", name),
        ("ok_1d",  &fs.ok.d1.to_string()),
        ("ok_3d",  &fs.ok.d3.to_string()),
        ("ok_7d",  &fs.ok.d7.to_string()),
        ("ok_30d", &fs.ok.d30.to_string()),
        ("fail_30d", &fs.fail.d30.to_string()),
    ]);
    if with_time {
        block.push_str(&tf("admin.stats.feature_time", &[
            ("t_1d",  &fmt_secs(fs.amount.d1)),
            ("t_3d",  &fmt_secs(fs.amount.d3)),
            ("t_7d",  &fmt_secs(fs.amount.d7)),
            ("t_30d", &fmt_secs(fs.amount.d30)),
        ]));
    }
    block
}

pub async fn render_stats(client: &Client) -> String {
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
    let dl = match get_download_stats(client).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[admin event=stats_downloads_failed] err={e}");
            return t("admin.stats_dl_error");
        }
    };

    let mut out = String::new();

    // ── کاربران ──
    out.push_str(&tf("admin.stats.users", &[
        ("total",   &users.total.to_string()),
        ("new_1d",  &users.new_1d.to_string()),
        ("new_3d",  &users.new_3d.to_string()),
        ("new_7d",  &users.new_7d.to_string()),
        ("new_30d", &users.new_30d.to_string()),
    ]));

    // ── کاربران فعال (DAU/WAU/بازگشت/پرمصرف‌ترین) ──
    // پرمصرف‌ترین فیچر: مقایسه‌ی پرمصرف‌ترین فیچر AI با درخواست‌های یوتیوب (۷ روز).
    let (top_label, top_count) = if dl.requests_7d >= active.top_feature_count {
        (feature_label("youtube"), dl.requests_7d)
    } else {
        (feature_label(&active.top_feature), active.top_feature_count)
    };
    out.push_str(&tf("admin.stats.active", &[
        ("dau",       &active.dau.to_string()),
        ("wau",       &active.wau.to_string()),
        ("returning", &active.returning_1d.to_string()),
        ("top_name",  &top_label),
        ("top_count", &top_count.to_string()),
    ]));

    // ── یوتیوب دانلودر ──
    out.push_str(&tf("admin.stats.youtube", &[
        ("req_1d",  &dl.requests_1d.to_string()),
        ("req_3d",  &dl.requests_3d.to_string()),
        ("req_7d",  &dl.requests_7d.to_string()),
        ("req_30d", &dl.requests_30d.to_string()),

        ("dl_1d",   &fmt_bytes(dl.bytes_downloaded_1d)),
        ("dl_3d",   &fmt_bytes(dl.bytes_downloaded_3d)),
        ("dl_7d",   &fmt_bytes(dl.bytes_downloaded_7d)),
        ("dl_30d",  &fmt_bytes(dl.bytes_downloaded_30d)),

        ("up_1d",   &dl.uploads_ok_1d.to_string()),
        ("up_3d",   &dl.uploads_ok_3d.to_string()),
        ("up_7d",   &dl.uploads_ok_7d.to_string()),
        ("up_30d",  &dl.uploads_ok_30d.to_string()),

        ("upb_1d",  &fmt_bytes(dl.bytes_uploaded_1d)),
        ("upb_3d",  &fmt_bytes(dl.bytes_uploaded_3d)),
        ("upb_7d",  &fmt_bytes(dl.bytes_uploaded_7d)),
        ("upb_30d", &fmt_bytes(dl.bytes_uploaded_30d)),
    ]));

    // ── فیچرهای هوش مصنوعی ──
    for (feature, name_key, with_time) in AI_FEATURES {
        let fs = match get_feature_stats(client, feature).await {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[admin event=stats_feature_failed] feature={feature} err={e}");
                continue;
            }
        };
        out.push_str(&render_feature_block(&t(name_key), &fs, *with_time));
    }

    // اعداد آمار انگلیسی می‌مانند (خوانایی بهتر برای ادمین).
    out
}

// ── «آمار بیشتر» (فیچرهای ۷–۱۱) ──────────────────────────────────────────────────

// فیچرهایی که در «آمار بیشتر» تفکیک action نشون داده می‌شن.
const MORE_FEATURES: &[(&str, &str)] = &[
    ("youtube",  "admin.stats.more.youtube"),
    ("emoji",    "admin.stats.more.emoji"),
    ("cookie",   "admin.stats.more.cookie"),
    ("paywall",  "admin.stats.more.paywall"),
    ("cpu",      "admin.stats.more.cpu"),
];

const MORE_MAX_ROWS: usize = 14;

pub async fn render_stats_more(client: &Client) -> String {
    let mut out = t("admin.stats_more_header");

    for (feature, name_key) in MORE_FEATURES {
        let rows = match get_action_breakdown(client, feature).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[admin event=stats_more_failed] feature={feature} err={e}");
                continue;
            }
        };
        out.push_str(&format!("\n━━ {} ━━\n", t(name_key)));
        if rows.is_empty() {
            out.push_str(&t("admin.stats_no_data"));
            continue;
        }
        for row in rows.iter().take(MORE_MAX_ROWS) {
            // خط: • action · status: 1d | 7d | 30d
            let label = if row.status.is_empty() || row.status == "ok" {
                row.action.clone()
            } else {
                format!("{} · {}", row.action, row.status)
            };
            out.push_str(&format!(
                "• {label}:  1d {}  |  7d {}  |  30d {}\n",
                row.d1, row.d7, row.d30
            ));
        }
        if rows.len() > MORE_MAX_ROWS {
            out.push_str(&tf("admin.stats_more_rows", &[("n", &(rows.len() - MORE_MAX_ROWS).to_string())]));
        }
    }

    out
}

// ── «خطاهای ۱ روز گذشته» ─────────────────────────────────────────────────────────

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// خروجی HTML (نقل‌قول جمع‌شو). با ParseMode::Html فرستاده می‌شه.
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
    // blockquote جمع‌شو — کاربر می‌زنه باز می‌شه.
    format!("{}<blockquote expandable>{}</blockquote>", html_escape(&header), body.trim_end())
}
