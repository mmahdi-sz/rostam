use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_trace_id() -> u64 {
    NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Truncate a string to its first 6 chars followed by `…` (for IDs and tokens in logs).
pub fn trunc(s: &str) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(6).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

pub fn trunc_id(id: i64) -> String {
    trunc(&id.to_string())
}

/// Redact Telegram bot token (`/bot<token>`) from outgoing error/log strings.
pub fn redact(s: &str) -> std::borrow::Cow<'_, str> {
    const NEEDLE: &str = "/bot";
    const MASK: &str = "/bot<redacted>";

    let is_token_start = |rest: &str| rest.as_bytes().first().is_some_and(u8::is_ascii_digit);

    // Hot path: zero allocation when no token is present.
    let Some(first) = s
        .match_indices(NEEDLE)
        .find(|(i, _)| is_token_start(&s[i + NEEDLE.len()..]))
        .map(|(i, _)| i)
    else {
        return std::borrow::Cow::Borrowed(s);
    };

    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;
    let mut at = Some(first);
    while let Some(i) = at {
        out.push_str(&s[cursor..i]);
        out.push_str(MASK);
        let after = i + NEEDLE.len();
        // Token continues until the next '/' or end of string.
        cursor = s[after..]
            .find('/')
            .map(|off| after + off)
            .unwrap_or(s.len());
        at = s[cursor..]
            .match_indices(NEEDLE)
            .find(|(off, _)| is_token_start(&s[cursor + off + NEEDLE.len()..]))
            .map(|(off, _)| cursor + off);
    }
    out.push_str(&s[cursor..]);
    std::borrow::Cow::Owned(out)
}

pub fn init_subscriber() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_level(false)
        .try_init();
}

#[cfg(feature = "testapi")]
tokio::task_local! {
    pub static CAPTURED_TRACES: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
}

/// Low-level: emit a trace event with a pre-formatted details string.
/// Lets existing log_trace(trace_id, event, details) callers migrate with
/// a one-line shim: `fn log_trace(t,e,d){crate::log::emit("domain",t,e,d)}`
pub fn emit(domain: &str, trace_id: u64, event: &str, details: &str) {
    // Redact token once for both the log line and testapi capture.
    let details = redact(details);
    let line = if details.is_empty() {
        format!("[{domain} trace={trace_id} event={event}]")
    } else {
        format!("[{domain} trace={trace_id} event={event}] {details}")
    };
    tracing::info!(domain = domain, trace = trace_id, event = event, "{}", line);

    #[cfg(feature = "testapi")]
    let _ = CAPTURED_TRACES.try_with(|arc| {
        if let Ok(mut lock) = arc.lock() {
            lock.push(serde_json::json!({
                "domain": domain,
                "trace_id": trace_id,
                "event": event,
                "details": &*details,
                "line": line,
            }));
        }
    });
}

/// Log the actor line when only user_id is available (no full User struct):
/// `[<domain> trace=N actor] id=<id6>… <key=val ...>`
///
/// Usage: `log_actor_id!("upscale", trace, user_id, "clicked" => cb_data);`
#[macro_export]
macro_rules! log_actor_id {
    ($domain:expr, $trace:expr, $user_id:expr $(, $key:expr => $val:expr)*) => {{
        let id_str = $crate::log::trunc_id($user_id as i64);
        let mut parts: Vec<String> = vec![format!("id={}", id_str)];
        $( parts.push({
            let k: &str = $key;
            let v = format!("{}", $val);
            if k == "=>" { format!("=> {}", v) } else { format!("{}={}", k, v) }
        }); )*
        let joined = parts.join(" ");
        // Redact token once for log line and test capture.
        let details = $crate::log::redact(&joined);
        let line = format!("[{} trace={} actor] {}", $domain, $trace, details);
        tracing::info!(domain = $domain, trace = $trace, event = "actor", "{}", line);

        #[cfg(feature = "testapi")]
        let _ = $crate::log::CAPTURED_TRACES.try_with(|arc| {
            if let Ok(mut lock) = arc.lock() {
                lock.push(serde_json::json!({
                    "domain": $domain,
                    "trace_id": $trace,
                    "event": "actor",
                    "details": &*details,
                    "line": line,
                }));
            }
        });
    }};
}

/// Log the actor line (once per user action, at handler entry):
/// `[<domain> trace=N actor] user=@<username> id=<id6>… <key=val ...>`
///
/// Usage: `log_actor!("upscale", trace, user, "rank" => rank, "clicked" => cb_data);`
/// where `user` is a `&frankenstein::types::User`.
#[macro_export]
macro_rules! log_actor {
    ($domain:expr, $trace:expr, $user:expr $(, $key:expr => $val:expr)*) => {{
        let username = $user.username.as_deref().unwrap_or("?");
        let id_str = $crate::log::trunc_id($user.id as i64);
        let mut parts: Vec<String> = vec![
            format!("user=@{}", username),
            format!("id={}", id_str),
        ];
        $( parts.push({
            let k: &str = $key;
            let v = format!("{}", $val);
            if k == "=>" { format!("=> {}", v) } else { format!("{}={}", k, v) }
        }); )*
        let joined = parts.join(" ");
        let details = $crate::log::redact(&joined);
        let line = format!("[{} trace={} actor] {}", $domain, $trace, details);
        tracing::info!(domain = $domain, trace = $trace, event = "actor", "{}", line);

        #[cfg(feature = "testapi")]
        let _ = $crate::log::CAPTURED_TRACES.try_with(|arc| {
            if let Ok(mut lock) = arc.lock() {
                lock.push(serde_json::json!({
                    "domain": $domain,
                    "trace_id": $trace,
                    "event": "actor",
                    "details": &*details,
                    "line": line,
                }));
            }
        });
    }};
}

/// Log an event line (one per step, logged BEFORE the step runs):
/// `[<domain> trace=N event=<step>] key=val ... => outcome`
///
/// Usage: `log_ev!("upscale", trace, "quota_check", "used" => used, "limit" => limit, "=>" => "pass");`
#[macro_export]
macro_rules! log_ev {
    ($domain:expr, $trace:expr, $event:expr $(, $key:expr => $val:expr)*) => {{
        let parts: Vec<String> = vec![$({
            let k: &str = $key;
            let v = format!("{}", $val);
            if k == "=>" { format!("=> {}", v) } else { format!("{}={}", k, v) }
        }),*];
        let joined = parts.join(" ");
        let details = $crate::log::redact(&joined);
        let line = if details.is_empty() {
            format!("[{} trace={} event={}]", $domain, $trace, $event)
        } else {
            format!("[{} trace={} event={}] {}", $domain, $trace, $event, details)
        };
        tracing::info!(domain = $domain, trace = $trace, event = $event, "{}", line);

        #[cfg(feature = "testapi")]
        let _ = $crate::log::CAPTURED_TRACES.try_with(|arc| {
            if let Ok(mut lock) = arc.lock() {
                lock.push(serde_json::json!({
                    "domain": $domain,
                    "trace_id": $trace,
                    "event": $event,
                    "details": &*details,
                    "line": line,
                }));
            }
        });
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trunc_short() {
        assert_eq!(trunc("short"), "short");
        assert_eq!(trunc("123456"), "123456");
    }

    #[test]
    fn trunc_long() {
        assert_eq!(trunc("1234567890"), "123456…");
    }

    #[test]
    fn trunc_id_digits() {
        assert_eq!(trunc_id(671234567), "671234…");
        assert_eq!(trunc_id(123), "123");
    }

    #[test]
    fn next_trace_id_increments() {
        let a = next_trace_id();
        let b = next_trace_id();
        assert!(b > a);
    }

    // Verify event macro builds correctly (output goes to stderr; no panic = pass).
    #[test]
    fn log_ev_no_panic() {
        let trace = 99u64;
        log_ev!("test", trace, "step");
        log_ev!("test", trace, "quota_check", "used" => 2, "limit" => 3, "=>" => "pass");
        log_ev!("test", trace, "cpu_acquire", "=>" => "cores=[]");
    }

    #[test]
    fn log_actor_no_panic() {
        // Minimal stub matching frankenstein::types::User shape used by the macro.
        struct FakeUser {
            pub id: u64,
            pub username: Option<String>,
        }
        let user = FakeUser {
            id: 671234567,
            username: Some("parsa".to_string()),
        };
        let trace = 99u64;
        log_actor!("test", trace, &user, "rank" => "Dalavar", "clicked" => "upscale:model:x");
    }

    // Synthetic token — real token should never appear in tests.
    const FAKE_TOKEN: &str = "123456789:AAHfake_token_value_for_tests_only";

    #[test]
    fn redact_reqwest_display_shape() {
        let s = format!(
            "HTTP error: error sending request for url \
             (https://api.telegram.org/file/bot{FAKE_TOKEN}/voice/file_1.oga)"
        );
        let out = redact(&s);
        assert!(!out.contains(FAKE_TOKEN), "token survived: {out}");
        assert!(out.contains("/bot<redacted>"));
        // Path after token must remain intact for log debuggability.
        assert!(out.ends_with("/voice/file_1.oga)"));
    }

    #[test]
    fn redact_debug_shape() {
        // `{e:?}` shape logged by deoldify/feynobg.
        let s = format!(
            "reqwest::Error {{ kind: Request, url: \"https://api.telegram.org/bot{FAKE_TOKEN}/getFile\" }}"
        );
        let out = redact(&s);
        assert!(!out.contains(FAKE_TOKEN), "token survived: {out}");
        assert!(out.contains("/bot<redacted>/getFile"));
    }

    #[test]
    fn redact_multiple_occurrences() {
        let s = format!(
            "first https://api.telegram.org/bot{FAKE_TOKEN}/getFile \
             then https://api.telegram.org/file/bot{FAKE_TOKEN}/a/b.oga end"
        );
        let out = redact(&s);
        assert!(!out.contains(FAKE_TOKEN), "token survived: {out}");
        assert_eq!(out.matches("/bot<redacted>").count(), 2);
        assert!(out.ends_with("/a/b.oga end"));
    }

    #[test]
    fn redact_without_token_does_not_allocate() {
        // Performance contract: hot path (vast majority of log lines) must not allocate.
        let s = "[yt trace=7 event=quota_check] used=2 limit=3 => pass";
        assert!(matches!(redact(s), std::borrow::Cow::Borrowed(_)));
        assert_eq!(redact(s), s);
    }

    #[test]
    fn redact_leaves_non_token_bot_path_alone() {
        let s = "GET https://example.com/bot/docs failed";
        assert!(matches!(redact(s), std::borrow::Cow::Borrowed(_)));
        assert_eq!(redact(s), s);
    }

    #[test]
    fn redact_leaves_local_bot_api_path_alone() {
        // Local Bot API path logged on downloads.
        let s = "[bot trace=3 event=local_copy] path=/var/lib/telegram-bot-api/voice/file_1.oga";
        assert!(matches!(redact(s), std::borrow::Cow::Borrowed(_)));
        assert_eq!(redact(s), s);
    }

    /// E2E test to ensure reqwest errors stored in DB are redacted.
    #[tokio::test]
    #[ignore]
    async fn redact_e2e_reqwest_error_reaches_db_redacted() {
        // Synthetic token for test only.
        const E2E_TOKEN: &str = "987654321:AAHe2e_synthetic_token_do_not_use";
        const FEATURE: &str = "e2e_redact_probe";

        // Port 1 is closed to simulate connection failure locally.
        let url = format!("http://127.0.0.1:1/file/bot{E2E_TOKEN}/probe.oga");
        let err = crate::http::client()
            .get(&url)
            .send()
            .await
            .expect_err("port 1 must refuse the connection");
        let raw = format!("{err}");
        // If this assertion fails, reqwest no longer includes URL in Display
        // and filter assumptions must be re-evaluated.
        assert!(raw.contains(E2E_TOKEN), "reqwest no longer leaks the url");
        assert!(!redact(&raw).contains(E2E_TOKEN));

        let Some(db_url) = crate::config::database_url() else {
            panic!("DATABASE_URL not resolvable from .env — run from the crate root");
        };
        let db = crate::database::postgresql::PostgresDatabase::connect(&db_url)
            .await
            .expect("dev DB must be reachable");
        crate::stats::init(db.pool().clone());

        // Real path: function invoked by handlers on unexpected error.
        crate::stats::record_error_global(FEATURE, &format!("download failed: {raw}")).await;

        // Allow batch flusher to write to database
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;

        let client = db.get().await.expect("checkout client from pool");
        let row = client
            .query_one(
                "SELECT message FROM stats_errors WHERE feature = $1 ORDER BY id DESC LIMIT 1",
                &[&FEATURE],
            )
            .await
            .expect("the probe row must exist");
        let stored: String = row.get(0);
        client
            .execute("DELETE FROM stats_errors WHERE feature = $1", &[&FEATURE])
            .await
            .expect("probe cleanup");

        assert!(!stored.contains(E2E_TOKEN), "token stored in DB");
        assert!(stored.contains("/bot<redacted>"), "stored: {stored}");
    }
}
