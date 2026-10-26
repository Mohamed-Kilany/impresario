//! Start Impresario Agent's XCUITest bundle over the direct outer-RSD path.
//!
//! `XCUITestService::run_with_rsd` opens `com.apple.dt.testmanagerd.remote`
//! and `com.apple.instruments.dtservicehub` on the existing RemotePairing RSD
//! tunnel; it does not create a CoreDeviceProxy. The test being launched is
//! `ImpresarioAgentTests/testServe`, which blocks until a master sends
//! `Shutdown`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use idevice::provider::IdeviceProvider;
use idevice::services::dvt::xctest::listener::{XCTestCaseResult, XCUITestListener};
use idevice::services::dvt::xctest::{TestConfig, XCUITestService};
use idevice::services::installation_proxy::InstallationProxyClient;
use idevice::{Idevice, IdeviceError};

use crate::tunnel::AppleTunnel;
use crate::{Error, Result};

/// Configuration for one agent-serving UI test.
#[derive(Debug, Clone)]
pub struct RunnerOptions {
    /// The UI-test bundle (`...agent.tests.xctrunner`).
    pub runner_bundle_id: String,
    /// The host app XCTest launches (`com.impresario.agent`).
    pub target_bundle_id: String,
    /// The agent's TCP port on the device.
    pub agent_port: u16,
    /// How long to wait for the agent port before giving up.
    pub startup_timeout: Duration,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            runner_bundle_id: "com.impresario.agent.tests.xctrunner".to_string(),
            target_bundle_id: "com.impresario.agent".to_string(),
            agent_port: 50062,
            startup_timeout: Duration::from_secs(120),
        }
    }
}

/// A live XCUITest run and its gRPC endpoint.
pub struct RunnerSession {
    endpoint: String,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl std::fmt::Debug for RunnerSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerSession")
            .field("endpoint", &self.endpoint)
            .field("finished", &self.is_finished())
            .finish()
    }
}

impl RunnerSession {
    /// The agent endpoint the master should connect to.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Whether the XCUITest run has already ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.task
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }

    /// Stop the run by closing the tunnel.
    pub fn abort(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    /// Wait for the runner and report its result.
    pub async fn wait(&mut self) -> Result<()> {
        let task = self
            .task
            .take()
            .ok_or_else(|| Error::Device("the agent UI test was already stopped".into()))?;
        task.await
            .map_err(|error| Error::Device(format!("XCUITest task: {error}")))?
    }
}

impl Drop for RunnerSession {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Start `ImpresarioAgentTests/testServe` and wait until the agent port accepts.
pub async fn start(mut tunnel: AppleTunnel, options: RunnerOptions) -> Result<RunnerSession> {
    let host = tunnel.host.clone();
    let endpoint = format!("{host}:{}", options.agent_port);
    let port = options.agent_port;
    let startup_timeout = options.startup_timeout;
    let task = tokio::spawn(async move { run(&mut tunnel, options).await });

    let started = Instant::now();
    loop {
        if task.is_finished() {
            let outcome = task.await;
            return Err(match outcome {
                Ok(Ok(())) => Error::Device(
                    "the agent UI test finished before its port accepted; Impresario Agent did \
                     not serve"
                        .into(),
                ),
                Ok(Err(error)) => error,
                Err(error) => Error::Device(format!("XCUITest task: {error}")),
            });
        }
        if tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect((host.as_str(), port)),
        )
        .await
        .is_ok_and(|result| result.is_ok())
        {
            tracing::info!(endpoint = %endpoint, "agent is listening");
            return Ok(RunnerSession {
                endpoint,
                task: Some(task),
            });
        }
        if started.elapsed() >= startup_timeout {
            task.abort();
            return Err(Error::Device(format!(
                "timed out after {}s waiting for Impresario Agent to listen on {endpoint}",
                startup_timeout.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn runner_environment(agent_port: u16) -> plist::Dictionary {
    let mut environment = plist::Dictionary::new();
    environment.insert(
        "IMPRESARIO_AGENT_PORT".to_string(),
        plist::Value::String(agent_port.to_string()),
    );
    environment
}

async fn run(tunnel: &mut AppleTunnel, options: RunnerOptions) -> Result<()> {
    let mut installation: InstallationProxyClient = tunnel
        .handshake
        .connect(&mut tunnel.handle)
        .await
        .map_err(|error| Error::Device(format!("InstallationProxy: {error}")))?;
    let mut config = TestConfig::from_installation_proxy(
        &mut installation,
        &options.runner_bundle_id,
        Some(&options.target_bundle_id),
    )
    .await
    .map_err(|error| {
        Error::Device(format!(
            "build test config for {} (target {}): {error}",
            options.runner_bundle_id, options.target_bundle_id
        ))
    })?;
    config.tests_to_run = None;
    // The Swift test bundle reads its bind port from the launch environment.
    // Without this, a non-default `agent_port` would make the host dial one
    // port while the agent listens on 50062.
    config.runner_env = Some(runner_environment(options.agent_port));
    tracing::info!(
        runner = %config.runner_bundle_id,
        executable = %config.runner_bundle_executable,
        target = ?config.target_bundle_id,
        "starting the agent UI test"
    );

    let service = XCUITestService::new(Arc::new(NoProvider));
    let mut listener = Events;
    let major = tunnel.os_major_version();
    service
        .run_with_rsd(
            config,
            &mut listener,
            None,
            &mut tunnel.handle,
            &tunnel.handshake,
            major,
        )
        .await
        .map_err(|error| Error::Device(format!("XCUITest run: {error}")))
}

/// `run_with_rsd` does not use a provider; the stock service still requires one.
#[derive(Debug)]
struct NoProvider;

impl IdeviceProvider for NoProvider {
    fn connect(
        &self,
        _port: u16,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Idevice, IdeviceError>> + Send>> {
        Box::pin(async {
            Err(IdeviceError::UnknownErrorType(
                "the direct RSD XCUITest path does not use an IdeviceProvider".into(),
            ))
        })
    }

    fn label(&self) -> &str {
        "impresario-rsd"
    }

    fn get_pairing_file(
        &self,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = std::result::Result<idevice::pairing_file::PairingFile, IdeviceError>,
                > + Send,
        >,
    > {
        Box::pin(async {
            Err(IdeviceError::UnknownErrorType(
                "the direct RSD XCUITest path does not use a pairing file".into(),
            ))
        })
    }
}

struct Events;

impl XCUITestListener for Events {
    async fn did_begin_executing_test_plan(&mut self) -> std::result::Result<(), IdeviceError> {
        tracing::info!("XCUITest plan started");
        Ok(())
    }

    async fn did_finish_executing_test_plan(&mut self) -> std::result::Result<(), IdeviceError> {
        tracing::info!("XCUITest plan finished");
        Ok(())
    }

    async fn test_bundle_ready(&mut self) -> std::result::Result<(), IdeviceError> {
        tracing::debug!("XCUITest bundle ready");
        Ok(())
    }

    async fn test_bundle_ready_with_protocol_version(
        &mut self,
        protocol_version: u64,
        minimum_version: u64,
    ) -> std::result::Result<(), IdeviceError> {
        tracing::debug!(protocol_version, minimum_version, "XCUITest bundle ready");
        Ok(())
    }

    async fn test_runner_ready_with_capabilities(
        &mut self,
    ) -> std::result::Result<(), IdeviceError> {
        tracing::debug!("XCUITest runner ready with capabilities");
        Ok(())
    }

    async fn test_case_did_start(
        &mut self,
        test_class: &str,
        method: &str,
    ) -> std::result::Result<(), IdeviceError> {
        tracing::info!(test_class, method, "XCUITest case started");
        Ok(())
    }

    async fn test_case_did_finish(
        &mut self,
        result: XCTestCaseResult,
    ) -> std::result::Result<(), IdeviceError> {
        if result.status == "passed" {
            tracing::info!(
                test_class = %result.test_class,
                method = %result.method,
                duration = result.duration,
                "XCUITest case passed"
            );
        } else {
            tracing::warn!(
                test_class = %result.test_class,
                method = %result.method,
                status = %result.status,
                duration = result.duration,
                "XCUITest case did not pass"
            );
        }
        Ok(())
    }

    async fn test_case_did_fail(
        &mut self,
        test_class: &str,
        method: &str,
        message: &str,
        file: &str,
        line: u64,
    ) -> std::result::Result<(), IdeviceError> {
        tracing::error!(
            test_class,
            method,
            file,
            line,
            message,
            "XCUITest case failed"
        );
        Ok(())
    }

    async fn log_message(&mut self, message: &str) -> std::result::Result<(), IdeviceError> {
        tracing::debug!(message, "XCUITest");
        Ok(())
    }

    async fn log_debug_message(&mut self, message: &str) -> std::result::Result<(), IdeviceError> {
        tracing::trace!(message, "XCUITest debug");
        Ok(())
    }

    async fn did_begin_initializing_for_ui_testing(
        &mut self,
    ) -> std::result::Result<(), IdeviceError> {
        tracing::info!("XCUITest UI initialization started");
        Ok(())
    }

    async fn initialization_for_ui_testing_did_fail(
        &mut self,
        description: &str,
    ) -> std::result::Result<(), IdeviceError> {
        tracing::error!(description, "XCUITest UI initialization failed");
        Err(IdeviceError::UnexpectedResponse(format!(
            "XCUITest UI initialization failed: {description}"
        )))
    }

    async fn did_fail_to_bootstrap(
        &mut self,
        description: &str,
    ) -> std::result::Result<(), IdeviceError> {
        tracing::error!(description, "XCUITest bootstrap failed");
        Err(IdeviceError::UnexpectedResponse(format!(
            "XCUITest bootstrap failed: {description}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::runner_environment;

    #[test]
    fn runner_environment_sets_the_agent_port() {
        let environment = runner_environment(50123);
        assert_eq!(
            environment
                .get("IMPRESARIO_AGENT_PORT")
                .and_then(plist::Value::as_string),
            Some("50123")
        );
    }
}
