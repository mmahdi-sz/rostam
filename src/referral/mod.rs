use tokio_postgres::{Client, GenericClient};

use crate::rank::types::{Rank, ceil_div};

/// Referral tier thresholds (`Sohrab` < `Esfandyar` < `Rostam`).
pub const TIERS: &[(u32, Rank)] = &[
    (10, Rank::Sohrab),
    (20, Rank::Esfandyar),
    (50, Rank::Rostam),
];

/// Tier hierarchy index (`Dalavar`/`Sepahbod` return `-1`).
fn tier_position(rank: Rank) -> i32 {
    TIERS
        .iter()
        .position(|(_, r)| *r == rank)
        .map(|i| i as i32)
        .unwrap_or(-1)
}

/// Duration per point activation in days.
pub const ACTIVATION_DAYS: i64 = 31;

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Stash a referral link payload. Caller gates it to first-ever sighting of
/// `referred_id`; the PK makes it once-only anyway. Becomes a point in
/// `confirm_on_join` as soon as the user is in the force-join channel.
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

/// Confirm a pending referral the moment the user joins the force-join channel.
/// Point is permanent: leaving later keeps it, re-joining gives no second point
/// (PK on `referred_id`). Returns true if a new point was just recorded.
pub async fn confirm_on_join(client: &Client, referred_id: i64) -> bool {
    let now = now_epoch();
    let r = client
        .execute(
            "WITH moved AS (
                 DELETE FROM referral_pending WHERE referred_id = $1
                 RETURNING referrer_id
             )
             INSERT INTO referrals (referred_id, referrer_id, created_at)
             SELECT $1, referrer_id, $2 FROM moved
             ON CONFLICT (referred_id) DO NOTHING",
            &[&referred_id, &now],
        )
        .await;
    match r {
        Ok(n) => n > 0,
        Err(e) => {
            eprintln!("[referral event=confirm_on_join_failed] referred_id={referred_id} err={e}");
            crate::stats::record_error_global("referral", &e.to_string()).await;
            false
        }
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

/// Count pending referrals for referrer.
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

/// Total spent referral points for user.
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

/// Records activation and deducts referral points in a single statement.
pub async fn record_activation(
    client: &(impl GenericClient + ?Sized),
    user_id: i64,
    rank: Rank,
    points_spent: i64,
    expires_at: i64,
) -> Result<bool, tokio_postgres::Error> {
    let now = now_epoch();
    let rows = client
        .query(
            "INSERT INTO referral_activations (user_id, rank, points_spent, activated_at, expires_at)
             SELECT $1, $2, $3::bigint, $4, $5
              WHERE (SELECT count(*) FROM referrals WHERE referrer_id = $1)
                    - (SELECT COALESCE(SUM(points_spent), 0) FROM referral_activations WHERE user_id = $1)
                    >= $3::bigint
             ON CONFLICT (user_id, rank, expires_at) DO NOTHING
             RETURNING 1",
            &[
                &user_id,
                &rank.as_str(),
                &points_spent,
                &now,
                &expires_at,
            ],
        )
        .await;
    match rows {
        Ok(rows) => Ok(!rows.is_empty()),
        Err(e) => {
            eprintln!(
                "[referral event=record_activation_failed] user_id={user_id} rank={} err={e}",
                rank.as_str()
            );
            crate::stats::record_error_global("referral", &e.to_string()).await;
            Err(e)
        }
    }
}

/// Referral activation calculation plan.
pub enum ActivationPlan {
    /// Requested rank is lower in referral tier order than current active rank.
    Reject,
    /// User currently has permanent rank.
    AlreadyUnlimited,
    /// Apply rank activation with converted remaining days.
    Apply { rank: Rank, expires_at: i64 },
}

/// Available referral points (`total - spent`).
#[allow(dead_code)]
pub fn available_points(total_referrals: i64, total_spent: i64) -> i64 {
    total_referrals.saturating_sub(total_spent).max(0)
}

/// Checks if available points meet tier threshold.
#[allow(dead_code)]
pub fn can_claim_tier(total_referrals: i64, total_spent: i64, tier_threshold: u32) -> bool {
    available_points(total_referrals, total_spent) >= tier_threshold as i64
}

/// Calculates converted days from current rank to target rank based on weights.
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

    /// E2E test on dev DB: 10 points balance with 6 concurrent requests costing 10 each
    /// must deduct exactly once, and balance must never go negative.
    ///
    /// Tests two guards: phase 1 with distinct `expires_at` (WHERE clause guard)
    /// and phase 2 with identical `expires_at` (unique index guard).
    ///
    /// `cargo test record_activation_e2e -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn record_activation_e2e_never_overdraws() {
        use std::sync::Arc;

        const UID: i64 = -999_001;

        let Some(db_url) = crate::config::database_url() else {
            panic!("DATABASE_URL not resolvable from .env — run from the crate root");
        };
        let (client, conn) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
            .await
            .expect("dev DB must be reachable");
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let client = Arc::new(client);

        let reset = || {
            let client = client.clone();
            async move {
                client
                    .execute(
                        "DELETE FROM referral_activations WHERE user_id = $1",
                        &[&UID],
                    )
                    .await
                    .expect("cleanup activations");
                client
                    .execute("DELETE FROM referrals WHERE referrer_id = $1", &[&UID])
                    .await
                    .expect("cleanup referrals");
                client
                    .execute(
                        "INSERT INTO referrals (referred_id, referrer_id, created_at)
                         SELECT $1::bigint - g, $1::bigint, $2::bigint
                           FROM generate_series(1, 10) g",
                        &[&UID, &now_epoch()],
                    )
                    .await
                    .expect("seed 10 referrals");
            }
        };
        let balance = || {
            let client = client.clone();
            async move {
                let row = client
                    .query_one(
                        "SELECT (SELECT count(*) FROM referrals WHERE referrer_id = $1)
                              - (SELECT COALESCE(SUM(points_spent), 0)
                                   FROM referral_activations WHERE user_id = $1)",
                        &[&UID],
                    )
                    .await
                    .expect("read balance");
                row.get::<_, i64>(0)
            }
        };

        for (phase, same_expiry) in [("distinct_expiry", false), ("same_expiry", true)] {
            reset().await;
            assert_eq!(balance().await, 10, "{phase}: seed balance");

            let mut set = tokio::task::JoinSet::new();
            for i in 0..6i64 {
                let client = client.clone();
                let expires_at = if same_expiry {
                    2_000_000_000
                } else {
                    2_000_000_000 + i
                };
                set.spawn(async move {
                    record_activation(&*client, UID, Rank::Sohrab, 10, expires_at).await
                });
            }
            let mut debited = 0;
            while let Some(res) = set.join_next().await {
                if res.expect("task panicked").expect("query failed") {
                    debited += 1;
                }
            }
            assert_eq!(debited, 1, "{phase}: points were debited more than once");
            assert_eq!(balance().await, 0, "{phase}: balance drifted");
        }

        client
            .execute(
                "DELETE FROM referral_activations WHERE user_id = $1",
                &[&UID],
            )
            .await
            .expect("final cleanup activations");
        client
            .execute("DELETE FROM referrals WHERE referrer_id = $1", &[&UID])
            .await
            .expect("final cleanup referrals");
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
