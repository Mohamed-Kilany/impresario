//! Pure-Rust Apple development provisioning.
//!
//! The Apple ID session logs in through `impresario-apple-auth`; this module
//! turns that session into the files a physical device needs:
//!
//! 1. register the device UDID with the developer team,
//! 2. register the agent's App IDs,
//! 3. create or reuse an Apple development certificate and its private key,
//! 4. build a PKCS#12 identity,
//! 5. download and validate one development profile per bundle, and
//! 6. install the result as a normal Impresario signing set.
//!
//! No host tool (`openssl`, `xcrun`, `security`, Python) is involved. The
//! certificate key is RSA-2048 and the PKCS#12 is written by `p12-keystore`.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use impresario_apple_auth::{
    dev::{
        app_ids::{AppId, AppIdsApi, Profile as ApiProfile},
        certificates::{CertificatesApi, DevelopmentCertificate},
        developer_session::DeveloperSession,
        device_type::DeveloperDeviceType,
        devices::DevicesApi,
        teams::DeveloperTeam,
    },
    SideloadError, APPLE_ROOT, APPLE_WWDR_G3,
};
use p12_keystore::{
    Certificate as P12Certificate, KeyStore, KeyStoreEntry, LocalKeyId, Pkcs12ImportPolicy,
    PrivateKey, PrivateKeyChain,
};
use plist::{Dictionary, Value};
use rcgen::{CertificateParams, DnType, KeyPair, PKCS_RSA_SHA256};
use rsa::pkcs1::EncodeRsaPublicKey;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::RsaPrivateKey;
use uuid::Uuid;
use x509_certificate::CapturedX509Certificate;
use zeroize::Zeroizing;

use crate::signing_set::{safe_key, SigningImportReport, SigningSet};

/// Which Apple platform endpoint and profile platform to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionPlatform {
    Tvos,
    Ios,
}

impl ProvisionPlatform {
    fn developer_device_type(self) -> DeveloperDeviceType {
        match self {
            Self::Tvos => DeveloperDeviceType::Tvos,
            Self::Ios => DeveloperDeviceType::Ios,
        }
    }

    /// The value Apple places in a profile's `Platform` array.
    #[must_use]
    pub fn profile_label(self) -> &'static str {
        match self {
            Self::Tvos => "tvOS",
            Self::Ios => "iOS",
        }
    }

    /// A human-readable platform name.
    #[must_use]
    pub fn label(self) -> &'static str {
        self.profile_label()
    }
}

/// A provisioning request. It contains no private key or password.
#[derive(Debug, Clone)]
pub struct ProvisionRequest {
    pub platform: ProvisionPlatform,
    pub machine_name: String,
    pub device_name: String,
    pub udid: String,
    pub bundle_ids: Vec<String>,
    pub revoke_serials: Vec<String>,
}

/// A development certificate and key loaded from an existing PKCS#12.
///
/// The key and password stay private to this module; callers only construct
/// this value to reuse an existing identity.
pub struct ExistingIdentity {
    p12: Zeroizing<Vec<u8>>,
    password: Zeroizing<String>,
    private_key: RsaPrivateKey,
    leaf_certificate_der: Vec<u8>,
}

impl std::fmt::Debug for ExistingIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExistingIdentity")
            .field("p12_bytes", &self.p12.len())
            .field("certificate_bytes", &self.leaf_certificate_der.len())
            .finish_non_exhaustive()
    }
}

impl ExistingIdentity {
    /// Load a PKCS#12 identity for certificate reuse.
    pub fn from_pkcs12(p12: Vec<u8>, password: Zeroizing<String>) -> Result<Self, ProvisionError> {
        if p12.is_empty() {
            return Err(ProvisionError::Invalid(
                "the existing PKCS#12 is empty".to_string(),
            ));
        }
        let store = KeyStore::from_pkcs12(&p12, &password, Pkcs12ImportPolicy::Strict)
            .map_err(|error| ProvisionError::Local(format!("parse existing PKCS#12: {error}")))?;
        let (_, chain) = store.private_key_chain().ok_or_else(|| {
            ProvisionError::Local("the existing PKCS#12 has no private key chain".to_string())
        })?;
        let private_key = RsaPrivateKey::from_pkcs8_der(chain.key().as_der()).map_err(|error| {
            ProvisionError::Local(format!(
                "the existing PKCS#12 does not contain an RSA PKCS#8 development key: {error}"
            ))
        })?;
        let leaf = chain.certs().first().ok_or_else(|| {
            ProvisionError::Local("the existing PKCS#12 has no leaf certificate".to_string())
        })?;
        let leaf_certificate_der = leaf.as_der().to_vec();
        if !certificate_der_matches_key(&leaf_certificate_der, &private_key) {
            return Err(ProvisionError::Local(
                "the existing PKCS#12 leaf certificate does not match its private key".to_string(),
            ));
        }
        Ok(Self {
            p12: Zeroizing::new(p12),
            password,
            private_key,
            leaf_certificate_der,
        })
    }

    /// The existing PKCS#12 bytes, for diagnostics only. The value is not
    /// printed by `Debug`.
    #[must_use]
    pub fn p12_len(&self) -> usize {
        self.p12.len()
    }
}

/// The generated or reused signing identity.
///
/// The private key itself is not stored here after the PKCS#12 has been made.
pub struct IdentityMaterial {
    pub certificate_serial: Option<String>,
    pub certificate_name: Option<String>,
    pub reused_certificate: bool,
    pub reused_key: bool,
    p12: Zeroizing<Vec<u8>>,
    password: Zeroizing<String>,
}

impl std::fmt::Debug for IdentityMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityMaterial")
            .field("certificate_serial", &self.certificate_serial)
            .field("certificate_name", &self.certificate_name)
            .field("reused_certificate", &self.reused_certificate)
            .field("reused_key", &self.reused_key)
            .field("p12_bytes", &self.p12.len())
            .finish_non_exhaustive()
    }
}

/// One downloaded and validated development profile.
#[derive(Clone)]
pub struct ProvisionedBundle {
    pub bundle_id: String,
    pub profile_name: String,
    pub profile_uuid: String,
    pub expiration: Option<String>,
    pub entitlements: Dictionary,
    profile: Vec<u8>,
}

impl std::fmt::Debug for ProvisionedBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionedBundle")
            .field("bundle_id", &self.bundle_id)
            .field("profile_name", &self.profile_name)
            .field("profile_uuid", &self.profile_uuid)
            .field("expiration", &self.expiration)
            .field(
                "entitlements",
                &self.entitlements.keys().collect::<Vec<_>>(),
            )
            .field("profile_bytes", &self.profile.len())
            .finish_non_exhaustive()
    }
}

/// Everything created by one provisioning run.
pub struct ProvisionReport {
    pub team_id: String,
    pub platform: ProvisionPlatform,
    pub device_udid: String,
    pub device_name: String,
    pub app_ids_created: Vec<String>,
    pub app_ids_existing: Vec<String>,
    pub identity: IdentityMaterial,
    pub bundles: Vec<ProvisionedBundle>,
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for ProvisionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionReport")
            .field("team_id", &self.team_id)
            .field("platform", &self.platform)
            .field("device_udid", &self.device_udid)
            .field("device_name", &self.device_name)
            .field("app_ids_created", &self.app_ids_created)
            .field("app_ids_existing", &self.app_ids_existing)
            .field("identity", &self.identity)
            .field("bundles", &self.bundles)
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// A certificate that can be revoked to free one of Apple's limited slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateSummary {
    pub serial_number: String,
    pub name: Option<String>,
    pub machine_name: Option<String>,
    pub expiration: Option<String>,
    pub status: Option<String>,
}

/// List the team's development certificates for one platform.
///
/// This is the read-only half of certificate management: `signing provision`
/// uses the same list internally before it creates or reuses an identity.
pub async fn list_certificates(
    session: &mut DeveloperSession,
    team: &DeveloperTeam,
    platform: ProvisionPlatform,
) -> Result<Vec<CertificateSummary>, ProvisionError> {
    let certificates = session
        .list_all_development_certs(team, platform.developer_device_type())
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!("could not list the team's certificates: {error}"))
        })?;
    Ok(certificates.iter().map(certificate_summary).collect())
}

/// Provisioning failed.
#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    /// The request or a local signing material is invalid.
    #[error("{0}")]
    Invalid(String),
    /// Apple returned an error or an unexpected response.
    #[error("{0}")]
    Apple(String),
    /// The account's development certificate limit is full. Revoke one of
    /// these serials and retry.
    #[error(
        "the Apple development certificate limit is reached; revoke one of the listed certificates"
    )]
    CertificatesFull(Vec<CertificateSummary>),
    /// A local file, PKCS#12 operation or profile parse failed.
    #[error("{0}")]
    Local(String),
}

impl ProvisionError {
    /// The certificates to show when Apple refuses a new certificate.
    #[must_use]
    pub fn certificates(&self) -> Option<&[CertificateSummary]> {
        match self {
            Self::CertificatesFull(certificates) => Some(certificates),
            Self::Invalid(_) | Self::Apple(_) | Self::Local(_) => None,
        }
    }
}

/// Provision an Apple development identity and profiles.
///
/// `existing` is reused when its public key already has an active development
/// certificate. When it has no matching certificate, the same key is submitted
/// for a new certificate; when `existing` is `None`, a new RSA key is made.
pub async fn provision(
    session: &mut DeveloperSession,
    team: &DeveloperTeam,
    request: &ProvisionRequest,
    existing: Option<&ExistingIdentity>,
) -> Result<ProvisionReport, ProvisionError> {
    validate_request(request)?;
    let device_type = request.platform.developer_device_type();

    session
        .ensure_device_registered(
            team,
            &request.device_name,
            &request.udid,
            request.platform.developer_device_type(),
        )
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!(
                "could not register device {} with team {}: {error}",
                request.udid, team.team_id
            ))
        })?;

    let (app_ids, app_ids_created, app_ids_existing) =
        ensure_app_ids(session, team, device_type.clone(), &request.bundle_ids).await?;

    for serial in &request.revoke_serials {
        session
            .revoke_development_cert(team, serial, device_type.clone())
            .await
            .map_err(|error| {
                ProvisionError::Apple(format!("could not revoke certificate {serial}: {error}"))
            })?;
    }

    let identity = ensure_identity(session, team, request, existing, device_type.clone()).await?;

    let mut bundles = Vec::with_capacity(request.bundle_ids.len());
    let mut warnings = Vec::new();
    for bundle_id in &request.bundle_ids {
        let app_id = app_ids.get(bundle_id).ok_or_else(|| {
            ProvisionError::Apple(format!("Apple did not return the App ID for {bundle_id}"))
        })?;
        let profile = download_profile(session, team, app_id, device_type.clone()).await?;
        bundles.push(validate_profile(
            request,
            team,
            bundle_id,
            &profile,
            &mut warnings,
        )?);
    }

    Ok(ProvisionReport {
        team_id: team.team_id.clone(),
        platform: request.platform,
        device_udid: request.udid.clone(),
        device_name: request.device_name.clone(),
        app_ids_created,
        app_ids_existing,
        identity,
        bundles,
        warnings,
    })
}

/// Write a [`ProvisionReport`] into the machine-global signing store.
///
/// The source tree is temporary; only the validated signing set is retained.
pub fn install(
    report: &ProvisionReport,
    signing_root: &Path,
    name: &str,
) -> Result<SigningImportReport, ProvisionError> {
    std::fs::create_dir_all(signing_root).map_err(|error| {
        ProvisionError::Local(format!("create {}: {error}", signing_root.display()))
    })?;
    let source = signing_root.join(format!(".provision-{}", Uuid::new_v4()));
    let guard = RemoveOnDrop(source.clone());
    std::fs::create_dir_all(source.join("bundles"))
        .map_err(|error| ProvisionError::Local(format!("create provisioning tree: {error}")))?;
    std::fs::write(source.join("team-id"), format!("{}\n", report.team_id))
        .map_err(|error| ProvisionError::Local(format!("write team-id: {error}")))?;
    std::fs::write(source.join("identity.p12"), &*report.identity.p12)
        .map_err(|error| ProvisionError::Local(format!("write identity.p12: {error}")))?;
    for bundle in &report.bundles {
        let directory = source.join("bundles").join(safe_key(&bundle.bundle_id));
        std::fs::create_dir_all(&directory).map_err(|error| {
            ProvisionError::Local(format!("create {}: {error}", directory.display()))
        })?;
        std::fs::write(directory.join("profile.mobileprovision"), &bundle.profile).map_err(
            |error| {
                ProvisionError::Local(format!("write profile for {}: {error}", bundle.bundle_id))
            },
        )?;
        let mut entitlements = Vec::new();
        Value::Dictionary(bundle.entitlements.clone())
            .to_writer_xml(&mut entitlements)
            .map_err(|error| {
                ProvisionError::Local(format!(
                    "write entitlements for {}: {error}",
                    bundle.bundle_id
                ))
            })?;
        std::fs::write(directory.join("entitlements.plist"), entitlements).map_err(|error| {
            ProvisionError::Local(format!(
                "write entitlements for {}: {error}",
                bundle.bundle_id
            ))
        })?;
    }

    let result = SigningSet::import(&source, signing_root, name, &report.identity.password)
        .map_err(|error| ProvisionError::Local(error.to_string()));
    drop(guard);
    result
}

fn validate_request(request: &ProvisionRequest) -> Result<(), ProvisionError> {
    if request.udid.trim().is_empty() {
        return Err(ProvisionError::Invalid(
            "the device UDID is empty; pass --udid or store exactly one pairing".to_string(),
        ));
    }
    if request.machine_name.trim().is_empty() {
        return Err(ProvisionError::Invalid(
            "the certificate machine name is empty".to_string(),
        ));
    }
    if request.device_name.trim().is_empty() {
        return Err(ProvisionError::Invalid(
            "the device name is empty".to_string(),
        ));
    }
    if request.bundle_ids.is_empty() {
        return Err(ProvisionError::Invalid(
            "no bundle ids were requested".to_string(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for bundle_id in &request.bundle_ids {
        if !bundle_id.contains('.') || bundle_id.chars().any(char::is_whitespace) {
            return Err(ProvisionError::Invalid(format!(
                "'{bundle_id}' is not a valid bundle id"
            )));
        }
        if !seen.insert(bundle_id.as_str()) {
            return Err(ProvisionError::Invalid(format!(
                "bundle id {bundle_id} was requested twice"
            )));
        }
    }
    Ok(())
}

async fn ensure_app_ids(
    session: &mut DeveloperSession,
    team: &DeveloperTeam,
    device_type: DeveloperDeviceType,
    bundle_ids: &[String],
) -> Result<
    (
        std::collections::BTreeMap<String, AppId>,
        Vec<String>,
        Vec<String>,
    ),
    ProvisionError,
> {
    let response = session
        .list_app_ids(team, device_type.clone())
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!("could not list the team's App IDs: {error}"))
        })?;
    let mut by_identifier = response
        .app_ids
        .into_iter()
        .map(|app_id| (app_id.identifier.clone(), app_id))
        .collect::<std::collections::BTreeMap<_, _>>();
    let missing = bundle_ids
        .iter()
        .filter(|bundle_id| !by_identifier.contains_key(*bundle_id))
        .count();
    if missing > 0
        && response
            .available_quantity
            .is_some_and(|available| available < missing as i64)
    {
        return Err(ProvisionError::Apple(format!(
            "the team has no App ID slots left for {missing} new bundle(s); remove old App IDs in the Apple Developer account and retry"
        )));
    }

    let mut created = Vec::new();
    let mut existing = Vec::new();
    for bundle_id in bundle_ids {
        if by_identifier.contains_key(bundle_id) {
            existing.push(bundle_id.clone());
            continue;
        }
        let name = app_id_name(bundle_id);
        let app_id = match session
            .add_app_id(team, &name, bundle_id, device_type.clone())
            .await
        {
            Ok(app_id) => app_id,
            Err(error) if is_developer_error(&error, 35) => {
                // Apple sometimes reports a duplicate while the list endpoint
                // has not caught up yet. Re-list once before failing.
                let refreshed = session
                    .list_app_ids(team, device_type.clone())
                    .await
                    .map_err(|list_error| {
                        ProvisionError::Apple(format!(
                            "could not add App ID {bundle_id} ({error}); re-list failed: {list_error}"
                        ))
                    })?;
                refreshed
                    .app_ids
                    .into_iter()
                    .find(|candidate| candidate.identifier == *bundle_id)
                    .ok_or_else(|| {
                        ProvisionError::Apple(format!(
                            "Apple rejected App ID {bundle_id} as a duplicate but did not return it"
                        ))
                    })?
            }
            Err(error) => {
                return Err(ProvisionError::Apple(format!(
                    "could not add App ID {bundle_id}: {error}"
                )));
            }
        };
        created.push(bundle_id.clone());
        by_identifier.insert(bundle_id.clone(), app_id);
    }

    Ok((by_identifier, created, existing))
}

fn app_id_name(bundle_id: &str) -> String {
    let normalized =
        impresario_apple_auth::dev::normalize_app_names(&format!("Impresario {bundle_id}"));
    normalized.chars().take(48).collect()
}

async fn ensure_identity(
    session: &mut DeveloperSession,
    team: &DeveloperTeam,
    request: &ProvisionRequest,
    existing: Option<&ExistingIdentity>,
    device_type: DeveloperDeviceType,
) -> Result<IdentityMaterial, ProvisionError> {
    let mut certificates = session
        .list_all_development_certs(team, device_type.clone())
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!("could not list the team's certificates: {error}"))
        })?;

    let (private_key, reuse_key, existing_match) = if let Some(identity) = existing {
        let matching = certificates.iter().find(|certificate| {
            certificate_is_active(certificate)
                && certificate_matches_key(certificate, &identity.private_key)
        });
        (
            identity.private_key.clone(),
            true,
            matching.map(|certificate| certificate_summary(certificate).serial_number),
        )
    } else {
        (
            generate_key().map_err(|error| {
                ProvisionError::Local(format!("could not generate an RSA key: {error}"))
            })?,
            false,
            None,
        )
    };

    if existing_match.is_some() {
        let identity = existing.ok_or_else(|| {
            ProvisionError::Local("internal error: reused a missing identity".to_string())
        })?;
        let certificate = certificates
            .iter()
            .find(|certificate| {
                certificate_is_active(certificate)
                    && certificate_matches_key(certificate, &private_key)
            })
            .ok_or_else(|| {
                ProvisionError::Local("internal error: certificate disappeared".to_string())
            })?;
        let certificate_der = certificate_der(certificate)?;
        let (p12, password) = if certificate_der == identity.leaf_certificate_der {
            (identity.p12.clone(), identity.password.clone())
        } else {
            create_pkcs12(&private_key, &certificate_der, identity.password.clone())?
        };
        return Ok(IdentityMaterial {
            certificate_serial: certificate.serial_number.clone(),
            certificate_name: certificate.name.clone(),
            reused_certificate: true,
            reused_key: reuse_key,
            p12,
            password,
        });
    }

    let csr = certificate_signing_request(&private_key, &request.machine_name)
        .map_err(|error| ProvisionError::Local(format!("could not build the CSR: {error}")))?;
    let cert_request = match session
        .submit_development_csr(team, csr, request.machine_name.clone(), device_type.clone())
        .await
    {
        Ok(request_id) => request_id,
        Err(error) if is_developer_error(&error, 7460) => {
            return Err(ProvisionError::CertificatesFull(
                certificates.iter().map(certificate_summary).collect(),
            ));
        }
        Err(error) => {
            return Err(ProvisionError::Apple(format!(
                "could not submit the development certificate request: {error}"
            )));
        }
    };

    // Apple can take a moment to expose a newly issued certificate.
    let mut issued: Option<DevelopmentCertificate> = None;
    for attempt in 0..10 {
        certificates = session
            .list_all_development_certs(team, device_type.clone())
            .await
            .map_err(|error| {
                ProvisionError::Apple(format!(
                    "could not list certificates after submitting the CSR: {error}"
                ))
            })?;
        issued = certificates
            .iter()
            .find(|certificate| {
                certificate.certificate_id.as_deref() == Some(cert_request.cert_request_id.as_str())
                    || certificate_matches_key(certificate, &private_key)
            })
            .cloned();
        if issued.is_some() {
            break;
        }
        if attempt < 9 {
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
    }
    let certificate = issued.ok_or_else(|| {
        ProvisionError::Apple(
            "Apple accepted the certificate request but did not return the certificate; retry and select the existing signing identity if it appears"
                .to_string(),
        )
    })?;
    let certificate_der = certificate_der(&certificate)?;
    let password = existing.map_or_else(new_password, |identity| identity.password.clone());
    let (p12, password) = create_pkcs12(&private_key, &certificate_der, password)?;
    Ok(IdentityMaterial {
        certificate_serial: certificate.serial_number.clone(),
        certificate_name: certificate.name.clone(),
        reused_certificate: false,
        reused_key: reuse_key,
        p12,
        password,
    })
}

async fn download_profile(
    session: &mut DeveloperSession,
    team: &DeveloperTeam,
    app_id: &AppId,
    device_type: DeveloperDeviceType,
) -> Result<ApiProfile, ProvisionError> {
    let mut last_error = None;
    for attempt in 0..3 {
        match session
            .download_team_provisioning_profile(team, app_id, device_type.clone())
            .await
        {
            Ok(profile) => return Ok(profile),
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
    Err(ProvisionError::Apple(format!(
        "could not download the provisioning profile for {}: {}",
        app_id.identifier,
        last_error.map_or_else(
            || "unknown Apple error".to_string(),
            |error| error.to_string()
        )
    )))
}

fn validate_profile(
    request: &ProvisionRequest,
    team: &DeveloperTeam,
    bundle_id: &str,
    profile: &ApiProfile,
    warnings: &mut Vec<String>,
) -> Result<ProvisionedBundle, ProvisionError> {
    if profile.encoded_profile.as_ref().is_empty() {
        return Err(ProvisionError::Apple(format!(
            "Apple returned an empty profile for {bundle_id}"
        )));
    }
    let plist_bytes = crate::signing_set::profile_plist_bytes(profile.encoded_profile.as_ref())
        .map_err(|error| {
            ProvisionError::Apple(format!(
                "could not decode the profile for {bundle_id}: {error}"
            ))
        })?;
    let value = Value::from_reader(Cursor::new(plist_bytes)).map_err(|error| {
        ProvisionError::Apple(format!(
            "could not parse the profile for {bundle_id}: {error}"
        ))
    })?;
    let dictionary = value.as_dictionary().ok_or_else(|| {
        ProvisionError::Apple(format!(
            "the profile for {bundle_id} is not a plist dictionary"
        ))
    })?;
    let entitlements = dictionary
        .get("Entitlements")
        .and_then(Value::as_dictionary)
        .cloned()
        .ok_or_else(|| {
            ProvisionError::Apple(format!("the profile for {bundle_id} has no Entitlements"))
        })?;

    let application_identifier = entitlements
        .get("application-identifier")
        .and_then(Value::as_string)
        .ok_or_else(|| {
            ProvisionError::Apple(format!(
                "the profile for {bundle_id} has no application-identifier"
            ))
        })?;
    let expected_identifier = format!("{}.{}", team.team_id, bundle_id);
    if application_identifier != expected_identifier {
        return Err(ProvisionError::Apple(format!(
            "the profile for {bundle_id} targets {application_identifier}, expected {expected_identifier}"
        )));
    }

    let profile_team = dictionary
        .get("TeamIdentifier")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_string)
        .or_else(|| {
            entitlements
                .get("com.apple.developer.team-identifier")
                .and_then(Value::as_string)
        });
    if profile_team != Some(team.team_id.as_str()) {
        return Err(ProvisionError::Apple(format!(
            "the profile for {bundle_id} belongs to team {}, expected {}",
            profile_team.unwrap_or("<none>"),
            team.team_id
        )));
    }

    let devices = string_array(dictionary, "ProvisionedDevices");
    if devices.is_empty() {
        return Err(ProvisionError::Apple(format!(
            "the profile for {bundle_id} has no ProvisionedDevices; development profiles must name the device"
        )));
    }
    if !devices
        .iter()
        .any(|device| device.eq_ignore_ascii_case(&request.udid))
    {
        return Err(ProvisionError::Apple(format!(
            "the profile for {bundle_id} does not include device {}",
            request.udid
        )));
    }

    let platforms = string_array(dictionary, "Platform");
    if !platforms.is_empty()
        && !platforms
            .iter()
            .any(|platform| platform.eq_ignore_ascii_case(request.platform.profile_label()))
    {
        return Err(ProvisionError::Apple(format!(
            "the profile for {bundle_id} is for {}, expected {}",
            platforms.join(", "),
            request.platform.profile_label()
        )));
    }
    if platforms.is_empty() {
        warnings.push(format!(
            "the profile for {bundle_id} has no Platform field; continuing because Apple omitted it"
        ));
    }

    let expiration = dictionary
        .get("ExpirationDate")
        .and_then(Value::as_date)
        .map(|date| (date.to_xml_format(), SystemTime::from(date)))
        .or_else(|| {
            Some((
                profile.date_expire.to_xml_format(),
                SystemTime::from(profile.date_expire),
            ))
        });
    if let Some((_, expires)) = &expiration {
        if *expires <= SystemTime::now() {
            return Err(ProvisionError::Apple(format!(
                "the profile for {bundle_id} expired"
            )));
        }
    } else {
        warnings.push(format!(
            "the profile for {bundle_id} has no expiration date"
        ));
    }

    if entitlements
        .get("get-task-allow")
        .and_then(Value::as_boolean)
        == Some(false)
    {
        return Err(ProvisionError::Apple(format!(
            "the profile for {bundle_id} is not a development profile (get-task-allow is false)"
        )));
    }
    if entitlements
        .get("get-task-allow")
        .and_then(Value::as_boolean)
        .is_none()
    {
        warnings.push(format!(
            "the profile for {bundle_id} does not set get-task-allow"
        ));
    }

    Ok(ProvisionedBundle {
        bundle_id: bundle_id.to_string(),
        profile_name: dictionary
            .get("Name")
            .and_then(Value::as_string)
            .unwrap_or(&profile.name)
            .to_string(),
        profile_uuid: dictionary
            .get("UUID")
            .and_then(Value::as_string)
            .unwrap_or(&profile.uuid)
            .to_string(),
        expiration: expiration.map(|(date, _)| date),
        entitlements,
        profile: profile.encoded_profile.as_ref().to_vec(),
    })
}

fn string_array(dictionary: &Dictionary, key: &str) -> Vec<String> {
    dictionary
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_string)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn certificate_der(certificate: &DevelopmentCertificate) -> Result<Vec<u8>, ProvisionError> {
    certificate
        .cert_content
        .as_ref()
        .map(|content| content.as_ref().to_vec())
        .ok_or_else(|| {
            ProvisionError::Apple(format!(
                "Apple returned certificate {} without content",
                certificate
                    .serial_number
                    .as_deref()
                    .unwrap_or("<unknown serial>")
            ))
        })
}

fn certificate_matches_key(certificate: &DevelopmentCertificate, key: &RsaPrivateKey) -> bool {
    let Some(content) = &certificate.cert_content else {
        return false;
    };
    certificate_der_matches_key(content.as_ref(), key)
}

fn certificate_der_matches_key(der: &[u8], key: &RsaPrivateKey) -> bool {
    let Ok(certificate) = CapturedX509Certificate::from_der(der) else {
        return false;
    };
    let Ok(public_key) = key.to_public_key().to_pkcs1_der() else {
        return false;
    };
    certificate.public_key_data().as_ref() == public_key.as_bytes()
}

fn certificate_is_active(certificate: &DevelopmentCertificate) -> bool {
    let not_expired = certificate
        .expiration_date
        .is_none_or(|date| SystemTime::from(date) > SystemTime::now());
    let not_revoked = certificate.status.as_deref().is_none_or(|status| {
        !matches!(
            status.to_ascii_lowercase().as_str(),
            "revoked" | "expired" | "invalid"
        )
    });
    not_expired && not_revoked
}

fn certificate_summary(certificate: &DevelopmentCertificate) -> CertificateSummary {
    CertificateSummary {
        serial_number: certificate
            .serial_number
            .clone()
            .unwrap_or_else(|| "<missing serial>".to_string()),
        name: certificate.name.clone(),
        machine_name: certificate.machine_name.clone(),
        expiration: certificate.expiration_date.map(|date| date.to_xml_format()),
        status: certificate.status.clone(),
    }
}

fn is_developer_error(error: &rootcause::Report, code: i64) -> bool {
    error.iter_reports().any(|cause| {
        matches!(
            cause.downcast_current_context::<SideloadError>(),
            Some(SideloadError::DeveloperError(found, _)) if *found == code
        )
    })
}

fn generate_key() -> Result<RsaPrivateKey, rsa::Error> {
    let mut rng = rsa::rand_core::OsRng;
    RsaPrivateKey::new(&mut rng, 2048)
}

fn certificate_signing_request(
    key: &RsaPrivateKey,
    machine_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let private_key_pem = key.to_pkcs8_pem(LineEnding::LF)?;
    let key_pair = KeyPair::from_pkcs8_pem_and_sign_algo(&private_key_pem, &PKCS_RSA_SHA256)?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, machine_name);
    Ok(params.serialize_request(&key_pair)?.pem()?)
}

fn create_pkcs12(
    key: &RsaPrivateKey,
    leaf_certificate: &[u8],
    password: Zeroizing<String>,
) -> Result<(Zeroizing<Vec<u8>>, Zeroizing<String>), ProvisionError> {
    let key_der = key
        .to_pkcs8_der()
        .map_err(|error| ProvisionError::Local(format!("encode the private key: {error}")))?;
    let private_key = PrivateKey::from_der(key_der.as_bytes())
        .map_err(|error| ProvisionError::Local(format!("load the private key: {error}")))?;
    let leaf = P12Certificate::from_der(leaf_certificate)
        .map_err(|error| ProvisionError::Local(format!("load the leaf certificate: {error}")))?;
    let mut certificates = vec![leaf];
    for ca in [APPLE_WWDR_G3, APPLE_ROOT] {
        certificates.push(
            P12Certificate::from_der(ca)
                .map_err(|error| ProvisionError::Local(format!("load the Apple CA: {error}")))?,
        );
    }
    let chain = PrivateKeyChain::new(
        LocalKeyId::from(Uuid::new_v4().as_bytes().to_vec()),
        private_key,
        certificates,
    );
    let mut store = KeyStore::new();
    store.add_entry(
        "Impresario Development",
        KeyStoreEntry::PrivateKeyChain(chain),
    );
    let bytes = store
        .writer(&password)
        .write()
        .map_err(|error| ProvisionError::Local(format!("write PKCS#12: {error}")))?;
    Ok((Zeroizing::new(bytes), password))
}

fn new_password() -> Zeroizing<String> {
    let mut bytes = [0u8; 32];
    use rand::RngCore as _;
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    Zeroizing::new(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_rejects_duplicate_or_malformed_bundle_ids() {
        let request = ProvisionRequest {
            platform: ProvisionPlatform::Tvos,
            machine_name: "test-machine".to_string(),
            device_name: "test-device".to_string(),
            udid: "AABBCC".to_string(),
            bundle_ids: vec!["com.example.one".to_string(), "com.example.one".to_string()],
            revoke_serials: Vec::new(),
        };
        assert!(validate_request(&request).is_err());

        let malformed = ProvisionRequest {
            bundle_ids: vec!["not-a-bundle".to_string()],
            ..request
        };
        assert!(validate_request(&malformed).is_err());
    }

    #[test]
    fn generated_identity_roundtrips_through_pkcs12() {
        let key = generate_key().unwrap();
        let key_pair = {
            let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
            KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &PKCS_RSA_SHA256).unwrap()
        };
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(DnType::CommonName, "Impresario test");
        let certificate = params.self_signed(&key_pair).unwrap();
        let certificate_der = certificate.der().to_vec();
        let password = Zeroizing::new("test-password".to_string());
        let (p12, password) = create_pkcs12(&key, &certificate_der, password).unwrap();
        let loaded = ExistingIdentity::from_pkcs12(p12.to_vec(), password).unwrap();
        assert_eq!(loaded.leaf_certificate_der, certificate_der);
    }

    #[test]
    fn certificate_public_key_matches_its_key() {
        let key = generate_key().unwrap();
        let key_pair = {
            let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
            KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &PKCS_RSA_SHA256).unwrap()
        };
        let params = CertificateParams::new(Vec::<String>::new()).unwrap();
        let certificate = params.self_signed(&key_pair).unwrap();
        assert!(certificate_der_matches_key(certificate.der(), &key));
    }
}
