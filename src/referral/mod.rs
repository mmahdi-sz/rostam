use frankenstein::client_reqwest::Bot;
use tokio_postgres::Client;

use crate::rank::types::{Rank, ceil_div};

/// مدت انتظار قبل از تأیید نهایی یه دعوت (روز) — کاربر باید تا این مدت عضو قفل اجباری بماند.
pub const PENDING_DAYS: i64 = 2;

/// آستانه‌های تعداد دعوت برای هر رتبه، به ترتیب پلکانی زیرمجموعه‌گیری
/// (سهراب < اسفندیار < رستم) — این ترتیب با وزن عمومی رتبه‌ها (`Rank::weight`)
/// فرق دارد، چون آنجا اسفندیار/سهراب برای کد هدیه هم‌وزن تعریف شده‌اند.
pub const TIERS: &[(u32, Rank)] = &[
    (10, Rank::Sohrab),
    (20, Rank::Esfandyar),
    (50, Rank::Rostam),
];

/// جایگاه یک رتبه در پلکان زیرمجموعه‌گیری. رتبه‌های خارج از پلکان (دلاور/سپهبد) پایین‌ترین‌اند.
fn tier_position(rank: Rank) -> i32 {
    TIERS
        .iter()
        .position(|(_, r)| *r == rank)
        .map(|i| i as i32)
        .unwrap_or(-1)
}

/// مدت هر فعال‌سازی با امتیاز (روز).
pub const ACTIVATION_DAYS: i64 = 31;

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// شروع یه دعوت در انتظار تأیید. فقط وقتی `referred_id` برای اولین‌بار در بات دیده
/// شده باید صدا زده شود (توسط caller گیت می‌شود) — این تابع خودش هم با PK یکتا از
/// دوباره‌نویسی جلوگیری می‌کند. تأیید نهایی (و شمارش امتیاز) با `sweep_confirm` انجام
/// می‌شود، بعد از اینکه کاربر حداقل `PENDING_DAYS` روز عضو قفل اجباری مانده باشد.
pub async fn record_referral(client: &Client, referred_id: i64, referrer_id: i64) {
    let now = now_epoch();
    let r = client
        .execute(
            "INSERT INTO referral_pending (referred_id, referrer_id, started_at)
         VALUES ($1, $2, $3)
         ON CONFLICT (referred_id) DO NOTHING",
            &[&referred_id, &referrer_id, &now],
        )
        .await;
    if let Err(e) = r {
        eprintln!(
            "[referral event=pending_record_failed] referred_id={referred_id} referrer_id={referrer_id} err={e}"
        );
        crate::stats::record_error_global("referral", &e.to_string()).await;
    }
}

/// دعوت‌های در انتظاری که `PENDING_DAYS` روز از شروعشان گذشته را بررسی می‌کند:
/// عضو قفل اجباری مانده → تأیید و به `referrals` منتقل می‌شود (امتیاز می‌گیرد)؛
/// عضو نمانده → حذف می‌شود (باطل). توسط یه job دوره‌ای در startup.rs صدا زده می‌شود.
pub async fn sweep_confirm(client: &Client, api: &Bot) {
    let cutoff = now_epoch() - PENDING_DAYS * 86_400;
    let rows = match client
        .query(
            "SELECT referred_id, referrer_id FROM referral_pending WHERE started_at <= $1",
            &[&cutoff],
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[referral event=sweep_query_failed] err={e}");
            crate::stats::record_error_global("referral", &e.to_string()).await;
            return;
        }
    };

    let mut confirmed = 0u32;
    let mut discarded = 0u32;
    for row in rows {
        let referred_id: i64 = row.get(0);
        let referrer_id: i64 = row.get(1);

        if crate::force_join::is_joined(api, referred_id).await {
            let now = now_epoch();
            let r = client
                .execute(
                    "INSERT INTO referrals (referred_id, referrer_id, created_at)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (referred_id) DO NOTHING",
                    &[&referred_id, &referrer_id, &now],
                )
                .await;
            if let Err(e) = r {
                eprintln!(
                    "[referral event=confirm_insert_failed] referred_id={referred_id} err={e}"
                );
                crate::stats::record_error_global("referral", &e.to_string()).await;
                continue;
            }
            confirmed += 1;
        } else {
            discarded += 1;
        }

        let _ = client
            .execute(
                "DELETE FROM referral_pending WHERE referred_id = $1",
                &[&referred_id],
            )
            .await;
    }

    if confirmed > 0 || discarded > 0 {
        eprintln!("[referral event=sweep_done] confirmed={confirmed} discarded={discarded}");
    }
}

pub async fn count_referrals(client: &Client, referrer_id: i64) -> i64 {
    client
        .query_one(
            "SELECT COUNT(*) FROM referrals WHERE referrer_id = $1",
            &[&referrer_id],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(0)
}

/// تعداد دعوت‌های این referrer که هنوز در انتظار تأیید (۲ روز عضویت) هستند.
pub async fn count_pending(client: &Client, referrer_id: i64) -> i64 {
    client
        .query_one(
            "SELECT COUNT(*) FROM referral_pending WHERE referrer_id = $1",
            &[&referrer_id],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(0)
}

/// مجموع امتیازهایی که تا الان صرف فعال‌سازی رتبه شده.
pub async fn total_spent_points(client: &Client, user_id: i64) -> i64 {
    client
        .query_one(
            "SELECT COALESCE(SUM(points_spent), 0) FROM referral_activations WHERE user_id = $1",
            &[&user_id],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(0)
}

pub async fn record_activation(
    client: &Client,
    user_id: i64,
    rank: Rank,
    points_spent: i64,
    expires_at: i64,
) {
    let now = now_epoch();
    let r = client.execute(
        "INSERT INTO referral_activations (user_id, rank, points_spent, activated_at, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
        &[&user_id, &rank.as_str(), &(points_spent as i32), &now, &expires_at],
    ).await;
    if let Err(e) = r {
        eprintln!(
            "[referral event=record_activation_failed] user_id={user_id} rank={} err={e}",
            rank.as_str()
        );
        crate::stats::record_error_global("referral", &e.to_string()).await;
    }
}

/// نتیجه‌ی محاسبه‌ی فعال‌سازی بر اساس رتبه‌ی فعلی کاربر.
pub enum ActivationPlan {
    /// رتبه‌ی درخواستی پایین‌تر از رتبه‌ی فعال فعلی (در پلکان زیرمجموعه‌گیری) است.
    Reject,
    /// کاربر همین الان رتبه‌ی نامحدود دارد — فعال‌سازی با امتیاز فایده‌ای ندارد.
    AlreadyUnlimited,
    /// اعمال شود؛ همون فرمول وزنی کد هدیه (`redeem::plan_redeem`): اگر رتبه‌ی فعال
    /// هم‌ارز/بالاتر باشد، باقیمانده‌ی روزهایش با نسبت وزن تبدیل و با ۳۱ روز جدید جمع می‌شود؛
    /// وگرنه فقط ۳۱ روز از همین لحظه.
    Apply { rank: Rank, expires_at: i64 },
}

/// موجودی امتیازهای زیرمجموعه‌گیری قابل خرج (تعداد کل - خرج شده)
#[allow(dead_code)]
pub fn available_points(total_referrals: i64, total_spent: i64) -> i64 {
    total_referrals.saturating_sub(total_spent).max(0)
}

/// آیا موجودی کاربر برای دریافت یک آستانه (پلکان) کافی است؟
#[allow(dead_code)]
pub fn can_claim_tier(total_referrals: i64, total_spent: i64, tier_threshold: u32) -> bool {
    available_points(total_referrals, total_spent) >= tier_threshold as i64
}

/// محاسبه‌ی روزهای تبدیل‌شده از رتبه‌ی قبلی به رتبه‌ی جدید با نسبت وزن‌ها
pub fn calculate_converted_days(remaining_days: i64, cur_weight: i64, target_weight: i64) -> i64 {
    if target_weight == cur_weight || target_weight == 0 {
        remaining_days
    } else {
        ceil_div(remaining_days.saturating_mul(cur_weight), target_weight)
    }
}

pub async fn plan_activation(client: &Client, user_id: i64, tier_rank: Rank) -> ActivationPlan {
    let now = now_epoch();
    let cur = crate::rank::store::get_user_rank(client, user_id)
        .await
        .ok()
        .flatten();

    let Some(cur) = cur else {
        return ActivationPlan::Apply {
            rank: tier_rank,
            expires_at: now + ACTIVATION_DAYS * 86_400,
        };
    };

    let active = match cur.expires_at {
        Some(exp) => exp > now,
        None => true,
    };

    if !active {
        return ActivationPlan::Apply {
            rank: tier_rank,
            expires_at: now + ACTIVATION_DAYS * 86_400,
        };
    }

    // ترتیب اختصاصی پلکان زیرمجموعه‌گیری، نه وزن عمومی رتبه‌ها.
    if tier_position(tier_rank) < tier_position(cur.rank) {
        return ActivationPlan::Reject;
    }

    let Some(cur_exp) = cur.expires_at else {
        return ActivationPlan::AlreadyUnlimited;
    };

    let wc = cur.rank.weight();
    let wn = tier_rank.weight();
    let remaining_days = ceil_div((cur_exp - now).max(0), 86_400);
    let converted = calculate_converted_days(remaining_days, wc, wn);
    let total_days = ACTIVATION_DAYS + converted;
    ActivationPlan::Apply {
        rank: tier_rank,
        expires_at: now + total_days * 86_400,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_tier_position() {
        assert_eq!(tier_position(Rank::Sohrab), 0);
        assert_eq!(tier_position(Rank::Esfandyar), 1);
        assert_eq!(tier_position(Rank::Rostam), 2);
        assert_eq!(tier_position(Rank::Dalavar), -1);
        assert_eq!(tier_position(Rank::Sepahbod), -1);
    }

    #[test]
    fn test_tiers_order_and_counts() {
        assert_eq!(TIERS.len(), 3);
        assert_eq!(TIERS[0], (10, Rank::Sohrab));
        assert_eq!(TIERS[1], (20, Rank::Esfandyar));
        assert_eq!(TIERS[2], (50, Rank::Rostam));
    }

    #[test]
    fn test_available_points() {
        assert_eq!(available_points(15, 10), 5);
        assert_eq!(available_points(10, 10), 0);
        assert_eq!(available_points(5, 10), 0);
    }

    #[test]
    fn test_can_claim_tier() {
        assert!(can_claim_tier(20, 10, 10)); // 10 points left, cost 10 -> ok
        assert!(!can_claim_tier(15, 10, 10)); // 5 points left, cost 10 -> insufficient
        assert!(can_claim_tier(50, 0, 50)); // 50 points left, cost 50 -> ok
        assert!(!can_claim_tier(49, 0, 50)); // 49 points left, cost 50 -> insufficient
    }

    #[test]
    fn test_calculate_converted_days() {
        // Same weight
        assert_eq!(calculate_converted_days(10, 5, 5), 10);
        // Upgrade from weight 5 to 10
        assert_eq!(calculate_converted_days(10, 5, 10), 5); // 10 * 5 / 10 = 5
        assert_eq!(calculate_converted_days(7, 5, 10), 4); // ceil(35 / 10) = 4
        // Edge cases
        assert_eq!(calculate_converted_days(0, 5, 10), 0);
    }

    #[test]
    fn test_referral_constants() {
        assert_eq!(PENDING_DAYS, 2);
        assert_eq!(ACTIVATION_DAYS, 31);
    }

    #[test]
    fn test_render_leaderboard_text_formatting() {
        let sample = vec![
            TopReferrer {
                user_id: 1001,
                username: Some("mmahdi_sz".to_string()),
                referral_count: 25,
            },
            TopReferrer {
                user_id: 1002,
                username: Some("username".to_string()),
                referral_count: 20,
            },
            TopReferrer {
                user_id: 1003,
                username: Some("third_user".to_string()),
                referral_count: 15,
            },
            TopReferrer {
                user_id: 1004,
                username: Some("fourth_user".to_string()),
                referral_count: 10,
            },
        ];

        let rendered = render_leaderboard_text(&sample);
        assert!(rendered.contains("\u{200F}🥇 mmahdi\\_sz : 25"));
        assert!(rendered.contains("\u{200F}🥈 username : 20"));
        assert!(rendered.contains("\u{200F}🥉 third\\_user : 15"));
        assert!(rendered.contains("\u{200F}4\\. fourth\\_user : 10"));
        assert!(rendered.contains('\u{200F}'));
    }

    #[test]
    fn test_render_leaderboard_text_empty() {
        let rendered = render_leaderboard_text(&[]);
        assert!(!rendered.is_empty());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TopReferrer {
    pub user_id: i64,
    pub username: Option<String>,
    pub referral_count: i64,
}

pub async fn get_top_referrers(client: &Client, limit: i64) -> Vec<TopReferrer> {
    let rows = match client
        .query(
            "SELECT r.referrer_id, COUNT(*) AS count, u.username
             FROM referrals r
             LEFT JOIN stats_users u ON r.referrer_id = u.user_id
             GROUP BY r.referrer_id, u.username
             ORDER BY count DESC, r.referrer_id ASC
             LIMIT $1",
            &[&limit],
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[referral event=get_top_referrers_failed] err={e}");
            crate::stats::record_error_global("referral", &e.to_string()).await;
            return Vec::new();
        }
    };

    rows.into_iter()
        .map(|row| TopReferrer {
            user_id: row.get(0),
            referral_count: row.get(1),
            username: row.get(2),
        })
        .collect()
}

pub fn render_leaderboard_text(top_users: &[TopReferrer]) -> String {
    let header = crate::i18n::t("start.leaderboard_title");
    if top_users.is_empty() {
        let empty_msg = crate::i18n::t("start.leaderboard_empty");
        return format!("{header}\n\n{empty_msg}");
    }

    let mut list_lines = Vec::new();
    for (idx, top) in top_users.iter().enumerate() {
        let rank_num = idx + 1;
        let rank_prefix = match rank_num {
            1 => "🥇".to_string(),
            2 => "🥈".to_string(),
            3 => "🥉".to_string(),
            n => format!("{n}\\."),
        };
        let fallback = format!("user_{}", top.user_id);
        let display_name = top
            .username
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&fallback);
        let escaped_name = crate::i18n::md_escape(display_name);
        list_lines.push(format!(
            "\u{200F}{rank_prefix} {escaped_name} : {}",
            top.referral_count
        ));
    }

    let list_str = list_lines.join("\n");
    format!("{header}\n\n{list_str}")
}

