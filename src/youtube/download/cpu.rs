use crate::youtube::trace::log_trace;

pub const SEP_BASE: &str = "http://127.0.0.1:6589";

pub async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = crate::http::client();
    let res = client
        .post(format!("{SEP_BASE}/cpu/acquire"))
        .form(&[
            ("user_id", user_id.to_string()),
            ("is_vip", "false".to_string()),
        ])
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await;
    match res {
        Ok(r) => {
            let json: serde_json::Value = r.json().await.unwrap_or_default();
            let cores: Vec<i32> = json
                .get("cores")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            log_trace(trace_id, "cpu_acquired", &format!("{cores:?}"));
            cores
        }
        Err(e) => {
            log_trace(trace_id, "cpu_acquire_failed", &format!("{e}"));
            vec![]
        }
    }
}

pub async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    if cores.is_empty() {
        return;
    }
    let client = crate::http::client();
    let body = serde_json::json!({ "cores": cores });
    let r = client
        .post(format!("{SEP_BASE}/cpu/release"))
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    log_trace(
        trace_id,
        "cpu_released",
        &format!("cores={cores:?} ok={}", r.is_ok()),
    );
}
