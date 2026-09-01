use deadpool_postgres::Pool;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub enum TelemetryMsg {
    Event {
        user_id: i64,
        feature: String,
        action: String,
        status: String,
        amount: i64,
    },
    Error {
        feature: String,
        message: String,
    },
}

#[derive(Clone)]
pub struct TelemetryFlusher {
    tx: UnboundedSender<TelemetryMsg>,
}

impl TelemetryFlusher {
    pub fn new(pool: Pool) -> Self {
        let (tx, mut rx) = unbounded_channel();

        tokio::spawn(async move {
            let mut events = Vec::with_capacity(128);
            let mut errors = Vec::with_capacity(64);
            let mut interval = tokio::time::interval(Duration::from_millis(250));

            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(TelemetryMsg::Event { user_id, feature, action, status, amount }) => {
                                events.push((user_id, feature, action, status, amount));
                                if events.len() >= 100 {
                                    flush_events(&pool, &mut events).await;
                                }
                            }
                            Some(TelemetryMsg::Error { feature, message }) => {
                                errors.push((feature, message));
                                if errors.len() >= 50 {
                                    flush_errors(&pool, &mut errors).await;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = interval.tick() => {
                        if !events.is_empty() {
                            flush_events(&pool, &mut events).await;
                        }
                        if !errors.is_empty() {
                            flush_errors(&pool, &mut errors).await;
                        }
                    }
                }
            }
        });

        Self { tx }
    }

    pub fn send(&self, msg: TelemetryMsg) {
        let _ = self.tx.send(msg);
    }
}

async fn flush_events(pool: &Pool, buffer: &mut Vec<(i64, String, String, String, i64)>) {
    if buffer.is_empty() {
        return;
    }
    let batch: Vec<_> = std::mem::take(buffer);

    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[stats event=flusher_get_client_failed err={e}]");
            return;
        }
    };

    if batch.len() == 1 {
        let (user_id, feature, action, status, amount) = &batch[0];
        let _ = client
            .execute(
                "INSERT INTO stats_events (user_id, feature, action, status, amount) VALUES ($1, $2, $3, $4, $5)",
                &[user_id, feature, action, status, amount],
            )
            .await;
        return;
    }

    // Multi-row parameterized batch insert
    let mut sql =
        String::from("INSERT INTO stats_events (user_id, feature, action, status, amount) VALUES ");
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        Vec::with_capacity(batch.len() * 5);

    for (i, item) in batch.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let base = i * 5;
        sql.push_str(&format!(
            "(${}, ${}, ${}, ${}, ${})",
            base + 1,
            base + 2,
            base + 3,
            base + 4,
            base + 5
        ));
        params.push(&item.0);
        params.push(&item.1);
        params.push(&item.2);
        params.push(&item.3);
        params.push(&item.4);
    }

    if let Err(e) = client.execute(&sql, &params[..]).await {
        eprintln!("[stats event=batch_flush_events_failed err={e}]");
    }
}

async fn flush_errors(pool: &Pool, buffer: &mut Vec<(String, String)>) {
    if buffer.is_empty() {
        return;
    }
    let batch: Vec<_> = std::mem::take(buffer);

    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[stats event=flusher_get_client_failed err={e}]");
            return;
        }
    };

    if batch.len() == 1 {
        let (feature, msg) = &batch[0];
        let _ = client
            .execute(
                "INSERT INTO stats_errors (feature, message) VALUES ($1, $2)",
                &[feature, msg],
            )
            .await;
        return;
    }

    let mut sql = String::from("INSERT INTO stats_errors (feature, message) VALUES ");
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        Vec::with_capacity(batch.len() * 2);

    for (i, item) in batch.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        let base = i * 2;
        sql.push_str(&format!("(${}, ${})", base + 1, base + 2));
        params.push(&item.0);
        params.push(&item.1);
    }

    if let Err(e) = client.execute(&sql, &params[..]).await {
        eprintln!("[stats event=batch_flush_errors_failed err={e}]");
    }
}
