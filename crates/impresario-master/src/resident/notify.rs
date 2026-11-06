//! Notification delivery for the resident runner (TASK.md section 8.3).
//!
//! Routing and duplicate-failure throttling live in `service`; this module
//! turns a routed notification into a toast/webhook attempt, applies quiet
//! hours and per-channel rate limits, and records one `notify_delivery` event
//! per channel. Delivery is best-effort: a failing toast server or webhook must
//! never stop a watcher or schedule.
//!
//! Webhook bearer secrets are read from the OS credential store under service
//! `impresario.notify`, account `webhook:NAME`. `IMPRESARIO_WEBHOOK_<NAME>_SECRET`
//! is a headless override and is checked first.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local};
use impresario_core::home::Home;
use impresario_core::resident::{Notifications, RateLimit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::jobs::JobStore;

use super::service::{append_event, parse_local, ServiceState};

/// OS credential-store service used for webhook secrets.
pub const KEYRING_SERVICE: &str = "impresario.notify";

/// Persisted moving-window count for one channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateWindow {
    pub started_at: String,
    pub count: u64,
}

/// One routed notification, independent of the transport that delivers it.
#[derive(Debug, Clone)]
pub struct Message {
    /// `watch` or `schedule`.
    pub kind: &'static str,
    pub source: String,
    pub id: String,
    pub event: &'static str,
    pub status: String,
    pub signature: Option<String>,
    pub count: u64,
    pub job: Option<String>,
    pub report: Option<PathBuf>,
    pub dump: Option<PathBuf>,
    pub exit_code: Option<i32>,
    pub at: String,
}

impl Message {
    fn title(&self) -> String {
        format!("Impresario {} '{}': {}", self.kind, self.source, self.event)
    }

    fn body(&self) -> String {
        let mut parts = vec![format!("status {}", self.status)];
        if let Some(signature) = &self.signature {
            parts.push(format!("signature {signature}"));
        }
        if let Some(report) = &self.report {
            parts.push(format!("report {}", report.display()));
        }
        parts.join("; ")
    }

    /// JSON body posted to webhook channels.
    #[must_use]
    pub fn payload(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "source": self.source,
            "event": self.event,
            "status": self.status,
            "signature": self.signature,
            "count": self.count,
            "job": self.job,
            "report": self.report.as_ref().map(|path| path.display().to_string()),
            "dump": self.dump.as_ref().map(|path| path.display().to_string()),
            "exit_code": self.exit_code,
            "at": self.at,
            "title": self.title(),
            "body": self.body(),
        })
    }
}

#[derive(Debug)]
enum DeliveryResult {
    Delivered,
    Suppressed(&'static str),
    Failed(String),
}

/// Deliver one routed notification to every configured channel.
///
/// Quiet hours suppress all channels; rate limits are independent per channel.
/// Suppression still writes a `notify_delivery` event, so `events.jsonl` tells
/// the whole story. The state lock is never held across a network or toast call.
pub fn dispatch(
    home: &Home,
    notifications: &Notifications,
    state: &Arc<Mutex<ServiceState>>,
    channels: &[String],
    message: &Message,
) {
    let now = Local::now();
    let quiet = notifications.is_quiet_at(now.time());
    let unique: Vec<&String> = {
        let mut seen = BTreeSet::new();
        channels
            .iter()
            .filter(|channel| seen.insert(channel.as_str()))
            .collect()
    };
    let mut plans: Vec<(&String, Option<&'static str>)> = Vec::new();
    {
        let Ok(mut state) = state.lock() else {
            return;
        };
        for channel in unique {
            if quiet {
                plans.push((channel, Some("quiet_hours")));
                continue;
            }
            let allowed = notifications
                .rate_limits
                .get(channel)
                .is_none_or(|limit| allow_rate(&mut state.notification_rates, channel, limit, now));
            plans.push((channel, if allowed { None } else { Some("rate_limit") }));
        }
    }

    for (channel, suppression) in plans {
        let result = if let Some(reason) = suppression {
            DeliveryResult::Suppressed(reason)
        } else {
            match deliver_channel(notifications, channel, message) {
                Ok(()) => DeliveryResult::Delivered,
                Err(error) => DeliveryResult::Failed(error),
            }
        };
        append_event(home, delivery_event(message, channel, &result));
    }
}

fn allow_rate(
    entries: &mut BTreeMap<String, RateWindow>,
    channel: &str,
    limit: &RateLimit,
    now: DateTime<Local>,
) -> bool {
    let Ok(window) = limit.window() else {
        // Invalid limits are config errors; never silently silence a channel
        // because a hand-edited value slipped through validation.
        return true;
    };
    if window.is_zero() || limit.max == 0 {
        return true;
    }
    let active = entries.get(channel).is_some_and(|entry| {
        parse_local(&entry.started_at).is_some_and(|started| now - started < window)
    });
    if active {
        if let Some(entry) = entries.get_mut(channel) {
            if entry.count >= limit.max {
                return false;
            }
            entry.count = entry.count.saturating_add(1);
            return true;
        }
    }
    entries.insert(
        channel.to_string(),
        RateWindow {
            started_at: now.to_rfc3339(),
            count: 1,
        },
    );
    true
}

fn deliver_channel(
    notifications: &Notifications,
    channel: &str,
    message: &Message,
) -> Result<(), String> {
    if channel == "toast" {
        return show_toast(message);
    }
    let Some(name) = channel.strip_prefix("webhook:") else {
        return Err(format!("unknown notification channel '{channel}'"));
    };
    let Some(endpoint) = notifications.webhooks.get(name) else {
        return Err(format!(
            "webhook '{name}' has no notifications.webhooks.{name}.url"
        ));
    };
    let secret = webhook_secret(name);
    deliver_webhook(&endpoint.url, name, &message.payload(), secret.as_deref())
}

fn delivery_event(message: &Message, channel: &str, result: &DeliveryResult) -> Value {
    let (ok, suppressed, error) = match result {
        DeliveryResult::Delivered => (true, None, None),
        DeliveryResult::Suppressed(reason) => (false, Some(*reason), None),
        DeliveryResult::Failed(error) => (false, None, Some(error.as_str())),
    };
    json!({
        "kind": "notify_delivery",
        "id": message.id,
        "source_kind": message.kind,
        "source": message.source,
        "event": message.event,
        "channel": channel,
        "ok": ok,
        "suppressed": suppressed,
        "error": error,
        "at": Local::now().to_rfc3339(),
    })
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn show_toast(message: &Message) -> Result<(), String> {
    use notify_rust::Notification;

    let mut notification = Notification::new();
    notification
        .summary(&message.title())
        .body(&message.body())
        .appname("Impresario");
    if message.report.is_some() {
        notification.action("default", "Open report");
    }
    let handle = notification
        .show()
        .map_err(|error| format!("OS toast failed: {error}"))?;
    // The resident runner stays alive, so a small detached thread can wait for
    // the user to click the notification without blocking the loop. The legacy
    // macOS backend needs a main run loop for callbacks and would otherwise
    // leak a blocked thread; there the handle drop sends the toast
    // asynchronously and the report path stays in the body.
    #[cfg(any(windows, target_os = "linux"))]
    if let Some(report) = message.report.clone() {
        std::thread::spawn(move || {
            handle.wait_for_action(move |action| {
                if action == "default" {
                    let _ = crate::open::open_external(&report);
                }
            });
        });
    }
    #[cfg(target_os = "macos")]
    drop(handle);
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn show_toast(_message: &Message) -> Result<(), String> {
    Err("OS toast is not supported on this platform".to_string())
}

/// POST one JSON payload. `secret`, when present, becomes a bearer token.
///
/// Kept public for focused tests with a local TCP listener; production callers
/// use [`dispatch`], which resolves the secret from the credential store.
pub fn deliver_webhook(
    url: &str,
    name: &str,
    payload: &Value,
    secret: Option<&str>,
) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
    let mut request = agent
        .post(url)
        .set("Content-Type", "application/json")
        .set(
            "User-Agent",
            &format!("impresario/{}", env!("CARGO_PKG_VERSION")),
        )
        .set("X-Impresario-Webhook", name);
    if let Some(secret) = secret {
        request = request.set("Authorization", &format!("Bearer {secret}"));
    }
    let body = serde_json::to_vec(payload)
        .map_err(|error| format!("webhook '{name}' payload could not be encoded: {error}"))?;
    match request.send_bytes(&body) {
        Ok(response) if (200..300).contains(&response.status()) => Ok(()),
        Ok(response) => Err(format!(
            "webhook '{name}' returned HTTP {}",
            response.status()
        )),
        Err(ureq::Error::Status(status, _)) => {
            Err(format!("webhook '{name}' returned HTTP {status}"))
        }
        Err(error) => Err(format!("webhook '{name}' request failed: {error}")),
    }
}

/// Read a webhook secret, preferring the headless environment override over
/// the OS credential store.
#[must_use]
pub fn webhook_secret(name: &str) -> Option<String> {
    if let Some(secret) = env_webhook_secret(name) {
        return Some(secret);
    }
    keyring::Entry::new(KEYRING_SERVICE, &format!("webhook:{name}"))
        .and_then(|entry| entry.get_password())
        .ok()
        .filter(|secret| !secret.is_empty())
}

/// Store a webhook secret in the OS credential store.
pub fn set_webhook_secret(name: &str, secret: &str) -> Result<(), String> {
    if secret.is_empty() {
        return Err("the webhook secret must not be empty".to_string());
    }
    keyring::Entry::new(KEYRING_SERVICE, &format!("webhook:{name}"))
        .and_then(|entry| entry.set_password(secret))
        .map_err(|error| {
            format!(
                "cannot store webhook '{name}' secret in the OS credential store: {error}; \
                 set {} for a headless session",
                env_webhook_secret_name(name)
            )
        })
}

/// Remove a webhook secret. `Ok(false)` means there was nothing to remove.
pub fn clear_webhook_secret(name: &str) -> Result<bool, String> {
    match keyring::Entry::new(KEYRING_SERVICE, &format!("webhook:{name}"))
        .and_then(|entry| entry.delete_credential())
    {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(format!(
            "cannot remove webhook '{name}' secret from the OS credential store: {error}"
        )),
    }
}

/// Whether a secret is stored in the OS credential store (the environment
/// override is reported separately by the CLI).
pub fn stored_webhook_secret(name: &str) -> Result<bool, String> {
    match keyring::Entry::new(KEYRING_SERVICE, &format!("webhook:{name}"))
        .and_then(|entry| entry.get_password())
    {
        Ok(secret) => Ok(!secret.is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(format!(
            "cannot read webhook '{name}' secret from the OS credential store: {error}"
        )),
    }
}

/// The environment variable used as a headless webhook-secret override.
#[must_use]
pub fn env_webhook_secret_name(name: &str) -> String {
    let mut key = String::from("IMPRESARIO_WEBHOOK_");
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            key.push(character.to_ascii_uppercase());
        } else {
            key.push('_');
        }
    }
    key.push_str("_SECRET");
    key
}

fn env_webhook_secret(name: &str) -> Option<String> {
    std::env::var(env_webhook_secret_name(name))
        .ok()
        .filter(|secret| !secret.is_empty())
}

/// Validate the name used by `webhook:NAME` and by the secret commands.
#[must_use]
pub fn webhook_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// All top-level notification events, oldest first, limited to the newest
/// `limit` entries. `limit` zero returns none.
pub fn notification_events(home: &Home, limit: usize) -> Result<Vec<Value>, String> {
    let mut events: Vec<Value> = read_events(home)?
        .into_iter()
        .filter(|event| {
            matches!(
                event.get("kind").and_then(Value::as_str),
                Some("watch_notify" | "schedule_notify")
            )
        })
        .collect();
    let keep_from = events.len().saturating_sub(limit);
    Ok(events.split_off(keep_from))
}

/// Read every resident event, ignoring malformed/truncated lines.
pub fn read_events(home: &Home) -> Result<Vec<Value>, String> {
    let path = home.service_events_path();
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    Ok(source
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect())
}

/// Resolve a notification id or job id to a report file for `notify open`.
///
/// The event log is authoritative because it also covers watcher dumps. A job
/// record is the fallback when the event has been rotated away.
pub fn resolve_report(home: &Home, id: &str) -> Result<PathBuf, String> {
    let direct = PathBuf::from(id);
    if direct.is_file() {
        return Ok(direct);
    }
    for event in read_events(home)?.iter().rev() {
        let matches_id = event.get("id").and_then(Value::as_str) == Some(id)
            || event.get("job").and_then(Value::as_str) == Some(id);
        if !matches_id {
            continue;
        }
        if let Some(report) = event.get("report").and_then(Value::as_str) {
            let path = PathBuf::from(report);
            if path.is_file() {
                return Ok(path);
            }
        }
        if let Some(dump) = event.get("dump").and_then(Value::as_str) {
            if let Some(report) = report_in_dir(Path::new(dump)) {
                return Ok(report);
            }
        }
    }
    if let Ok(Some(record)) = JobStore::new(home).read(id) {
        if let Some(report) = report_in_dir(Path::new(&record.run_dir)) {
            return Ok(report);
        }
    }
    Err(format!(
        "no report found for notification '{id}'; pass a report path or check `impresario notify list`"
    ))
}

fn report_in_dir(directory: &Path) -> Option<PathBuf> {
    ["report.html", "report.json"]
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_core::resident::QuietHours;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    fn message() -> Message {
        Message {
            kind: "watch",
            source: "soak".to_string(),
            id: "job-1".to_string(),
            event: "failure",
            status: "failed".to_string(),
            signature: Some("tv/P/A/S/step:0/CHECK_FAILED".to_string()),
            count: 1,
            job: Some("job-1".to_string()),
            report: Some(PathBuf::from("reports/job-1/report.html")),
            dump: None,
            exit_code: Some(1),
            at: "2026-11-05T00:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn quiet_hours_boundaries_wrap_and_exclude_the_end() {
        let notifications = Notifications {
            quiet_hours: Some(QuietHours {
                start: "22:00".to_string(),
                end: "07:00".to_string(),
            }),
            ..Notifications::default()
        };
        assert!(
            !notifications.is_quiet_at(chrono::NaiveTime::from_hms_opt(21, 59, 0).expect("time"))
        );
        assert!(notifications.is_quiet_at(chrono::NaiveTime::from_hms_opt(22, 0, 0).expect("time")));
        assert!(notifications.is_quiet_at(chrono::NaiveTime::from_hms_opt(6, 59, 0).expect("time")));
        assert!(!notifications.is_quiet_at(chrono::NaiveTime::from_hms_opt(7, 0, 0).expect("time")));
    }

    #[test]
    fn rate_limit_allows_max_then_suppresses_until_the_window_expires() {
        let mut entries = BTreeMap::new();
        let limit = RateLimit {
            max: 2,
            window: "1h".to_string(),
        };
        let start = Local::now();
        assert!(allow_rate(&mut entries, "toast", &limit, start));
        assert!(allow_rate(&mut entries, "toast", &limit, start));
        assert!(!allow_rate(&mut entries, "toast", &limit, start));
        assert!(allow_rate(
            &mut entries,
            "toast",
            &limit,
            start + chrono::Duration::hours(2)
        ));
        assert_eq!(entries.get("toast").map(|entry| entry.count), Some(1));
    }

    #[test]
    fn invalid_rate_limits_fail_open() {
        let mut entries = BTreeMap::new();
        let now = Local::now();
        assert!(allow_rate(
            &mut entries,
            "toast",
            &RateLimit {
                max: 0,
                window: "1h".to_string(),
            },
            now,
        ));
        assert!(allow_rate(
            &mut entries,
            "toast",
            &RateLimit {
                max: 1,
                window: "0s".to_string(),
            },
            now,
        ));
        assert!(entries.is_empty());
    }

    #[test]
    fn payload_carries_report_and_never_a_secret() {
        let value = message().payload();
        assert_eq!(value["id"], "job-1");
        assert_eq!(value["event"], "failure");
        assert_eq!(value["report"], "reports/job-1/report.html");
        assert!(value.get("secret").is_none());
    }

    #[test]
    fn webhook_posts_json_with_a_bearer_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let header_end = loop {
                let read = stream.read(&mut buffer).expect("read");
                assert!(read > 0, "request closed before headers");
                request.extend_from_slice(&buffer[..read]);
                if let Some(index) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n".as_slice())
                {
                    break index + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .find_map(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).expect("read body");
                assert!(read > 0, "request closed before body");
                request.extend_from_slice(&buffer[..read]);
            }
            stream
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("respond");
            String::from_utf8_lossy(&request).into_owned()
        });
        deliver_webhook(
            &format!("http://{address}/hook"),
            "ops",
            &message().payload(),
            Some("top-secret"),
        )
        .expect("delivery");
        let request = server.join().expect("server");
        assert!(request.starts_with("POST /hook HTTP/1.1"), "{request}");
        let authorization = request
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.trim())
            .expect("authorization header");
        assert_eq!(authorization, "Bearer top-secret", "{request}");
        assert!(request.contains("\"signature\""), "{request}");
    }
}
