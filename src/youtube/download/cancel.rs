use std::sync::{Arc, LazyLock};

use tokio::sync::Notify;

use crate::common::job::{JobGuard, JobRegistry};

static ACTIVE_DOWNLOADS: LazyLock<JobRegistry<u64, Arc<Notify>>> = LazyLock::new(JobRegistry::new);

pub fn register_cancel(request_id: u64) -> Arc<Notify> {
    ACTIVE_DOWNLOADS.register_notify(request_id)
}

pub fn cancel_guard(request_id: u64) -> JobGuard<u64, Arc<Notify>> {
    ACTIVE_DOWNLOADS.guard(request_id)
}

pub fn cancel_download(request_id: u64) -> bool {
    ACTIVE_DOWNLOADS.cancel_notify(&request_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_youtube_cancel_lifecycle() {
        let req_id = 999_888_777u64;
        let notify = register_cancel(req_id);
        assert!(ACTIVE_DOWNLOADS.is_active(&req_id));

        let mut fut = std::pin::pin!(notify.notified());
        let cancelled = cancel_download(req_id);
        assert!(cancelled);
        assert!(!ACTIVE_DOWNLOADS.is_active(&req_id));

        tokio::time::timeout(std::time::Duration::from_millis(50), fut.as_mut())
            .await
            .expect("notify must fire on cancel");

        // Test guard drop unregister
        let req_id_2 = 999_888_778u64;
        let _notify2 = register_cancel(req_id_2);
        assert!(ACTIVE_DOWNLOADS.is_active(&req_id_2));
        {
            let _guard = cancel_guard(req_id_2);
            assert!(ACTIVE_DOWNLOADS.is_active(&req_id_2));
        }
        assert!(!ACTIVE_DOWNLOADS.is_active(&req_id_2));
    }
}
