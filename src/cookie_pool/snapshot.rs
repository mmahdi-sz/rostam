use crate::database::postgresql::PostgresDatabase;

use super::pool::CookiePool;

pub async fn save_snapshot(database: &Option<PostgresDatabase>, cookie_pool: &mut CookiePool) {
    let Some(db) = database else { return };
    if let Err(error) = db.save_snapshot(&cookie_pool.snapshot()).await {
        eprintln!("failed to save cookie pool snapshot: {error}");
    }
}
