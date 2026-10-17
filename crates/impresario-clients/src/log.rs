//! Neutral log types shared by the session client, transports and the
//! reporters: executor logs from `StreamLogs` and host-side collector lines
//! (adb logcat, connector output) all become [`LogRecord`]s in a ring
//! [`LogBuffer`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use impresario_proto::v1 as pb;
use tonic::codec::Streaming;

use crate::agent_client::ClientError;
use crate::convert;

/// Log severity, mirroring `impresario.v1.LogLevel` without its sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// All levels in increasing severity.
    pub const ALL: [LogLevel; 4] = [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ];

    /// Upper-case label used in reports and `StreamLogs` filtering.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// One captured log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    /// Unix epoch milliseconds; 0 when the source had no timestamp.
    pub unix_ms: i64,
    pub level: LogLevel,
    /// `"agent"`, `"console"`, `"logcat"`, `"instrument"`, `"connector"`, ...
    pub source: String,
    pub message: String,
}

impl LogRecord {
    #[must_use]
    pub fn new(source: impl Into<String>, level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            unix_ms: unix_ms_now(),
            level,
            source: source.into(),
            message: message.into(),
        }
    }
}

/// Current unix time in milliseconds (0 when the clock is before the epoch).
#[must_use]
pub fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A bounded, shareable ring of log records (failure capture keeps the last
/// 200 entries; transports keep more).
#[derive(Clone, Debug)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogRecord>>>,
    capacity: usize,
}

impl LogBuffer {
    /// A buffer that keeps at most `capacity` records; `0` drops everything.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
            capacity,
        }
    }

    /// A buffer that records nothing (default for transports without
    /// host-side collectors).
    #[must_use]
    pub fn empty() -> Self {
        Self::new(0)
    }

    /// Append a record, dropping the oldest when over capacity.
    pub fn push(&self, record: LogRecord) {
        if self.capacity == 0 {
            return;
        }
        let mut inner = lock(&self.inner);
        if inner.len() >= self.capacity {
            inner.pop_front();
        }
        inner.push_back(record);
    }

    /// Every record, oldest first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<LogRecord> {
        lock(&self.inner).iter().cloned().collect()
    }

    /// The newest `n` records, oldest first.
    #[must_use]
    pub fn last(&self, n: usize) -> Vec<LogRecord> {
        let inner = lock(&self.inner);
        let skip = inner.len().saturating_sub(n);
        inner.iter().skip(skip).cloned().collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.inner).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        lock(&self.inner).is_empty()
    }
}

/// A live `StreamLogs` stream (spec 7.2.10).
pub struct LogStream {
    inner: Streaming<pb::LogEntry>,
}

impl LogStream {
    pub(crate) fn new(inner: Streaming<pb::LogEntry>) -> Self {
        Self { inner }
    }

    /// The next entry, or `None` when the executor closed the stream.
    pub async fn next(&mut self) -> Option<Result<LogRecord, ClientError>> {
        match self.inner.message().await {
            Ok(Some(entry)) => Some(Ok(convert::log_record_from_proto(&entry))),
            Ok(None) => None,
            Err(status) => Some(Err(ClientError::from_status(status))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(i: i64) -> LogRecord {
        LogRecord {
            unix_ms: i,
            level: LogLevel::Info,
            source: "test".to_string(),
            message: format!("line {i}"),
        }
    }

    #[test]
    fn ring_drops_oldest() {
        let buffer = LogBuffer::new(3);
        for i in 0..5 {
            buffer.push(record(i));
        }
        let all = buffer.snapshot();
        assert_eq!(all.len(), 3);
        assert_eq!(all.first().map(|r| r.unix_ms), Some(2));
        assert_eq!(all.last().map(|r| r.unix_ms), Some(4));
    }

    #[test]
    fn last_returns_newest_in_order() {
        let buffer = LogBuffer::new(10);
        for i in 0..10 {
            buffer.push(record(i));
        }
        let last = buffer.last(3);
        assert_eq!(
            last.iter().map(|r| r.unix_ms).collect::<Vec<_>>(),
            vec![7, 8, 9]
        );
    }

    #[test]
    fn empty_buffer_discards() {
        let buffer = LogBuffer::empty();
        buffer.push(record(1));
        assert!(buffer.is_empty());
    }

    #[test]
    fn levels_are_ordered() {
        assert!(LogLevel::Debug < LogLevel::Error);
        assert_eq!(LogLevel::Warn.as_str(), "WARN");
    }
}
