//! Background progress ticker for long-running Telegram bot jobs.
//!
//! Provides `ProgressTicker` to periodically edit status messages with live progress,
//! deduplicating identical edits and integrating with `JobRegistry` cancellation tokens.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use frankenstein::client_reqwest::Bot;
use frankenstein::types::InlineKeyboardMarkup;

use crate::bot::edit_text_md;

/// Configuration builder and runner for a live Telegram message progress ticker.
pub struct ProgressTicker {
    api: Bot,
    chat_id: i64,
    message_id: i32,
    interval: Duration,
    cancel_flag: Option<Arc<AtomicBool>>,
    keyboard: Option<InlineKeyboardMarkup>,
}

/// RAII handle to an active background ticker task.
///
/// Sets the internal `stop` flag on drop to gracefully terminate the ticker loop.
#[derive(Debug)]
pub struct ProgressTickerHandle {
    stop_flag: Arc<AtomicBool>,
}

impl ProgressTickerHandle {
    /// Manually signals the ticker loop to terminate.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

impl Drop for ProgressTickerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl ProgressTicker {
    /// Creates a new progress ticker for a given message.
    pub fn new(api: &Bot, chat_id: i64, message_id: i32) -> Self {
        Self {
            api: api.clone(),
            chat_id,
            message_id,
            interval: Duration::from_millis(2000),
            cancel_flag: None,
            keyboard: None,
        }
    }

    /// Sets the polling/edit interval (defaults to 2000ms to avoid Telegram rate limits).
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Attaches an optional `JobRegistry` cancellation flag.
    ///
    /// The ticker automatically exits early if `cancel_flag.load(Ordering::SeqCst)` becomes true.
    pub fn with_cancel_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel_flag = Some(flag);
        self
    }

    /// Attaches an optional inline keyboard (e.g. `job_cancel_keyboard`) to remain visible on edits.
    pub fn with_keyboard(mut self, keyboard: InlineKeyboardMarkup) -> Self {
        self.keyboard = Some(keyboard);
        self
    }

    /// Spawns the ticker loop via `crate::app::spawn_user_task`.
    ///
    /// The callback receives the elapsed duration and generates the MarkdownV2 body.
    /// Deduplicates edits: skips Telegram API calls if the rendered text is unchanged.
    pub fn spawn<F>(self, render_fn: F) -> ProgressTickerHandle
    where
        F: Fn(Duration) -> Option<String> + Send + Sync + 'static,
    {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_inner = stop_flag.clone();
        let api = self.api;
        let chat_id = self.chat_id;
        let message_id = self.message_id;
        let interval = self.interval;
        let cancel_flag = self.cancel_flag;
        let keyboard = self.keyboard;

        crate::app::spawn_user_task(async move {
            let start_time = Instant::now();
            let mut ticker_interval = tokio::time::interval(interval);
            let mut last_rendered = String::new();

            loop {
                ticker_interval.tick().await;

                if stop_inner.load(Ordering::SeqCst) {
                    break;
                }

                if let Some(cf) = &cancel_flag {
                    if cf.load(Ordering::SeqCst) {
                        break;
                    }
                }

                let elapsed = start_time.elapsed();
                let Some(rendered_text) = render_fn(elapsed) else {
                    continue;
                };

                if rendered_text == last_rendered {
                    continue;
                }

                last_rendered = rendered_text.clone();
                let _ = edit_text_md(&api, chat_id, message_id, &rendered_text, keyboard.clone()).await;
            }
        });

        ProgressTickerHandle { stop_flag }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ticker_handle_drop_sets_stop_flag() {
        let stop_flag = Arc::new(AtomicBool::new(false));
        {
            let handle = ProgressTickerHandle {
                stop_flag: stop_flag.clone(),
            };
            assert!(!stop_flag.load(Ordering::SeqCst));
            drop(handle);
        }
        assert!(stop_flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_ticker_handle_manual_stop() {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let handle = ProgressTickerHandle {
            stop_flag: stop_flag.clone(),
        };
        assert!(!stop_flag.load(Ordering::SeqCst));
        handle.stop();
        assert!(stop_flag.load(Ordering::SeqCst));
    }
}
