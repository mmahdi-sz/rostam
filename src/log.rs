use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_trace_id() -> u64 {
    NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Truncate a string to its first 6 chars followed by `…` (for IDs and tokens in logs).
pub fn trunc(s: &str) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(6).collect();
    if chars.next().is_some() { format!("{head}…") } else { head }
}

pub fn trunc_id(id: i64) -> String {
    trunc(&id.to_string())
}

#[cfg(feature = "testapi")]
tokio::task_local! {
    pub static CAPTURED_TRACES: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
}

/// Low-level: emit a trace event with a pre-formatted details string.
/// Lets existing log_trace(trace_id, event, details) callers migrate with
/// a one-line shim: `fn log_trace(t,e,d){crate::log::emit("domain",t,e,d)}`
pub fn emit(domain: &str, trace_id: u64, event: &str, details: &str) {
    let line = if details.is_empty() {
        format!("[{domain} trace={trace_id} event={event}]")
    } else {
        format!("[{domain} trace={trace_id} event={event}] {details}")
    };
    eprintln!("{line}");

    #[cfg(feature = "testapi")]
    let _ = CAPTURED_TRACES.try_with(|arc| arc.lock().unwrap().push(serde_json::json!({
        "domain": domain,
        "trace_id": trace_id,
        "event": event,
        "details": details,
        "line": line,
    })));
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
        let line = format!("[{} trace={} actor] {}", $domain, $trace, parts.join(" "));
        eprintln!("{}", line);
        
        #[cfg(feature = "testapi")]
        let _ = $crate::log::CAPTURED_TRACES.try_with(|arc| {
            arc.lock().unwrap().push(serde_json::json!({
                "domain": $domain,
                "trace_id": $trace,
                "event": "actor",
                "details": parts.join(" "),
                "line": line,
            }))
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
        let line = format!("[{} trace={} actor] {}", $domain, $trace, parts.join(" "));
        eprintln!("{}", line);

        #[cfg(feature = "testapi")]
        let _ = $crate::log::CAPTURED_TRACES.try_with(|arc| {
            arc.lock().unwrap().push(serde_json::json!({
                "domain": $domain,
                "trace_id": $trace,
                "event": "actor",
                "details": parts.join(" "),
                "line": line,
            }))
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
        let line = if parts.is_empty() {
            format!("[{} trace={} event={}]", $domain, $trace, $event)
        } else {
            format!("[{} trace={} event={}] {}", $domain, $trace, $event, parts.join(" "))
        };
        eprintln!("{}", line);

        #[cfg(feature = "testapi")]
        let _ = $crate::log::CAPTURED_TRACES.try_with(|arc| {
            arc.lock().unwrap().push(serde_json::json!({
                "domain": $domain,
                "trace_id": $trace,
                "event": $event,
                "details": parts.join(" "),
                "line": line,
            }))
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
        struct FakeUser { pub id: u64, pub username: Option<String> }
        let user = FakeUser { id: 671234567, username: Some("parsa".to_string()) };
        let trace = 99u64;
        log_actor!("test", trace, &user, "rank" => "Dalavar", "clicked" => "upscale:model:x");
    }
}
