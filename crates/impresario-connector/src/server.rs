//! The tonic `Agent` service (spec 7.1, 10.1): handshake, the per-session
//! command stream, out-of-band screenshots, log streaming and shutdown, plus
//! the [`SessionManager`] holding the live sessions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use impresario_core::platforms::Platform;
use impresario_proto::v1 as pb;
use pb::agent_server::Agent;

use crate::errors::ConnectorError;
use crate::session::{self, Session, SessionConfig, SessionHandle};

/// The session-id metadata key (mirrors `impresario-clients::agent_client`).
const SESSION_ID_KEY: &str = "x-impresario-session-id";
/// The shared-token metadata key (spec 7.2.1).
const TOKEN_KEY: &str = "x-impresario-token";

/// Wire protocol version served by this connector.
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// How long `Shutdown` waits for a running command stream to stop.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The live sessions of one connector process.
#[derive(Default)]
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<SessionHandle>>>,
    next_id: AtomicU64,
}

impl SessionManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh session id (handshake response).
    #[must_use]
    pub fn next_session_id(&self) -> String {
        let number = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        format!("session-{number}")
    }

    /// Store a freshly connected session.
    pub fn insert(&self, session: Session) -> Arc<SessionHandle> {
        let handle = Arc::new(SessionHandle::new(Arc::new(session)));
        lock(&self.sessions).insert(handle.session.id.clone(), handle.clone());
        handle
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<SessionHandle>> {
        lock(&self.sessions).get(id).cloned()
    }

    /// Remove a session (shutdown path; the handle keeps the resources alive).
    #[must_use]
    pub fn remove(&self, id: &str) -> Option<Arc<SessionHandle>> {
        lock(&self.sessions).remove(id)
    }

    #[must_use]
    pub fn session_count(&self) -> usize {
        lock(&self.sessions).len()
    }

    /// Stop every session (server shutdown): cancel, then tear down.
    pub async fn teardown_all(&self) {
        let handles: Vec<Arc<SessionHandle>> = lock(&self.sessions)
            .drain()
            .map(|(_, handle)| handle)
            .collect();
        for handle in handles {
            handle.shutdown.cancel();
            if handle.stream_taken.load(Ordering::SeqCst) {
                handle.wait_closed(SHUTDOWN_GRACE).await;
            }
            handle.session.teardown().await;
        }
    }
}

/// The `Agent` service (spec 7.1): one [`SessionManager`] behind every RPC.
pub struct ConnectorService {
    manager: Arc<SessionManager>,
    token: Option<String>,
    version: String,
}

impl ConnectorService {
    #[must_use]
    pub fn new(token: Option<String>) -> Self {
        Self {
            manager: Arc::new(SessionManager::new()),
            token: token.filter(|value| !value.is_empty()),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// The session manager (server lifecycle: `teardown_all`).
    #[must_use]
    pub fn manager(&self) -> &Arc<SessionManager> {
        &self.manager
    }

    /// Shared-token check (spec 7.2.1): `x-impresario-token` metadata, or the
    /// handshake's `auth_token` field. Without a configured token every call
    /// passes.
    fn authorize(&self, metadata: &MetadataMap, body_token: &str) -> Result<(), Status> {
        let Some(expected) = self.token.as_deref() else {
            return Ok(());
        };
        let provided = metadata
            .get(TOKEN_KEY)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .or_else(|| {
                if body_token.is_empty() {
                    None
                } else {
                    Some(body_token.to_string())
                }
            });
        match provided {
            Some(provided) if constant_time_eq(provided.as_bytes(), expected.as_bytes()) => Ok(()),
            _ => Err(Status::unauthenticated(
                "missing or invalid impresario token (spec 7.2.1)",
            )),
        }
    }

    /// The session named by the `x-impresario-session-id` metadata.
    fn session_from_metadata(&self, metadata: &MetadataMap) -> Result<Arc<SessionHandle>, Status> {
        let session_id = metadata
            .get(SESSION_ID_KEY)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| Status::invalid_argument("missing x-impresario-session-id metadata"))?;
        self.manager
            .get(&session_id)
            .ok_or_else(|| Status::not_found(format!("unknown session '{session_id}'")))
    }

    async fn handle_handshake(&self, request: pb::HandshakeRequest) -> pb::HandshakeResponse {
        let reject = |error: ConnectorError| pb::HandshakeResponse {
            accepted: false,
            session_id: String::new(),
            agent_version: self.version.clone(),
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: Vec::new(),
            device: None,
            error: Some(error.to_proto()),
        };

        if !protocol_major_compatible(&request.protocol_version) {
            return reject(ConnectorError::protocol(format!(
                "protocol version '{}' is not served by this connector (it speaks {PROTOCOL_VERSION})",
                request.protocol_version
            )));
        }
        let platform = match platform_from_proto(request.platform) {
            Ok(platform) => platform,
            Err(error) => return reject(error),
        };
        let app_url = if request.app_url.is_empty() {
            None
        } else {
            Some(request.app_url.clone())
        };
        let config = match SessionConfig::from_handshake(
            platform,
            &request.target_endpoint,
            app_url,
            &request.options,
        ) {
            Ok(config) => config,
            Err(error) => return reject(error),
        };
        let id = self.manager.next_session_id();
        let session = match Session::connect(config, id.clone()).await {
            Ok(session) => session,
            Err(error) => return reject(error),
        };
        let device = session.device.clone();
        self.manager.insert(session);
        pb::HandshakeResponse {
            accepted: true,
            session_id: id,
            agent_version: self.version.clone(),
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: capabilities_for(platform),
            device: Some(device),
            error: None,
        }
    }
}

#[tonic::async_trait]
impl Agent for ConnectorService {
    type SessionStream = ReceiverStream<Result<pb::Event, Status>>;
    type StreamLogsStream = ReceiverStream<Result<pb::LogEntry, Status>>;

    async fn handshake(
        &self,
        request: Request<pb::HandshakeRequest>,
    ) -> Result<Response<pb::HandshakeResponse>, Status> {
        let metadata = request.metadata().clone();
        let request = request.into_inner();
        self.authorize(&metadata, &request.auth_token)?;
        Ok(Response::new(self.handle_handshake(request).await))
    }

    async fn session(
        &self,
        request: Request<tonic::Streaming<pb::Command>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        let metadata = request.metadata().clone();
        self.authorize(&metadata, "")?;
        let handle = self.session_from_metadata(&metadata)?;
        if handle.stream_taken.swap(true, Ordering::SeqCst) {
            return Err(Status::failed_precondition(
                "this session already has an open command stream",
            ));
        }
        Ok(Response::new(session::run_stream(
            handle,
            request.into_inner(),
        )))
    }

    async fn capture_screenshot(
        &self,
        request: Request<pb::ScreenshotRequest>,
    ) -> Result<Response<pb::ScreenshotResponse>, Status> {
        let metadata = request.metadata().clone();
        self.authorize(&metadata, "")?;
        let handle = self.session_from_metadata(&metadata)?;
        let response = match handle.session.capture_out_of_band().await {
            Ok((png, width, height)) => pb::ScreenshotResponse {
                png,
                width,
                height,
                error: None,
            },
            Err(error) => pb::ScreenshotResponse {
                png: Vec::new(),
                width: 0,
                height: 0,
                error: Some(error.to_proto()),
            },
        };
        Ok(Response::new(response))
    }

    async fn stream_logs(
        &self,
        request: Request<pb::LogRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        let metadata = request.metadata().clone();
        self.authorize(&metadata, "")?;
        let handle = self.session_from_metadata(&metadata)?;
        let min_rank = session::min_level_rank(request.get_ref().min_level);
        let mut console = handle.session.console_subscribe();
        let shutdown = handle.shutdown.clone();
        let (sender, receiver) = mpsc::channel::<Result<pb::LogEntry, Status>>(128);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    event = console.recv() => match event {
                        Ok(event) => {
                            if session::console_level_rank(event.level) >= min_rank {
                                let entry = session::console_to_log_entry(&event);
                                if sender.send(Ok(entry)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }

    async fn shutdown(
        &self,
        request: Request<pb::ShutdownRequest>,
    ) -> Result<Response<pb::ShutdownResponse>, Status> {
        let metadata = request.metadata().clone();
        self.authorize(&metadata, "")?;
        // Web sessions have no app to terminate (spec 6.1); the flag is
        // accepted and ignored.
        let session_id = metadata
            .get(SESSION_ID_KEY)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if let Some(handle) = self.manager.remove(&session_id) {
            handle.shutdown.cancel();
            if handle.stream_taken.load(Ordering::SeqCst) {
                handle.wait_closed(SHUTDOWN_GRACE).await;
            }
            handle.session.teardown().await;
        }
        // Unknown sessions answer ok: shutdown is idempotent (decision I-36).
        Ok(Response::new(pb::ShutdownResponse {}))
    }
}

/// Map a protobuf platform onto the core enum; only web runtimes are served.
fn platform_from_proto(value: i32) -> Result<Platform, ConnectorError> {
    match pb::Platform::try_from(value) {
        Ok(pb::Platform::Web) => Ok(Platform::Web),
        Ok(pb::Platform::Webos) => Ok(Platform::WebOs),
        Ok(pb::Platform::Tizen) => Ok(Platform::Tizen),
        Ok(other) => Err(ConnectorError::protocol(format!(
            "impresario-connector serves web platforms only (web, webos, tizen); got {other:?}"
        ))),
        Err(_) => Err(ConnectorError::protocol(format!(
            "unknown platform value {value}"
        ))),
    }
}

/// The capabilities a web session advertises (spec 6.1 + 10): everything the
/// connector supports except touch gestures (`swipe` is `UNSUPPORTED_ACTION`).
#[must_use]
pub fn capabilities_for(platform: Platform) -> Vec<i32> {
    let mut capabilities = vec![
        pb::Capability::MultiSession,
        pb::Capability::Logs,
        pb::Capability::Screenshot,
        pb::Capability::ProgrammaticFocus,
        pb::Capability::KeyEvents,
        pb::Capability::AppControl,
    ];
    if platform != Platform::Web {
        capabilities.push(pb::Capability::DpadFocus);
    }
    capabilities
        .into_iter()
        .map(|capability| capability as i32)
        .collect()
}

/// Protocol compatibility: the connector serves major version 1 only
/// (spec 7.4); `"1.0.0"` style versions with a numeric major.
#[must_use]
pub fn protocol_major_compatible(version: &str) -> bool {
    version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        == Some(1)
}

/// Constant-time byte comparison for the shared token.
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::metadata::{MetadataKey, MetadataValue};

    fn metadata(key: &'static str, value: &str) -> MetadataMap {
        let mut map = MetadataMap::new();
        let value = MetadataValue::try_from(value).expect("ascii value");
        map.insert(MetadataKey::from_static(key), value);
        map
    }

    #[test]
    fn capabilities_cover_the_web_surface() {
        let web = capabilities_for(Platform::Web);
        assert!(web.contains(&(pb::Capability::MultiSession as i32)));
        assert!(web.contains(&(pb::Capability::Logs as i32)));
        assert!(web.contains(&(pb::Capability::Screenshot as i32)));
        assert!(web.contains(&(pb::Capability::ProgrammaticFocus as i32)));
        assert!(web.contains(&(pb::Capability::KeyEvents as i32)));
        assert!(web.contains(&(pb::Capability::AppControl as i32)));
        assert!(!web.contains(&(pb::Capability::DpadFocus as i32)));
        assert!(!web.contains(&(pb::Capability::TouchGestures as i32)));

        for platform in [Platform::WebOs, Platform::Tizen] {
            let tv = capabilities_for(platform);
            assert!(
                tv.contains(&(pb::Capability::DpadFocus as i32)),
                "{platform}"
            );
        }
    }

    #[test]
    fn protocol_major_is_enforced() {
        assert!(protocol_major_compatible("1.0.0"));
        assert!(protocol_major_compatible("1.7.2"));
        assert!(!protocol_major_compatible("2.0.0"));
        assert!(!protocol_major_compatible("0.9.0"));
        assert!(!protocol_major_compatible(""));
        assert!(!protocol_major_compatible("one.zero"));
    }

    #[test]
    fn token_comparison_is_exact() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrez"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(!constant_time_eq(b"", b"secret"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn authorization_reads_metadata_or_body() {
        let service = ConnectorService::new(Some("hunter2".to_string()));
        let empty = MetadataMap::new();
        assert!(service.authorize(&empty, "hunter2").is_ok());
        assert!(service.authorize(&empty, "wrong").is_err());
        assert_eq!(
            service.authorize(&empty, "").expect_err("missing").code(),
            tonic::Code::Unauthenticated
        );
        let header = metadata(TOKEN_KEY, "hunter2");
        assert!(service.authorize(&header, "").is_ok());
        let wrong = metadata(TOKEN_KEY, "nope");
        assert!(service.authorize(&wrong, "hunter2").is_err());

        let open = ConnectorService::new(None);
        assert!(open.authorize(&empty, "").is_ok());
    }

    #[test]
    fn session_ids_are_unique() {
        let manager = SessionManager::new();
        assert_eq!(manager.next_session_id(), "session-1");
        assert_eq!(manager.next_session_id(), "session-2");
        assert_eq!(manager.session_count(), 0);
        assert!(manager.get("session-1").is_none());
        assert!(manager.remove("session-1").is_none());
    }

    #[test]
    fn platform_mapping_accepts_web_family_only() {
        assert_eq!(
            platform_from_proto(pb::Platform::Web as i32).expect("web"),
            Platform::Web
        );
        assert_eq!(
            platform_from_proto(pb::Platform::Webos as i32).expect("webos"),
            Platform::WebOs
        );
        assert_eq!(
            platform_from_proto(pb::Platform::Tizen as i32).expect("tizen"),
            Platform::Tizen
        );
        assert!(platform_from_proto(pb::Platform::Android as i32).is_err());
        assert!(platform_from_proto(999).is_err());
    }
}
