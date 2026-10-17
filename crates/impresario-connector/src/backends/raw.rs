//! `RawCdpBackend`: JSON-RPC over a CDP websocket via `tokio-tungstenite`
//! (spec 10.3): request/response correlation by id, per-call timeout, events
//! dispatched to listeners. Only the methods of spec 10.3 are used — no
//! `Target.*` and no newer optional methods.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use super::{
    BackendError, CdpBackend, ConsoleEvent, ConsoleLevel, KeyEvent, KeyEventType, MouseEvent,
};
use crate::errors::ConnectorError;

/// Connect + enable budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-call CDP timeout (a hung page must not hold the executor forever; the
/// command deadline is still enforced on top by the polling loops).
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Console/log broadcast depth.
const CONSOLE_CAPACITY: usize = 512;

struct RawState {
    next_id: AtomicU64,
    write: mpsc::Sender<Message>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, BackendError>>>>,
    closed: Mutex<Option<String>>,
    console: broadcast::Sender<ConsoleEvent>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RawState {
    fn is_closed(&self) -> Option<String> {
        lock(&self.closed).clone()
    }

    fn fail_all(&self, reason: &str) {
        {
            let mut closed = lock(&self.closed);
            if closed.is_none() {
                *closed = Some(reason.to_string());
            }
        }
        let pending = {
            let mut pending = lock(&self.pending);
            std::mem::take(&mut *pending)
        };
        for (_, sender) in pending {
            let _ = sender.send(Err(BackendError::device_unavailable(reason.to_string())));
        }
    }
}

/// The raw websocket backend.
pub struct RawCdpBackend {
    state: Arc<RawState>,
    /// Shared with the `chromiumoxide` backend (tests parameterise on it).
    pub call_timeout: Duration,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

impl RawCdpBackend {
    /// Connect, start the read/write pumps and enable the required domains
    /// (`Runtime`, `Log`, `Page`; spec 10.3).
    pub async fn connect(ws_url: &str) -> Result<Self, BackendError> {
        let connected =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(ws_url))
                .await
                .map_err(|_| {
                    BackendError::device_unavailable(format!("connecting to {ws_url} timed out"))
                })?;
        let (stream, _response) = connected.map_err(|error| {
            BackendError::device_unavailable(format!("cannot open {ws_url}: {error}"))
        })?;
        let (mut sink, mut source) = stream.split();

        let (write_tx, mut write_rx) = mpsc::channel::<Message>(64);
        let (console_tx, _) = broadcast::channel(CONSOLE_CAPACITY);
        let state = Arc::new(RawState {
            next_id: AtomicU64::new(0),
            write: write_tx,
            pending: Mutex::new(HashMap::new()),
            closed: Mutex::new(None),
            console: console_tx,
        });

        let writer_state = state.clone();
        let writer = tokio::spawn(async move {
            while let Some(message) = write_rx.recv().await {
                if let Err(error) = sink.send(message).await {
                    writer_state.fail_all(&format!("CDP write failed: {error}"));
                    break;
                }
            }
        });

        let reader_state = state.clone();
        let reader = tokio::spawn(async move {
            while let Some(message) = source.next().await {
                match message {
                    Ok(Message::Text(text)) => handle_text(&reader_state, text.as_str()),
                    Ok(Message::Close(frame)) => {
                        let reason = frame.map_or_else(
                            || "closed by the device".to_string(),
                            |frame| format!("closed by the device: {}", frame.reason),
                        );
                        reader_state.fail_all(&reason);
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        reader_state.fail_all(&format!("CDP read failed: {error}"));
                        break;
                    }
                }
            }
        });

        let backend = Self {
            state,
            call_timeout: CALL_TIMEOUT,
            reader,
            writer,
        };
        for method in ["Runtime.enable", "Log.enable", "Page.enable"] {
            backend.call(method, json!({})).await.map_err(|error| {
                BackendError::device_unavailable(format!("{method} failed on {ws_url}: {error}"))
            })?;
        }
        Ok(backend)
    }

    /// One JSON-RPC call: correlate the reply by id, enforce the per-call
    /// timeout, surface `-32601` as `UNSUPPORTED_ACTION`.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, BackendError> {
        if let Some(reason) = self.state.is_closed() {
            return Err(BackendError::device_unavailable(format!(
                "the CDP connection is closed: {reason}"
            )));
        }
        let id = self.state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = oneshot::channel();
        lock(&self.state.pending).insert(id, sender);
        let payload = json!({ "id": id, "method": method, "params": params });
        if self
            .state
            .write
            .send(Message::text(payload.to_string()))
            .await
            .is_err()
        {
            lock(&self.state.pending).remove(&id);
            return Err(BackendError::device_unavailable(
                "the CDP write channel is closed".to_string(),
            ));
        }
        match tokio::time::timeout(self.call_timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(BackendError::device_unavailable(
                self.state
                    .is_closed()
                    .unwrap_or_else(|| "the CDP connection ended".to_string()),
            )),
            Err(_) => {
                lock(&self.state.pending).remove(&id);
                Err(BackendError::timeout(format!(
                    "CDP method {method} did not answer within {} ms",
                    self.call_timeout.as_millis()
                )))
            }
        }
    }
}

fn handle_text(state: &Arc<RawState>, text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        tracing::debug!("ignoring a non-JSON CDP frame");
        return;
    };
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        let waiter = lock(&state.pending).remove(&id);
        if let Some(sender) = waiter {
            let result = if let Some(error) = value.get("error") {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                if code == -32601 {
                    Err(BackendError::unsupported(format!(
                        "the device does not support this CDP method (error {code}: {message})"
                    )))
                } else {
                    Err(BackendError::internal(format!(
                        "CDP error {code}: {message}"
                    )))
                }
            } else {
                Ok(value.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = sender.send(result);
        }
        return;
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "Runtime.consoleAPICalled" => {
            let level = console_level(params.get("type").and_then(Value::as_str).unwrap_or("log"));
            let message = console_message(&params);
            let _ = state.console.send(ConsoleEvent {
                level,
                source: "console".to_string(),
                message,
            });
        }
        "Log.entryAdded" => {
            let entry = params.get("entry").cloned().unwrap_or(Value::Null);
            let level = console_level(entry.get("level").and_then(Value::as_str).unwrap_or("info"));
            let message = entry
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let source = entry
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("log")
                .to_string();
            let _ = state.console.send(ConsoleEvent {
                level,
                source,
                message,
            });
        }
        "Inspector.detached" => {
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("detached");
            state.fail_all(&format!("detached: {reason}"));
        }
        _ => {}
    }
}

fn console_level(kind: &str) -> ConsoleLevel {
    match kind {
        "warning" | "warn" => ConsoleLevel::Warn,
        "error" | "assert" => ConsoleLevel::Error,
        "info" => ConsoleLevel::Info,
        _ => ConsoleLevel::Debug,
    }
}

fn console_message(params: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(args) = params.get("args").and_then(Value::as_array) {
        for arg in args {
            parts.push(console_value(arg));
        }
    }
    parts.join(" ")
}

fn console_value(arg: &Value) -> String {
    if let Some(value) = arg.get("value") {
        if let Some(text) = value.as_str() {
            return text.to_string();
        }
        return value.to_string();
    }
    if let Some(description) = arg.get("description").and_then(Value::as_str) {
        return description.to_string();
    }
    if let Some(raw) = arg.get("unserializableValue").and_then(Value::as_str) {
        return raw.to_string();
    }
    arg.get("type")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_string()
}

fn exception_summary(result: &Value) -> Option<String> {
    let details = result.get("exceptionDetails")?;
    let text = details.get("text").and_then(Value::as_str).unwrap_or("");
    let description = details
        .get("exception")
        .and_then(|exception| exception.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("");
    Some(if description.is_empty() {
        text.to_string()
    } else {
        description.to_string()
    })
}

#[async_trait::async_trait]
impl CdpBackend for RawCdpBackend {
    async fn evaluate(&self, expression: &str) -> Result<Value, BackendError> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(summary) = exception_summary(&result) {
            return Err(BackendError::internal(format!(
                "the page raised an exception: {summary}"
            )));
        }
        Ok(result
            .get("result")
            .and_then(|inner| inner.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn dispatch_key(&self, event: &KeyEvent) -> Result<(), BackendError> {
        let mut params = json!({
            "type": event.kind.as_str(),
            "key": event.key.clone(),
            "code": event.code.clone(),
            "windowsVirtualKeyCode": event.windows_vk,
            "nativeVirtualKeyCode": event.native_vk,
        });
        if let (KeyEventType::KeyDown, Some(text), Some(map)) =
            (event.kind, event.text.as_ref(), params.as_object_mut())
        {
            map.insert("text".to_string(), Value::String(text.clone()));
            map.insert("unmodifiedText".to_string(), Value::String(text.clone()));
        }
        self.call("Input.dispatchKeyEvent", params).await?;
        Ok(())
    }

    async fn dispatch_mouse(&self, event: &MouseEvent) -> Result<(), BackendError> {
        self.call(
            "Input.dispatchMouseEvent",
            json!({
                "type": event.kind.as_str(),
                "x": event.x,
                "y": event.y,
                "button": event.button,
                "buttons": event.buttons,
                "clickCount": event.click_count,
            }),
        )
        .await?;
        Ok(())
    }

    async fn insert_text(&self, text: &str) -> Result<(), BackendError> {
        self.call("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    async fn screenshot(&self) -> Result<Vec<u8>, BackendError> {
        let result = self
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| BackendError::internal("Page.captureScreenshot returned no data"))?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|error| BackendError::internal(format!("invalid screenshot base64: {error}")))
    }

    async fn navigate(&self, url: &str) -> Result<(), BackendError> {
        let result = self.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(error_text) = result.get("errorText").and_then(Value::as_str) {
            if !error_text.is_empty() {
                return Err(BackendError::device_unavailable(format!(
                    "navigation to {url} failed: {error_text}"
                )));
            }
        }
        Ok(())
    }

    async fn reload(&self) -> Result<(), BackendError> {
        self.call("Page.reload", json!({})).await?;
        Ok(())
    }

    fn console_events(
        &self,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = ConsoleEvent> + Send>> {
        Box::pin(
            tokio_stream::wrappers::BroadcastStream::new(self.state.console.subscribe())
                .filter_map(|event| async move { event.ok() }),
        )
    }

    async fn close(&self) {
        self.reader.abort();
        self.writer.abort();
        self.state.fail_all("the session closed the connection");
    }
}

/// Map a backend error onto a connector error (identity for the shared codes).
#[must_use]
pub fn backend_error(error: BackendError) -> ConnectorError {
    ConnectorError::new(error.code, error.message)
}

impl From<BackendError> for ConnectorError {
    fn from(error: BackendError) -> Self {
        backend_error(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_levels_map() {
        assert_eq!(console_level("warning"), ConsoleLevel::Warn);
        assert_eq!(console_level("error"), ConsoleLevel::Error);
        assert_eq!(console_level("info"), ConsoleLevel::Info);
        assert_eq!(console_level("log"), ConsoleLevel::Debug);
    }

    #[test]
    fn console_arguments_render() {
        let params = json!({"args": [
            {"type": "string", "value": "hello"},
            {"type": "number", "value": 42},
            {"type": "object", "description": "Object"},
            {"type": "number", "unserializableValue": "NaN"}
        ]});
        assert_eq!(console_message(&params), "hello 42 Object NaN");
    }

    #[test]
    fn exception_summary_prefers_descriptions() {
        let plain = json!({"exceptionDetails": {"text": "Uncaught"}});
        assert_eq!(exception_summary(&plain).as_deref(), Some("Uncaught"));
        let rich = json!({"exceptionDetails": {"text": "Uncaught", "exception": {"description": "Error: boom"}}});
        assert_eq!(exception_summary(&rich).as_deref(), Some("Error: boom"));
        assert_eq!(exception_summary(&json!({})), None);
    }

    #[test]
    fn backend_errors_keep_codes() {
        use impresario_core::command::WireErrorCode;
        let error = BackendError::unsupported("no such method");
        assert_eq!(error.code, WireErrorCode::UnsupportedAction);
        let converted: ConnectorError = error.into();
        assert_eq!(converted.code, WireErrorCode::UnsupportedAction);
        assert_eq!(converted.message, "no such method");
    }
}
