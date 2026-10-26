//! One product-level Apple TV preparation.
//!
//! This owns the whole physical path that the P0 spike proved:
//!
//! 1. pair-verify and open the outer RSD tunnel;
//! 2. mount the personalized Developer Disk Image;
//! 3. sign the host/stub/runner with the machine-global signing set;
//! 4. install the signed IPAs through AFC + InstallationProxy;
//! 5. open a second RSD tunnel and start `ImpresarioAgentTests/testServe`;
//! 6. wait until the agent's TCP port accepts.
//!
//! No host tool is spawned. The first tunnel stays alive for the life of the
//! session; the runner owns the second one, so a later clean-state reinstall can
//! put the app under test back without killing XCUITest.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::ddi::{self, DdiAssets, DdiState};
use crate::install::{self, InstallReport};
use crate::pairing::{ApplePaths, RpPairingFile};
use crate::products::ProductSet;
use crate::runner::{self, RunnerOptions, RunnerSession};
use crate::sign::{self, NestedSigning, SignRequest, SigningIdentity};
use crate::signing_set::SigningSet;
use crate::tunnel::AppleTunnel;
use crate::{Error, Result};

/// Inputs for one agent session.
pub struct SessionOptions {
    /// Device IP or DNS name on the local network.
    pub host: String,
    /// Pair-verify record (machine-global, never printed).
    pub pairing: RpPairingFile,
    /// Machine-global paths.
    pub paths: ApplePaths,
    /// Unsigned host/stub/runner bundles.
    pub products: ProductSet,
    /// Signing material for every bundle id.
    pub signing: SigningSet,
    /// Explicit DDI assets. `None` selects `apple/ddi/<build>/`.
    pub ddi: Option<DdiAssets>,
    /// Optional signed app under test (`.app` or `.ipa`) installed before the
    /// runner starts.
    pub app: Option<PathBuf>,
    /// Expected app-under-test bundle id.
    pub app_bundle_id: Option<String>,
    /// UDID to cross-check against the RSD handshake.
    pub expected_udid: Option<String>,
    /// Runner bundle id, agent port and startup timeout.
    pub runner: RunnerOptions,
}

impl std::fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionOptions")
            .field("host", &self.host)
            .field("products", &self.products.root())
            .field("signing", &self.signing.name())
            .field("ddi", &self.ddi)
            .field("app", &self.app)
            .field("app_bundle_id", &self.app_bundle_id)
            .field("expected_udid", &self.expected_udid)
            .field("runner", &self.runner)
            .finish_non_exhaustive()
    }
}

/// A live host-tunnel + XCUITest session.
pub struct PreparedDevice {
    endpoint: String,
    control: AppleTunnel,
    runner: RunnerSession,
    installs: Vec<InstallReport>,
}

impl std::fmt::Debug for PreparedDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedDevice")
            .field("endpoint", &self.endpoint)
            .field("control", &self.control)
            .field("runner", &self.runner)
            .field("installs", &self.installs)
            .finish_non_exhaustive()
    }
}

impl PreparedDevice {
    /// The agent endpoint (`host:port`).
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The long-lived control tunnel, used for clean-state reinstalls.
    pub fn control_mut(&mut self) -> &mut AppleTunnel {
        &mut self.control
    }

    /// The host/stub/app/runner installs performed by [`prepare`].
    #[must_use]
    pub fn installs(&self) -> &[InstallReport] {
        &self.installs
    }

    /// Whether XCUITest already ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.runner.is_finished()
    }

    /// Uninstall and reinstall the app under test on the long-lived control
    /// tunnel. XCUITest keeps running.
    pub async fn reset_app(&mut self, package: &Path, bundle_id: &str) -> Result<InstallReport> {
        install::reset_package(&mut self.control, package, bundle_id).await
    }

    /// Stop XCUITest and drop both tunnels.
    pub fn abort(&mut self) {
        self.runner.abort();
    }

    /// Wait for the runner to finish (usually after a master sends `Shutdown`).
    pub async fn wait(&mut self) -> Result<()> {
        self.runner.wait().await
    }
}

/// Prepare a physical Apple TV for one agent run.
pub async fn prepare(options: SessionOptions) -> Result<PreparedDevice> {
    options.validate()?;
    let mut control = AppleTunnel::connect(&options.host, &options.pairing)
        .await
        .map_err(|error| Error::Device(format!("connect to Apple TV {}: {error}", options.host)))?;
    if let Some(expected) = options.expected_udid.as_deref() {
        if let Some(actual) = control.udid() {
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(Error::Device(format!(
                    "connected to Apple TV {actual}, expected {expected}; check the device \
                     address before installing"
                )));
            }
        }
    }

    let assets = match options.ddi.clone() {
        Some(assets) => assets,
        None => {
            let build = control.build_version().ok_or_else(|| {
                Error::Device(
                    "RSD did not report BuildVersion; cannot select a Developer Disk Image".into(),
                )
            })?;
            DdiAssets::from_dir(&options.paths.ddi_build_dir(build))?
        }
    };
    let before = ddi::ensure_mounted(&mut control, &assets).await?;
    if before == DdiState::NotMounted {
        tracing::info!(device = %control.device_name(), "mounted Developer Disk Image");
    }

    let signed_root = options
        .paths
        .tmp_dir()
        .join(format!("signed-{}", Uuid::new_v4()));
    let signed_guard = RemoveOnDrop(signed_root.clone());
    let products = options.products.clone();
    let products_for_sign = products.clone();
    let signing = options.signing.clone();
    let output = signed_root.clone();
    let signed =
        tokio::task::spawn_blocking(move || sign_products(&products_for_sign, &signing, &output))
            .await
            .map_err(|error| Error::Signing(format!("signing task failed: {error}")))??;

    let mut installs = Vec::new();
    installs.push(
        install::install_bundle(&mut control, &signed.host, Some(&signed.host_bundle_id)).await?,
    );
    if let (Some(stub), Some(stub_bundle_id)) = (&signed.stub, signed.stub_bundle_id.as_deref()) {
        installs.push(install::install_bundle(&mut control, stub, Some(stub_bundle_id)).await?);
    }
    if let Some(app) = options.app.as_deref() {
        installs.push(
            install::install_package(&mut control, app, options.app_bundle_id.as_deref()).await?,
        );
    }
    installs.push(
        install::install_bundle(&mut control, &signed.runner, Some(&signed.runner_bundle_id))
            .await?,
    );
    drop(signed_guard);

    let runner_tunnel = AppleTunnel::connect(&options.host, &options.pairing)
        .await
        .map_err(|error| {
            Error::Device(format!(
                "open the XCUITest tunnel to {} after installing: {error}",
                options.host
            ))
        })?;
    let mut runner_options = options.runner;
    runner_options.runner_bundle_id = products.runner_bundle_id()?;
    runner_options.target_bundle_id = products.host_bundle_id()?;
    let runner = runner::start(runner_tunnel, runner_options).await?;
    let endpoint = runner.endpoint().to_string();
    tracing::info!(
        endpoint = %endpoint,
        device = %control.summary(),
        "Apple agent session is ready"
    );
    Ok(PreparedDevice {
        endpoint,
        control,
        runner,
        installs,
    })
}

/// Sign the imported host, stub and nested test bundles into `output`.
///
/// No device is contacted. The three destination bundles are named `host.app`,
/// `stub.app` and `runner.app`, regardless of Xcode's original names, and the
/// nested `.xctest` path inside the runner is preserved.
pub fn sign_products(
    products: &ProductSet,
    signing: &SigningSet,
    output: &Path,
) -> Result<SignedProducts> {
    std::fs::create_dir_all(output)
        .map_err(|error| Error::io(format!("create {}", output.display()), error))?;
    let identity = signing.identity()?;
    let host_bundle_id = products.host_bundle_id()?;
    let host = sign_one(
        products.host(),
        &output.join("host.app"),
        &host_bundle_id,
        signing,
        &identity,
        None,
    )?;
    let stub = products
        .stub()
        .map(|stub| {
            let bundle_id = products
                .stub_bundle_id()?
                .ok_or_else(|| Error::Device("stub bundle id is missing".into()))?;
            let signed = sign_one(
                stub,
                &output.join("stub.app"),
                &bundle_id,
                signing,
                &identity,
                None,
            )?;
            Ok::<_, Error>((signed, bundle_id))
        })
        .transpose()?;
    let runner_bundle_id = products.runner_bundle_id()?;
    let nested = match products.nested_relative_path() {
        Some(relative) => {
            let nested_bundle_id = products.nested_bundle_id()?.ok_or_else(|| {
                Error::Device("runner has a nested .xctest without a bundle id".into())
            })?;
            let materials = signing.require(&nested_bundle_id)?;
            Some((
                relative.to_string(),
                materials.profile.clone(),
                materials.entitlements.clone(),
            ))
        }
        None => None,
    };
    let runner = sign_one(
        products.runner(),
        &output.join("runner.app"),
        &runner_bundle_id,
        signing,
        &identity,
        nested,
    )?;
    Ok(SignedProducts {
        host,
        host_bundle_id,
        stub: stub.as_ref().map(|(path, _)| path.clone()),
        stub_bundle_id: stub.as_ref().map(|(_, bundle_id)| bundle_id.clone()),
        runner,
        runner_bundle_id,
    })
}

fn sign_one(
    source: &Path,
    destination: &Path,
    bundle_id: &str,
    signing: &SigningSet,
    identity: &SigningIdentity,
    nested: Option<(String, PathBuf, PathBuf)>,
) -> Result<PathBuf> {
    let materials = signing.require(bundle_id)?;
    if !materials.info.allows_bundle(bundle_id) {
        return Err(Error::Signing(format!(
            "{} does not allow bundle id {bundle_id}",
            materials.profile.display()
        )));
    }
    if let Some(team) = materials.info.team_id.as_deref() {
        if team != signing.team_id() {
            return Err(Error::Signing(format!(
                "{} belongs to team {team}, not {}",
                materials.profile.display(),
                signing.team_id()
            )));
        }
    }
    let nested = nested.map(|(relative_path, profile, entitlements)| NestedSigning {
        relative_path,
        profile,
        entitlements,
    });
    sign::sign_app(
        &SignRequest {
            source: source.to_path_buf(),
            destination: destination.to_path_buf(),
            profile: materials.profile.clone(),
            entitlements: materials.entitlements.clone(),
            nested,
        },
        identity,
        signing.team_id(),
    )?;
    Ok(destination.to_path_buf())
}

impl SessionOptions {
    fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            return Err(Error::Device("Apple device host is empty".into()));
        }
        if self.runner.agent_port == 0 {
            return Err(Error::Device("Apple agent port must not be zero".into()));
        }
        if self.runner.runner_bundle_id.trim().is_empty() {
            return Err(Error::Device("XCUITest runner bundle id is empty".into()));
        }
        if self.runner.target_bundle_id.trim().is_empty() {
            return Err(Error::Device("XCUITest target bundle id is empty".into()));
        }
        if self.app.is_some() && self.app_bundle_id.as_deref() == Some("") {
            return Err(Error::Device(
                "app bundle id is empty; omit it or pass a real value".into(),
            ));
        }
        Ok(())
    }
}

/// Paths and ids of a signed product set.
#[derive(Debug, Clone)]
pub struct SignedProducts {
    pub host: PathBuf,
    pub host_bundle_id: String,
    pub stub: Option<PathBuf>,
    pub stub_bundle_id: Option<String>,
    pub runner: PathBuf,
    pub runner_bundle_id: String,
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
