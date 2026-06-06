use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::Notify;

static ACTIVE_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<Notify>>>> = OnceLock::new();

fn active_downloads() -> &'static Mutex<HashMap<u64, Arc<Notify>>> {
    ACTIVE_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_cancel(request_id: u64) -> Arc<Notify> {
    let notify = Arc::new(Notify::new());
    active_downloads().lock().unwrap().insert(request_id, notify.clone());
    notify
}

pub fn unregister_cancel(request_id: u64) {
    active_downloads().lock().unwrap().remove(&request_id);
}

pub fn cancel_download(request_id: u64) -> bool {
    if let Some(notify) = active_downloads().lock().unwrap().remove(&request_id) {
        notify.notify_one();
        true
    } else {
        false
    }
}

pub struct UnregisterGuard(pub u64);
impl Drop for UnregisterGuard {
    fn drop(&mut self) {
        unregister_cancel(self.0);
    }
}
