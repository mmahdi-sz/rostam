use serde_json::Value;
use std::sync::Mutex;

pub static CAPTURED_PAYLOADS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

pub fn clear_payloads() {
    CAPTURED_PAYLOADS.lock().unwrap().clear();
}
