//! The `CdpBackend` trait (spec 10.2: the only surface the executor uses) and
//! the backend implementations.

pub mod raw;

#[cfg(feature = "chromiumoxide-backend")]
pub mod chromiumoxide;

use std::pin::Pin;

use futures::Stream;

use impresario_core::command::WireErrorCode;

/// Failure of one backend call, carrying the wire code it maps onto.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct BackendError {
    pub code: WireErrorCode,
    pub message: String,
}

impl BackendError {
    #[must_use]
    pub fn new(code: WireErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn device_unavailable(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::DeviceUnavailable, message)
    }

    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::UnsupportedAction, message)
    }

    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::Timeout, message)
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::Internal, message)
    }
}

/// CDP `Input.dispatchKeyEvent` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    KeyDown,
    KeyUp,
    RawKeyDown,
}

impl KeyEventType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            KeyEventType::KeyDown => "keyDown",
            KeyEventType::KeyUp => "keyUp",
            KeyEventType::RawKeyDown => "rawKeyDown",
        }
    }
}

/// One `Input.dispatchKeyEvent` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub kind: KeyEventType,
    pub key: String,
    pub code: String,
    pub key_code: u32,
    pub windows_vk: u32,
    pub native_vk: u32,
    /// Only sent with `KeyDown` events.
    pub text: Option<String>,
}

/// CDP `Input.dispatchMouseEvent` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventType {
    Moved,
    Pressed,
    Released,
}

impl MouseEventType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MouseEventType::Moved => "mouseMoved",
            MouseEventType::Pressed => "mousePressed",
            MouseEventType::Released => "mouseReleased",
        }
    }
}

/// One `Input.dispatchMouseEvent` call.
#[derive(Debug, Clone, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseEventType,
    pub x: f64,
    pub y: f64,
    pub button: &'static str,
    pub buttons: u32,
    pub click_count: u32,
}

/// Console severity used by page log events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// One page console/log event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleEvent {
    pub level: ConsoleLevel,
    pub source: String,
    pub message: String,
}

/// The CDP surface the executor drives (spec 10.2). One trait object per
/// session; all calls are correlated by the implementation and safe to run
/// concurrently (the out-of-band screenshot overlaps in-flight commands).
#[async_trait::async_trait]
pub trait CdpBackend: Send + Sync {
    /// `Runtime.evaluate` with `returnByValue` and `awaitPromise`.
    async fn evaluate(&self, expression: &str) -> Result<serde_json::Value, BackendError>;
    async fn dispatch_key(&self, event: &KeyEvent) -> Result<(), BackendError>;
    async fn dispatch_mouse(&self, event: &MouseEvent) -> Result<(), BackendError>;
    async fn insert_text(&self, text: &str) -> Result<(), BackendError>;
    /// `Page.captureScreenshot` as PNG bytes.
    async fn screenshot(&self) -> Result<Vec<u8>, BackendError>;
    async fn navigate(&self, url: &str) -> Result<(), BackendError>;
    async fn reload(&self) -> Result<(), BackendError>;
    /// Console events from `Runtime.consoleAPICalled` and `Log.entryAdded`.
    fn console_events(&self) -> Pin<Box<dyn Stream<Item = ConsoleEvent> + Send>>;
    /// Best-effort shutdown of the connection (idempotent).
    async fn close(&self);
}
