use crate::database::postgresql::PostgresDatabase;

use super::types::CookiePoolSnapshot;

pub async fn save_snapshot(database: &Option<PostgresDatabase>, snapshot: &CookiePoolSnapshot) {
    let Some(db) = database else { return };
    if let Err(error) = db.save_snapshot(snapshot).await {
        eprintln!("failed to save cookie pool snapshot: {error}");
    }
}
