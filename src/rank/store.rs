use tokio_postgres::{Client, GenericClient};

use super::types::Rank;

/// User rank details from DB.
pub struct UserRankRow {
    pub rank: Rank,
    /// Unix timestamp (`None` means permanent).
    pub expires_at: Option<i64>,
}

pub async fn get_user_rank(
    client: &Client,
    user_id: i64,
) -> Result<Option<UserRankRow>, tokio_postgres::Error> {
    let row = client
        .query_opt(
            "SELECT rank, expires_at FROM user_ranks WHERE user_id = $1",
            &[&user_id],
        )
        .await?;

    let Some(row) = row else { return Ok(None) };

    let rank_str: String = row.get(0);
    let expires_at: Option<i64> = row.get(1);

    let Some(rank) = Rank::from_str(&rank_str) else {
        return Ok(None);
    };

    Ok(Some(UserRankRow { rank, expires_at }))
}

pub async fn set_user_rank(
    client: &(impl GenericClient + ?Sized),
    user_id: i64,
    rank: Rank,
    expires_at: Option<i64>,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "INSERT INTO user_ranks (user_id, rank, expires_at, activated_at)
             VALUES ($1, $2, $3, EXTRACT(EPOCH FROM NOW())::BIGINT)
             ON CONFLICT (user_id) DO UPDATE SET
                rank = EXCLUDED.rank,
                expires_at = EXCLUDED.expires_at,
                activated_at = EXCLUDED.activated_at",
            &[&user_id, &rank.as_str(), &expires_at],
        )
        .await?;
    Ok(())
}
