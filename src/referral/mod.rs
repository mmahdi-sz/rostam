use frankenstein::client_reqwest::Bot;
use tokio_postgres::Client;

use crate::rank::types::{Rank, ceil_div};

/// مدت انتظار قبل از تأیید نهایی یه دعوت (روز) — کاربر باید تا این مدت عضو قفل اجباری بماند.
pub const PENDING_DAYS: i64 = 2;

/// آستانه‌های تعداد دعوت برای هر رتبه، به ترتیب پلکانی زیرمجموعه‌گیری
/// (سهراب < اسفندیار < رستم) — این ترتیب با وزن عمومی رتبه‌ها (`Rank::weight`)
/// فرق دارد، چون آنجا اسفندیار/سهراب برای کد هدیه هم‌وزن تعریف شده‌اند.
pub const TIERS: &[(u32, Rank)] = &[(10, Rank::Sohrab), (20, Rank::Esfandyar), (50, Rank::Rostam)];

/// جایگاه یک رتبه در پلکان زیرمجموعه‌گیری. رتبه‌های خارج از پلکان (دلاور/سپهبد) پایین‌ترین‌اند.
fn tier_position(rank: Rank) -> i32 {
    TIERS.iter().position(|(_, r)| *r == rank).map(|i| i as i32).unwrap_or(-1)
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
    let r = client.execute(
        "INSERT INTO referral_pending (referred_id, referrer_id, started_at)
         VALUES ($1, $2, $3)
         ON CONFLICT (referred_id) DO NOTHING",
        &[&referred_id, &referrer_id, &now],
    ).await;
    if let Err(e) = r {
        eprintln!("[referral event=pending_record_failed] referred_id={referred_id} referrer_id={referrer_id} err={e}");
    }
}

/// دعوت‌های در انتظاری که `PENDING_DAYS` روز از شروعشان گذشته را بررسی می‌کند:
/// عضو قفل اجباری مانده → تأیید و به `referrals` منتقل می‌شود (امتیاز می‌گیرد)؛
/// عضو نمانده → حذف می‌شود (باطل). توسط یه job دوره‌ای در startup.rs صدا زده می‌شود.
pub async fn sweep_confirm(client: &Client, api: &Bot) {
    let cutoff = now_epoch() - PENDING_DAYS * 86_400;
    let rows = match client.query(
        "SELECT referred_id, referrer_id FROM referral_pending WHERE started_at <= $1",
        &[&cutoff],
    ).await {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[referral event=sweep_query_failed] err={e}");
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
            let r = client.execute(
                "INSERT INTO referrals (referred_id, referrer_id, created_at)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (referred_id) DO NOTHING",
                &[&referred_id, &referrer_id, &now],
            ).await;
            if let Err(e) = r {
                eprintln!("[referral event=confirm_insert_failed] referred_id={referred_id} err={e}");
                continue;
            }
            confirmed += 1;
        } else {
            discarded += 1;
        }

        let _ = client.execute(
            "DELETE FROM referral_pending WHERE referred_id = $1",
            &[&referred_id],
        ).await;
    }

    if confirmed > 0 || discarded > 0 {
        eprintln!("[referral event=sweep_done] confirmed={confirmed} discarded={discarded}");
    }
}

pub async fn count_referrals(client: &Client, referrer_id: i64) -> i64 {
    client.query_one(
        "SELECT COUNT(*) FROM referrals WHERE referrer_id = $1",
        &[&referrer_id],
    ).await.map(|row| row.get(0)).unwrap_or(0)
}

/// تعداد دعوت‌های این referrer که هنوز در انتظار تأیید (۲ روز عضویت) هستند.
pub async fn count_pending(client: &Client, referrer_id: i64) -> i64 {
    client.query_one(
        "SELECT COUNT(*) FROM referral_pending WHERE referrer_id = $1",
        &[&referrer_id],
    ).await.map(|row| row.get(0)).unwrap_or(0)
}

/// مجموع امتیازهایی که تا الان صرف فعال‌سازی رتبه شده.
pub async fn total_spent_points(client: &Client, user_id: i64) -> i64 {
    client.query_one(
        "SELECT COALESCE(SUM(points_spent), 0) FROM referral_activations WHERE user_id = $1",
        &[&user_id],
    ).await.map(|row| row.get(0)).unwrap_or(0)
}

pub async fn record_activation(client: &Client, user_id: i64, rank: Rank, points_spent: i64, expires_at: i64) {
    let now = now_epoch();
    let r = client.execute(
        "INSERT INTO referral_activations (user_id, rank, points_spent, activated_at, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
        &[&user_id, &rank.as_str(), &(points_spent as i32), &now, &expires_at],
    ).await;
    if let Err(e) = r {
        eprintln!("[referral event=record_activation_failed] user_id={user_id} rank={} err={e}", rank.as_str());
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

pub async fn plan_activation(client: &Client, user_id: i64, tier_rank: Rank) -> ActivationPlan {
    let now = now_epoch();
    let cur = crate::rank::store::get_user_rank(client, user_id).await.ok().flatten();

    let active = cur.as_ref().is_some_and(|r| match r.expires_at {
        Some(exp) => exp > now,
        None => true,
    });

    if !active {
        return ActivationPlan::Apply { rank: tier_rank, expires_at: now + ACTIVATION_DAYS * 86_400 };
    }

    let cur = cur.unwrap();

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
    let converted = if wn == wc { remaining_days } else { ceil_div(remaining_days.saturating_mul(wc), wn) };
    let total_days = ACTIVATION_DAYS + converted;
    ActivationPlan::Apply { rank: tier_rank, expires_at: now + total_days * 86_400 }
}
