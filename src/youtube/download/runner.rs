use frankenstein::client_reqwest::Bot;

use super::cancel::register_cancel;
use super::playlist::run_playlist_download;
use super::single::run_download;
use super::store::get_request;
use super::types::Selection;

pub fn spawn_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
) {
    let cancel = register_cancel(request_id);
    crate::app::spawn_user_task(async move {
        // Check whether playlist or single video.
        if let Some(req_peek) = get_request(request_id) {
            if req_peek.is_playlist && !req_peek.playlist_items.is_empty() {
                run_playlist_download(
                    api,
                    request_id,
                    selection,
                    status_chat_id,
                    status_message_id,
                    cancel,
                )
                .await;
                return;
            }
        }
        run_download(
            api,
            request_id,
            selection,
            status_chat_id,
            status_message_id,
            cancel,
        )
        .await
    });
}
