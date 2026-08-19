use crate::youtube::trace::log_trace;

pub async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let cores = crate::moebius::cpu::acquire_cpu(user_id, trace_id).await;
    log_trace(trace_id, "cpu_acquired", &format!("{cores:?}"));
    cores
}

pub async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    if cores.is_empty() {
        return;
    }
    crate::moebius::cpu::release_cpu(cores.clone(), trace_id).await;
    log_trace(
        trace_id,
        "cpu_released",
        &format!("cores={cores:?} ok=true"),
    );
}
