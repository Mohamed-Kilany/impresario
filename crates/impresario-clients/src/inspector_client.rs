//! Master-side gRPC client for the additive `Inspector` service
//! (TASK-INSPECTOR section 4).
//!
//! The client is deliberately tiny: the inspector process handshakes through
//! [`crate::AgentClient`] to obtain a session id, then calls the unary
//! `Snapshot`/`Input` RPCs with that session id in metadata. It never opens
//! the bidirectional `Agent.Session` stream, so a run and the inspector can
//! share the same connector session.

use std::time::Duration;

use impresario_proto::v1 as pb;
use pb::inspector_client::InspectorClient as ProtoInspector;
use tonic::metadata::{MetadataKey, MetadataMap, MetadataValue};
use tonic::transport::{Channel, Endpoint};
use tonic::Request;

use crate::agent_client::ClientError;

const SESSION_ID_KEY: &str = "x-impresario-session-id";
const TOKEN_KEY: &str = "x-impresario-token";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP2_KEEPALIVE: Duration = Duration::from_secs(10);
/// Snapshots with a screenshot can be several megabytes.
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// A connected `Inspector` service.
#[derive(Debug, Clone)]
pub struct InspectorClient {
    inner: ProtoInspector<Channel>,
    token: Option<String>,
    address: String,
}

impl InspectorClient {
    /// Connect to the same endpoint as the agent (`host:port`, or a
    /// `scheme://` URL).
    pub async fn connect(addr: &str) -> Result<Self, ClientError> {
        let uri = if addr.contains("://") {
            addr.to_string()
        } else {
            format!("http://{addr}")
        };
        let endpoint = Endpoint::from_shared(uri)
            .map_err(|error| {
                ClientError::Transport(format!("invalid inspector address '{addr}': {error}"))
            })?
            .tcp_nodelay(true)
            .connect_timeout(CONNECT_TIMEOUT)
            .http2_keep_alive_interval(HTTP2_KEEPALIVE)
            .keep_alive_timeout(HTTP2_KEEPALIVE * 2)
            .keep_alive_while_idle(true);
        let channel = endpoint.connect().await.map_err(|error| {
            ClientError::Transport(format!("cannot reach the inspector at {addr}: {error}"))
        })?;
        Ok(Self {
            inner: ProtoInspector::new(channel)
                .max_decoding_message_size(MAX_MESSAGE_SIZE)
                .max_encoding_message_size(MAX_MESSAGE_SIZE),
            token: None,
            address: addr.to_string(),
        })
    }

    /// Attach the shared executor token (spec 7.2.1).
    #[must_use]
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|value| !value.is_empty());
        self
    }

    /// The address this client dials.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// One viewport-only snapshot for `session_id`.
    pub async fn snapshot(
        &self,
        session_id: &str,
        request: pb::InspectorSnapshotRequest,
    ) -> Result<pb::InspectorSnapshotResponse, ClientError> {
        let request = request_with_session(request, self.token.as_deref(), session_id)?;
        self.inner
            .clone()
            .snapshot(request)
            .await
            .map_err(ClientError::from_status)
            .map(|response| response.into_inner())
    }

    /// One input action for `session_id` (the executor re-acquires the
    /// snapshot and returns the new revision).
    pub async fn input(
        &self,
        session_id: &str,
        request: pb::InspectorInputRequest,
    ) -> Result<pb::InspectorInputResponse, ClientError> {
        let request = request_with_session(request, self.token.as_deref(), session_id)?;
        self.inner
            .clone()
            .input(request)
            .await
            .map_err(ClientError::from_status)
            .map(|response| response.into_inner())
    }
}

fn request_with_session<T>(
    message: T,
    token: Option<&str>,
    session_id: &str,
) -> Result<Request<T>, ClientError> {
    let mut request = Request::new(message);
    insert_metadata(request.metadata_mut(), SESSION_ID_KEY, session_id)?;
    if let Some(token) = token {
        insert_metadata(request.metadata_mut(), TOKEN_KEY, token)?;
    }
    Ok(request)
}

fn insert_metadata(
    map: &mut MetadataMap,
    key: &'static str,
    value: &str,
) -> Result<(), ClientError> {
    let value = MetadataValue::try_from(value)
        .map_err(|_| ClientError::Protocol(format!("metadata '{key}' must be printable ASCII")))?;
    map.insert(MetadataKey::from_static(key), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unprintable_metadata() {
        let mut map = MetadataMap::new();
        let error = insert_metadata(&mut map, SESSION_ID_KEY, "bad\nvalue")
            .expect_err("newlines are not valid metadata");
        assert!(matches!(error, ClientError::Protocol(_)));
    }
}
