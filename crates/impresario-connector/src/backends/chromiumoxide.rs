//! `ChromiumoxideBackend`: the `chromiumoxide`-crate implementation of the
//! [`CdpBackend`] surface (feature `chromiumoxide-backend`, option
//! `cdp_backend=chromiumoxide`, spec 10.1).
//!
//! chromiumoxide drives a *browser-level* CDP connection (Target discovery
//! plus flattened page sessions), unlike [`RawCdpBackend`], which talks to
//! the page directly. The session therefore hands this backend the browser
//! websocket (`/json/version`) plus the resolved page target id; page
//! selection here reproduces the `page_match` semantics of spec 10.3.

use std::pin::Pin;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    InsertTextParams, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::log::{EventEntryAdded, LogEntry, LogEntryLevel};
use chromiumoxide::cdp::browser_protocol::page::{CaptureScreenshotFormat, NavigateParams};
use chromiumoxide::cdp::js_protocol::runtime::{
    ConsoleApiCalledType, EvaluateParams, EventConsoleApiCalled, ExceptionDetails, RemoteObject,
};
use chromiumoxide::error::CdpError;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{Browser, Page};

use impresario_core::command::WireErrorCode;

use super::{
    BackendError, CdpBackend, ConsoleEvent, ConsoleLevel, KeyEvent, KeyEventType, MouseEvent,
    MouseEventType,
};

/// How long the page target may take to appear (and answer) after connect.
const PAGE_READY_BUDGET: Duration = Duration::from_secs(10);
/// Poll spacing while waiting for the target.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn abort_all(tasks: &[JoinHandle<()>]) {
    for task in tasks {
        task.abort();
    }
}

/// The `chromiumoxide` backend: one page handle plus the browser connection
/// that carries it.
pub struct ChromiumoxideBackend {
    /// Keeps the chromiumoxide handler connection alive (dropped last).
    _browser: Browser,
    page: Page,
    console: broadcast::Sender<ConsoleEvent>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl ChromiumoxideBackend {
    /// Connect to the browser websocket, wait for the resolved page target and
    /// hook up the console/log pumps.
    pub async fn connect(
        browser_ws: &str,
        page_id: Option<&str>,
        page_match: Option<&str>,
    ) -> Result<Self, BackendError> {
        let (browser, mut handler) =
            Browser::connect(browser_ws.to_string())
                .await
                .map_err(|error| {
                    BackendError::device_unavailable(format!(
                    "chromiumoxide cannot connect to the browser websocket {browser_ws}: {error}"
                ))
                })?;
        let mut tasks = vec![tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        })];

        let page = match wait_for_page(&browser, page_id, page_match).await {
            Ok(page) => page,
            Err(error) => {
                abort_all(&tasks);
                return Err(error);
            }
        };
        if let Err(error) = page.enable_runtime().await {
            abort_all(&tasks);
            return Err(map_cdp_error(error));
        }
        if let Err(error) = page.enable_log().await {
            abort_all(&tasks);
            return Err(map_cdp_error(error));
        }
        if let Err(error) = wait_page_ready(&page).await {
            abort_all(&tasks);
            return Err(error);
        }

        let (console, _) = broadcast::channel(512);
        match page.event_listener::<EventConsoleApiCalled>().await {
            Ok(mut stream) => {
                let tx = console.clone();
                tasks.push(tokio::spawn(async move {
                    while let Some(event) = stream.next().await {
                        let _ = tx.send(console_event(&event));
                    }
                }));
            }
            Err(error) => {
                abort_all(&tasks);
                return Err(map_cdp_error(error));
            }
        }
        match page.event_listener::<EventEntryAdded>().await {
            Ok(mut stream) => {
                let tx = console.clone();
                tasks.push(tokio::spawn(async move {
                    while let Some(event) = stream.next().await {
                        let _ = tx.send(log_event(&event.entry));
                    }
                }));
            }
            Err(error) => {
                abort_all(&tasks);
                return Err(map_cdp_error(error));
            }
        }

        Ok(Self {
            _browser: browser,
            page,
            console,
            tasks: Mutex::new(tasks),
        })
    }
}

/// Poll `Target`s until the resolved page target appears (spec 10.3
/// selection: by target id, else `page_match`, else exactly one).
async fn wait_for_page(
    browser: &Browser,
    page_id: Option<&str>,
    page_match: Option<&str>,
) -> Result<Page, BackendError> {
    let deadline = tokio::time::Instant::now() + PAGE_READY_BUDGET;
    loop {
        let outcome = match browser.pages().await {
            Ok(pages) => select_page(&pages, page_id, page_match).await,
            Err(error) => Err(BackendError::new(
                WireErrorCode::Transport,
                format!("cannot list the browser targets: {error}"),
            )),
        };
        match outcome {
            Ok(page) => return Ok(page),
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
                tracing::debug!(%error, "waiting for the page target");
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn select_page(
    pages: &[Page],
    page_id: Option<&str>,
    page_match: Option<&str>,
) -> Result<Page, BackendError> {
    let mut candidates: Vec<Page> = Vec::new();
    for page in pages {
        if let Some(id) = page_id {
            if page.target_id().as_ref() != id {
                continue;
            }
        } else if let Some(needle) = page_match {
            let url = page.url().await.ok().flatten().unwrap_or_default();
            let title = page.get_title().await.ok().flatten().unwrap_or_default();
            if !(url.contains(needle) || title.contains(needle)) {
                continue;
            }
        }
        candidates.push(page.clone());
    }
    match candidates.len() {
        0 => Err(BackendError::device_unavailable(format!(
            "no page target{} yet; browser targets: [{}]",
            page_id.map_or(String::new(), |id| format!(" with id '{id}'")),
            describe_pages(pages).await
        ))),
        1 => Ok(candidates.remove(0)),
        _ => Err(BackendError::device_unavailable(format!(
            "expected exactly one page target, found {}: [{}]{}",
            candidates.len(),
            describe_pages(&candidates).await,
            page_match.map_or(String::new(), |needle| format!(" (page_match '{needle}')"))
        ))),
    }
}

async fn describe_pages(pages: &[Page]) -> String {
    let mut described = Vec::new();
    for page in pages {
        let url = page.url().await.ok().flatten().unwrap_or_default();
        let title = page.get_title().await.ok().flatten().unwrap_or_default();
        described.push(format!("{title} ({url})"));
    }
    described.join(", ")
}

/// Wait until the page's execution context answers (the session's first
/// command injects the helper immediately after the handshake).
async fn wait_page_ready(page: &Page) -> Result<(), BackendError> {
    let deadline = tokio::time::Instant::now() + PAGE_READY_BUDGET;
    loop {
        let outcome = match evaluate_params("1+1") {
            Ok(params) => page.evaluate(params).await.map_err(map_cdp_error),
            Err(error) => Err(error),
        };
        match outcome {
            Ok(_) => return Ok(()),
            Err(error) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
                tracing::debug!(%error, "waiting for the page to answer");
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn evaluate_params(expression: &str) -> Result<EvaluateParams, BackendError> {
    EvaluateParams::builder()
        .expression(expression)
        .return_by_value(true)
        .await_promise(true)
        .build()
        .map_err(|error| {
            BackendError::internal(format!("cannot build the evaluate command: {error}"))
        })
}

fn key_params(event: &KeyEvent) -> DispatchKeyEventParams {
    let mut params = DispatchKeyEventParams::new(match event.kind {
        KeyEventType::KeyDown => DispatchKeyEventType::KeyDown,
        KeyEventType::KeyUp => DispatchKeyEventType::KeyUp,
        KeyEventType::RawKeyDown => DispatchKeyEventType::RawKeyDown,
    });
    params.key = Some(event.key.clone());
    params.code = Some(event.code.clone());
    params.windows_virtual_key_code = Some(i64::from(event.windows_vk));
    params.native_virtual_key_code = Some(i64::from(event.native_vk));
    if let (KeyEventType::KeyDown, Some(text)) = (event.kind, event.text.as_ref()) {
        params.text = Some(text.clone());
        params.unmodified_text = Some(text.clone());
    }
    params
}

fn mouse_params(event: &MouseEvent) -> DispatchMouseEventParams {
    let mut params = DispatchMouseEventParams::new(
        match event.kind {
            MouseEventType::Moved => DispatchMouseEventType::MouseMoved,
            MouseEventType::Pressed => DispatchMouseEventType::MousePressed,
            MouseEventType::Released => DispatchMouseEventType::MouseReleased,
        },
        event.x,
        event.y,
    );
    params.button = Some(mouse_button(event.button));
    params.buttons = Some(i64::from(event.buttons));
    params.click_count = Some(i64::from(event.click_count));
    params
}

fn mouse_button(name: &str) -> MouseButton {
    match name {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        "back" => MouseButton::Back,
        "forward" => MouseButton::Forward,
        _ => MouseButton::None,
    }
}

fn console_event(event: &EventConsoleApiCalled) -> ConsoleEvent {
    ConsoleEvent {
        level: console_level(&event.r#type),
        source: "console".to_string(),
        message: console_message(&event.args),
    }
}

fn console_level(kind: &ConsoleApiCalledType) -> ConsoleLevel {
    match kind {
        ConsoleApiCalledType::Warning => ConsoleLevel::Warn,
        ConsoleApiCalledType::Error | ConsoleApiCalledType::Assert => ConsoleLevel::Error,
        ConsoleApiCalledType::Info => ConsoleLevel::Info,
        _ => ConsoleLevel::Debug,
    }
}

fn console_message(objects: &[RemoteObject]) -> String {
    objects
        .iter()
        .map(console_value)
        .collect::<Vec<_>>()
        .join(" ")
}

fn console_value(object: &RemoteObject) -> String {
    if let Some(value) = object.value.as_ref() {
        if let Some(text) = value.as_str() {
            return text.to_string();
        }
        return value.to_string();
    }
    if let Some(description) = object.description.as_deref() {
        return description.to_string();
    }
    if let Some(raw) = object.unserializable_value.as_ref() {
        return raw.as_ref().to_string();
    }
    object.r#type.as_ref().to_string()
}

fn log_event(entry: &LogEntry) -> ConsoleEvent {
    ConsoleEvent {
        level: match entry.level {
            LogEntryLevel::Warning => ConsoleLevel::Warn,
            LogEntryLevel::Error => ConsoleLevel::Error,
            LogEntryLevel::Info => ConsoleLevel::Info,
            LogEntryLevel::Verbose => ConsoleLevel::Debug,
        },
        source: entry.source.as_ref().to_string(),
        message: entry.text.clone(),
    }
}

fn map_cdp_error(error: CdpError) -> BackendError {
    match &error {
        CdpError::JavascriptException(details) => BackendError::internal(format!(
            "the page raised an exception: {}",
            exception_text(details)
        )),
        CdpError::Timeout => BackendError::timeout("the CDP call timed out"),
        CdpError::ChromeMessage(message) if is_missing_method(message) => {
            BackendError::unsupported(message.clone())
        }
        _ => BackendError::new(WireErrorCode::Transport, error.to_string()),
    }
}

fn exception_text(details: &ExceptionDetails) -> String {
    if let Some(description) = details
        .exception
        .as_ref()
        .and_then(|exception| exception.description.as_deref())
    {
        if !description.is_empty() {
            return description.to_string();
        }
    }
    details.text.clone()
}

/// Chromium answers "-32601" for methods the runtime does not implement;
/// those map onto `UNSUPPORTED_ACTION` (spec 7.1).
fn is_missing_method(message: &str) -> bool {
    message.contains("-32601")
        || message.contains("wasn't found")
        || message.contains("was not found")
        || message.contains("Method not found")
}

#[async_trait::async_trait]
impl CdpBackend for ChromiumoxideBackend {
    async fn evaluate(&self, expression: &str) -> Result<serde_json::Value, BackendError> {
        let params = evaluate_params(expression)?;
        let result = self.page.evaluate(params).await.map_err(map_cdp_error)?;
        Ok(result.value().cloned().unwrap_or(serde_json::Value::Null))
    }

    async fn dispatch_key(&self, event: &KeyEvent) -> Result<(), BackendError> {
        self.page
            .execute(key_params(event))
            .await
            .map_err(map_cdp_error)?;
        Ok(())
    }

    async fn dispatch_mouse(&self, event: &MouseEvent) -> Result<(), BackendError> {
        self.page
            .execute(mouse_params(event))
            .await
            .map_err(map_cdp_error)?;
        Ok(())
    }

    async fn insert_text(&self, text: &str) -> Result<(), BackendError> {
        self.page
            .execute(InsertTextParams::new(text))
            .await
            .map_err(map_cdp_error)?;
        Ok(())
    }

    async fn screenshot(&self) -> Result<Vec<u8>, BackendError> {
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .build();
        self.page.screenshot(params).await.map_err(map_cdp_error)
    }

    async fn navigate(&self, url: &str) -> Result<(), BackendError> {
        // `Page.navigate` semantics (like the raw backend): return at
        // navigation start, not page load — the executor waits for readiness.
        let response = self
            .page
            .execute(NavigateParams::new(url))
            .await
            .map_err(map_cdp_error)?;
        if let Some(error_text) = response.result.error_text.as_deref() {
            if !error_text.is_empty() {
                return Err(BackendError::device_unavailable(format!(
                    "navigation to {url} failed: {error_text}"
                )));
            }
        }
        Ok(())
    }

    async fn reload(&self) -> Result<(), BackendError> {
        self.page.reload().await.map_err(map_cdp_error)?;
        Ok(())
    }

    fn console_events(&self) -> Pin<Box<dyn Stream<Item = ConsoleEvent> + Send>> {
        Box::pin(
            tokio_stream::wrappers::BroadcastStream::new(self.console.subscribe())
                .filter_map(|event| async move { event.ok() }),
        )
    }

    async fn close(&self) {
        abort_all(&lock(&self.tasks));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_levels_follow_the_raw_backend() {
        assert_eq!(
            console_level(&ConsoleApiCalledType::Warning),
            ConsoleLevel::Warn
        );
        assert_eq!(
            console_level(&ConsoleApiCalledType::Error),
            ConsoleLevel::Error
        );
        assert_eq!(
            console_level(&ConsoleApiCalledType::Assert),
            ConsoleLevel::Error
        );
        assert_eq!(
            console_level(&ConsoleApiCalledType::Info),
            ConsoleLevel::Info
        );
        assert_eq!(
            console_level(&ConsoleApiCalledType::Log),
            ConsoleLevel::Debug
        );
    }

    #[test]
    fn log_levels_follow_the_raw_backend() {
        let level = |level: LogEntryLevel| {
            log_event(&LogEntry {
                source: chromiumoxide::cdp::browser_protocol::log::LogEntrySource::Javascript,
                level,
                text: "hello".to_string(),
                timestamp: chromiumoxide::cdp::js_protocol::runtime::Timestamp::new(0.0),
                category: None,
                url: None,
                line_number: None,
                stack_trace: None,
                network_request_id: None,
                worker_id: None,
                args: None,
            })
        };
        assert_eq!(level(LogEntryLevel::Warning).level, ConsoleLevel::Warn);
        assert_eq!(level(LogEntryLevel::Error).level, ConsoleLevel::Error);
        assert_eq!(level(LogEntryLevel::Info).level, ConsoleLevel::Info);
        assert_eq!(level(LogEntryLevel::Verbose).level, ConsoleLevel::Debug);
        let entry = level(LogEntryLevel::Info);
        assert_eq!(entry.source, "javascript");
        assert_eq!(entry.message, "hello");
    }

    #[test]
    fn key_events_mirror_the_raw_payloads() {
        let event = KeyEvent {
            kind: KeyEventType::KeyDown,
            key: "Enter".to_string(),
            code: "Enter".to_string(),
            key_code: 13,
            windows_vk: 13,
            native_vk: 13,
            text: Some("\r".to_string()),
        };
        let params = key_params(&event);
        assert_eq!(params.r#type, DispatchKeyEventType::KeyDown);
        assert_eq!(params.key.as_deref(), Some("Enter"));
        assert_eq!(params.code.as_deref(), Some("Enter"));
        assert_eq!(params.windows_virtual_key_code, Some(13));
        assert_eq!(params.text.as_deref(), Some("\r"));

        let up = key_params(&KeyEvent {
            kind: KeyEventType::KeyUp,
            text: Some("ignored".to_string()),
            ..event
        });
        assert_eq!(up.r#type, DispatchKeyEventType::KeyUp);
        assert_eq!(up.text, None);
    }

    #[test]
    fn mouse_events_mirror_the_raw_payloads() {
        let event = MouseEvent {
            kind: MouseEventType::Pressed,
            x: 12.5,
            y: 30.0,
            button: "left",
            buttons: 1,
            click_count: 1,
        };
        let params = mouse_params(&event);
        assert_eq!(params.r#type, DispatchMouseEventType::MousePressed);
        assert_eq!(params.x, 12.5);
        assert_eq!(params.y, 30.0);
        assert_eq!(params.button, Some(MouseButton::Left));
        assert_eq!(params.buttons, Some(1));
        assert_eq!(params.click_count, Some(1));
        assert_eq!(mouse_button("none"), MouseButton::None);
        assert_eq!(mouse_button("mystery"), MouseButton::None);
    }

    #[test]
    fn missing_methods_become_unsupported() {
        assert!(is_missing_method(
            "-32601 'Page.captureScreenshot' wasn't found"
        ));
        assert!(is_missing_method("Method not found"));
        assert!(!is_missing_method("some other failure"));
        let mapped = map_cdp_error(CdpError::ChromeMessage(
            "-32601 'Page.captureScreenshot' wasn't found".to_string(),
        ));
        assert_eq!(mapped.code, WireErrorCode::UnsupportedAction);
        let timeout = map_cdp_error(CdpError::Timeout);
        assert_eq!(timeout.code, WireErrorCode::Timeout);
        let transport = map_cdp_error(CdpError::NoResponse);
        assert_eq!(transport.code, WireErrorCode::Transport);
    }
}
