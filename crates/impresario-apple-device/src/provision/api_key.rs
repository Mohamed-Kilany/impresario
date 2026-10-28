//! App Store Connect API-key provisioning.
//!
//! This is the unattended path: it uses the team API key, not an Apple ID
//! session. It mirrors the GrandSlam provisioning flow but talks to
//! `api.appstoreconnect.apple.com` for devices, bundle ids, certificates and
//! profiles.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use impresario_apple_auth::dev::{
    app_ids::Profile as ApiProfile,
    app_store_connect::{
        AppStoreConnectClient, AppStoreConnectError, AscBundleId, AscCertificate, AscProfile,
    },
    teams::DeveloperTeam,
};
use plist::{Data, Date};
use rootcause::Report;
use zeroize::Zeroizing;

use super::{
    app_id_name, create_pkcs12, generate_key, new_password, validate_profile, validate_request,
    CertificateSummary, ExistingIdentity, IdentityMaterial, ProvisionError, ProvisionPlatform,
    ProvisionReport, ProvisionRequest, ProvisionedBundle,
};
use crate::signing_set::PASSWORD_ENV;

/// List the team's development certificates through the API key.
pub async fn list_certificates_api(
    client: &AppStoreConnectClient,
) -> Result<Vec<CertificateSummary>, ProvisionError> {
    let certificates = client
        .list_development_certificates()
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!("could not list the team's certificates: {error}"))
        })?;
    Ok(certificates.iter().map(certificate_summary).collect())
}

/// Provision a development identity and profiles with an App Store Connect
/// API key.
///
/// `team_id` is the ten-character Apple Developer team id. The API key itself
/// identifies the team, but Apple does not expose the team id through this
/// endpoint, so the caller supplies it for profile validation.
pub async fn provision_with_api_key(
    client: &AppStoreConnectClient,
    team_id: &str,
    request: &ProvisionRequest,
    existing: Option<&ExistingIdentity>,
) -> Result<ProvisionReport, ProvisionError> {
    validate_request(request)?;
    let team_id = team_id.trim();
    if team_id.is_empty() {
        return Err(ProvisionError::Invalid(
            "the Apple Developer team id is empty; pass --team".to_string(),
        ));
    }
    let team = DeveloperTeam {
        name: None,
        team_id: team_id.to_string(),
        r#type: None,
        status: None,
    };

    let device = ensure_device(client, request).await?;
    let (app_ids, app_ids_created, app_ids_existing) = ensure_app_ids(client, request).await?;

    revoke_requested_certificates(client, request).await?;

    let identity = ensure_identity(client, request, existing).await?;

    let (bundles, warnings) =
        match download_bundles(client, &team, request, &app_ids, &device.id, &identity).await {
            Ok(result) => result,
            Err(error) => {
                return Err(match revoke_failed_identity(client, &identity).await {
                    Ok(()) => error,
                    Err(cleanup) => ProvisionError::Apple(format!("{error}; {cleanup}")),
                });
            }
        };

    Ok(ProvisionReport {
        team_id: team.team_id.clone(),
        platform: request.platform,
        device_udid: request.udid.clone(),
        device_name: request.device_name.clone(),
        app_ids_created,
        app_ids_existing,
        identity: identity.material,
        bundles,
        warnings,
    })
}

async fn ensure_device(
    client: &AppStoreConnectClient,
    request: &ProvisionRequest,
) -> Result<impresario_apple_auth::dev::app_store_connect::AscDevice, ProvisionError> {
    let devices = client.list_devices().await.map_err(|error| {
        ProvisionError::Apple(format!("could not list the team's devices: {error}"))
    })?;
    if let Some(device) = devices
        .into_iter()
        .find(|device| device.attributes.udid.eq_ignore_ascii_case(&request.udid))
    {
        if device.attributes.name == request.device_name {
            return Ok(device);
        }
        return client
            .update_device_name(&device.id, &request.device_name)
            .await
            .map_err(|error| {
                ProvisionError::Apple(format!(
                    "could not rename device {} to {}: {error}",
                    request.udid, request.device_name
                ))
            });
    }
    client
        .register_device(
            &request.device_name,
            &request.udid,
            api_platform(request.platform),
        )
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!(
                "could not register device {} with team: {error}",
                request.udid
            ))
        })
}

async fn ensure_app_ids(
    client: &AppStoreConnectClient,
    request: &ProvisionRequest,
) -> Result<(BTreeMap<String, AscBundleId>, Vec<String>, Vec<String>), ProvisionError> {
    let mut by_identifier = client
        .list_bundle_ids()
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!("could not list the team's App IDs: {error}"))
        })?
        .into_iter()
        .map(|bundle| (bundle.attributes.identifier.clone(), bundle))
        .collect::<BTreeMap<_, _>>();

    let mut created = Vec::new();
    let mut existing = Vec::new();
    for bundle_id in &request.bundle_ids {
        if by_identifier.contains_key(bundle_id) {
            existing.push(bundle_id.clone());
            continue;
        }
        let name = app_id_name(bundle_id);
        let bundle = match client
            .create_bundle_id(&name, bundle_id, api_platform(request.platform))
            .await
        {
            Ok(bundle) => bundle,
            Err(error) => {
                // Re-list once: Apple can report a duplicate before the
                // collection endpoint reflects it.
                let refreshed = client.list_bundle_ids().await;
                if let Ok(refreshed) = refreshed {
                    if let Some(bundle) = refreshed
                        .into_iter()
                        .find(|bundle| bundle.attributes.identifier == *bundle_id)
                    {
                        bundle
                    } else {
                        return Err(ProvisionError::Apple(format!(
                            "could not add App ID {bundle_id}: {error}"
                        )));
                    }
                } else {
                    return Err(ProvisionError::Apple(format!(
                        "could not add App ID {bundle_id}: {error}"
                    )));
                }
            }
        };
        created.push(bundle_id.clone());
        by_identifier.insert(bundle_id.clone(), bundle);
    }
    Ok((by_identifier, created, existing))
}

async fn revoke_requested_certificates(
    client: &AppStoreConnectClient,
    request: &ProvisionRequest,
) -> Result<(), ProvisionError> {
    if request.revoke_serials.is_empty() {
        return Ok(());
    }
    let certificates = client
        .list_development_certificates()
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!("could not list the team's certificates: {error}"))
        })?;
    for serial in &request.revoke_serials {
        let certificate = certificates
            .iter()
            .find(|certificate| {
                certificate
                    .attributes
                    .serial_number
                    .as_deref()
                    .is_some_and(|candidate| serials_equal(candidate, serial))
            })
            .ok_or_else(|| {
                ProvisionError::Apple(format!(
                    "could not find certificate serial {serial} in the team"
                ))
            })?;
        client
            .revoke_certificate(&certificate.id)
            .await
            .map_err(|error| {
                ProvisionError::Apple(format!("could not revoke certificate {serial}: {error}"))
            })?;
    }
    Ok(())
}

struct ApiIdentity {
    material: IdentityMaterial,
    certificate_id: String,
}

async fn ensure_identity(
    client: &AppStoreConnectClient,
    request: &ProvisionRequest,
    existing: Option<&ExistingIdentity>,
) -> Result<ApiIdentity, ProvisionError> {
    let certificates = client
        .list_development_certificates()
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!("could not list the team's certificates: {error}"))
        })?;
    let active = certificates
        .iter()
        .filter(|certificate| certificate_is_active(certificate))
        .collect::<Vec<_>>();

    if let Some(identity) = existing {
        for certificate in &active {
            let content = certificate_content(client, certificate).await?;
            if content == identity.leaf_certificate_der {
                return Ok(ApiIdentity {
                    material: IdentityMaterial {
                        certificate_serial: certificate.attributes.serial_number.clone(),
                        certificate_name: certificate_name(certificate),
                        reused_certificate: true,
                        reused_key: true,
                        p12: identity.p12.clone(),
                        password: identity.password.clone(),
                        leaf_certificate_der: content,
                    },
                    certificate_id: certificate.id.clone(),
                });
            }
        }
    }

    let (private_key, reused_key) = existing.map_or_else(
        || {
            generate_key().map(|key| (key, false)).map_err(|error| {
                ProvisionError::Local(format!("could not generate an RSA key: {error}"))
            })
        },
        |identity| Ok((identity.private_key.clone(), true)),
    )?;
    let csr = super::certificate_signing_request(&private_key, &request.machine_name)
        .map_err(|error| ProvisionError::Local(format!("could not build the CSR: {error}")))?;
    let certificate = match client.create_development_certificate(&csr).await {
        Ok(certificate) => certificate,
        Err(error) if is_certificate_limit_error(&error) => {
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
    let certificate_der = match certificate_content(client, &certificate).await {
        Ok(content) => content,
        Err(error) => {
            return Err(clean_up_new_certificate(client, &certificate, error).await);
        }
    };
    // The existing certificate was not reusable, so this is a new PKCS#12.
    // Always give it a fresh password rather than carrying over the old one
    // (which may have been exposed). The reused-certificate path above
    // returns before reaching this point.
    let password = std::env::var(PASSWORD_ENV)
        .map(Zeroizing::new)
        .unwrap_or_else(|_| new_password());
    let (p12, password) = match create_pkcs12(&private_key, &certificate_der, password) {
        Ok(pkcs12) => pkcs12,
        Err(error) => {
            return Err(clean_up_new_certificate(client, &certificate, error).await);
        }
    };
    Ok(ApiIdentity {
        material: IdentityMaterial {
            certificate_serial: certificate.attributes.serial_number.clone(),
            certificate_name: certificate_name(&certificate),
            reused_certificate: false,
            reused_key,
            p12,
            password,
            leaf_certificate_der: certificate_der,
        },
        certificate_id: certificate.id,
    })
}

async fn download_bundles(
    client: &AppStoreConnectClient,
    team: &DeveloperTeam,
    request: &ProvisionRequest,
    app_ids: &BTreeMap<String, AscBundleId>,
    device_id: &str,
    identity: &ApiIdentity,
) -> Result<(Vec<ProvisionedBundle>, Vec<String>), ProvisionError> {
    let profiles = client.list_profiles().await.map_err(|error| {
        ProvisionError::Apple(format!("could not list the team's profiles: {error}"))
    })?;
    let mut bundles = Vec::with_capacity(request.bundle_ids.len());
    let mut warnings = Vec::new();
    for bundle_id in &request.bundle_ids {
        let app_id = app_ids.get(bundle_id).ok_or_else(|| {
            ProvisionError::Apple(format!("Apple did not return the App ID for {bundle_id}"))
        })?;
        let profile_name = profile_name(bundle_id, request.platform);
        for profile in profiles
            .iter()
            .filter(|profile| profile.attributes.name == profile_name)
        {
            client.delete_profile(&profile.id).await.map_err(|error| {
                ProvisionError::Apple(format!(
                    "could not replace old profile {profile_name}: {error}"
                ))
            })?;
        }

        let mut last_error = None;
        let mut bundle = None;
        for attempt in 0..3 {
            let profile = match client
                .create_profile(
                    &profile_name,
                    api_profile_type(request.platform),
                    &app_id.id,
                    &identity.certificate_id,
                    Some(device_id),
                )
                .await
            {
                Ok(profile) => profile,
                Err(error) => {
                    last_error = Some(error.to_string());
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    continue;
                }
            };
            let profile = match materialize_profile(client, profile).await {
                Ok(profile) => profile,
                Err(error) => return Err(error),
            };
            let mut attempt_warnings = Vec::new();
            let api_profile = api_profile(&profile, app_id, request.platform)?;
            match validate_profile(
                request,
                team,
                bundle_id,
                &api_profile,
                &identity.material.leaf_certificate_der,
                &mut attempt_warnings,
            ) {
                Ok(result) => {
                    warnings.append(&mut attempt_warnings);
                    bundle = Some(result);
                    break;
                }
                Err(error @ ProvisionError::ProfileCertificateMissing(_)) => {
                    last_error = Some(error.to_string());
                    if attempt == 2 {
                        break;
                    }
                    // Apple can expose a new profile before the new
                    // certificate is embedded. Replace it and retry.
                    let _ = client.delete_profile(&profile.id).await;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error),
            }
        }
        bundles.push(bundle.ok_or_else(|| {
            ProvisionError::Apple(format!(
                "could not create a profile for {bundle_id}: {}",
                last_error.unwrap_or_else(|| "unknown Apple error".to_string())
            ))
        })?);
    }
    Ok((bundles, warnings))
}

async fn materialize_profile(
    client: &AppStoreConnectClient,
    mut profile: AscProfile,
) -> Result<AscProfile, ProvisionError> {
    if profile.profile_bytes().is_none() {
        profile = client.profile(&profile.id).await.map_err(|error| {
            ProvisionError::Apple(format!(
                "could not download profile {}: {error}",
                profile.id
            ))
        })?;
    }
    if profile.profile_bytes().is_none() {
        return Err(ProvisionError::Apple(format!(
            "Apple returned no profile content for {}",
            profile.attributes.name
        )));
    }
    Ok(profile)
}

fn api_profile(
    profile: &AscProfile,
    app_id: &AscBundleId,
    platform: ProvisionPlatform,
) -> Result<ApiProfile, ProvisionError> {
    let bytes = profile.profile_bytes().ok_or_else(|| {
        ProvisionError::Apple(format!(
            "Apple returned no profile content for {}",
            profile.attributes.name
        ))
    })?;
    Ok(ApiProfile {
        encoded_profile: Data::new(bytes),
        filename: format!("{}.mobileprovision", profile.attributes.name),
        provisioning_profile_id: profile.id.clone(),
        name: profile.attributes.name.clone(),
        status: profile
            .attributes
            .status
            .clone()
            .unwrap_or_else(|| "Active".to_string()),
        r#type: profile
            .attributes
            .profile_type
            .clone()
            .unwrap_or_else(|| api_profile_type(platform).to_string()),
        distribution_method: "development".to_string(),
        pro_pro_platorm: Some(platform.profile_label().to_string()),
        uuid: profile.attributes.uuid.clone(),
        // The profile plist normally carries ExpirationDate, which
        // `validate_profile` prefers. Use a future fallback so a missing
        // field cannot look expired because of clock skew.
        date_expire: Date::from(SystemTime::now() + Duration::from_secs(86_400)),
        managing_app: None,
        app_id_id: app_id.id.clone(),
        is_template_profile: false,
        is_team_profile: None,
        is_free_provisioning_profile: None,
    })
}

async fn revoke_failed_identity(
    client: &AppStoreConnectClient,
    identity: &ApiIdentity,
) -> Result<(), String> {
    if identity.material.reused_certificate {
        return Ok(());
    }
    let serial = identity
        .material
        .certificate_serial
        .as_deref()
        .ok_or_else(|| {
            "the newly issued certificate has no serial, so it could not be revoked automatically"
                .to_string()
        })?;
    client
        .revoke_certificate(&identity.certificate_id)
        .await
        .map_err(|error| format!("could not revoke the unused certificate {serial}: {error}"))
}

async fn clean_up_new_certificate(
    client: &AppStoreConnectClient,
    certificate: &AscCertificate,
    error: ProvisionError,
) -> ProvisionError {
    match client.revoke_certificate(&certificate.id).await {
        Ok(()) => error,
        Err(cleanup) => ProvisionError::Apple(format!(
            "{error}; could not revoke the unused certificate {}: {cleanup}",
            certificate
                .attributes
                .serial_number
                .as_deref()
                .unwrap_or("<unknown serial>")
        )),
    }
}

async fn certificate_content(
    client: &AppStoreConnectClient,
    certificate: &AscCertificate,
) -> Result<Vec<u8>, ProvisionError> {
    if let Some(content) = certificate.certificate_der() {
        return Ok(content);
    }
    client
        .certificate(&certificate.id)
        .await
        .map_err(|error| {
            ProvisionError::Apple(format!(
                "could not download certificate {}: {error}",
                certificate.id
            ))
        })?
        .certificate_der()
        .ok_or_else(|| {
            ProvisionError::Apple(format!(
                "Apple returned certificate {} without content",
                certificate
                    .attributes
                    .serial_number
                    .as_deref()
                    .unwrap_or("<unknown serial>")
            ))
        })
}

fn certificate_is_active(certificate: &AscCertificate) -> bool {
    let not_expired = certificate
        .attributes
        .expiration_date
        .as_deref()
        .and_then(|value| Date::from_xml_format(value).ok())
        .is_none_or(|date| SystemTime::from(date) > SystemTime::now());
    let not_revoked = certificate
        .attributes
        .status
        .as_deref()
        .is_none_or(|status| {
            !matches!(
                status.to_ascii_lowercase().as_str(),
                "revoked" | "expired" | "invalid"
            )
        });
    not_expired && not_revoked
}

fn certificate_name(certificate: &AscCertificate) -> Option<String> {
    certificate
        .attributes
        .name
        .clone()
        .or_else(|| certificate.attributes.display_name.clone())
}

fn certificate_summary(certificate: &AscCertificate) -> CertificateSummary {
    CertificateSummary {
        serial_number: certificate
            .attributes
            .serial_number
            .clone()
            .unwrap_or_else(|| "<missing serial>".to_string()),
        name: certificate.attributes.name.clone(),
        machine_name: certificate.attributes.display_name.clone(),
        expiration: certificate.attributes.expiration_date.clone(),
        status: certificate.attributes.status.clone(),
    }
}

fn is_certificate_limit_error(error: &Report) -> bool {
    error.iter_reports().any(|cause| {
        matches!(
            cause.downcast_current_context::<AppStoreConnectError>(),
            Some(error) if error.is_certificate_limit()
        )
    })
}

fn serials_equal(left: &str, right: &str) -> bool {
    left.trim_start_matches('0')
        .eq_ignore_ascii_case(right.trim_start_matches('0'))
}

fn api_platform(_platform: ProvisionPlatform) -> &'static str {
    // App Store Connect's device and bundle-id platform enum is IOS, MAC_OS
    // or UNIVERSAL. Apple TV and Apple Watch are registered under IOS; the
    // TVOS_APP_DEVELOPMENT profile type carries the tvOS distinction.
    "IOS"
}

fn api_profile_type(platform: ProvisionPlatform) -> &'static str {
    match platform {
        ProvisionPlatform::Tvos => "TVOS_APP_DEVELOPMENT",
        ProvisionPlatform::Ios => "IOS_APP_DEVELOPMENT",
    }
}

fn profile_name(bundle_id: &str, platform: ProvisionPlatform) -> String {
    format!(
        "Impresario {} Development {}",
        platform.profile_label(),
        bundle_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_apple_auth::dev::app_store_connect::AscCertificateAttributes;

    #[test]
    fn serial_matching_ignores_case_and_leading_zeroes() {
        assert!(serials_equal("00ab", "AB"));
        assert!(serials_equal("1234", "1234"));
        assert!(!serials_equal("1234", "1235"));
    }

    #[test]
    fn tvos_uses_ios_device_platform_and_tvos_profile_type() {
        assert_eq!(api_platform(ProvisionPlatform::Tvos), "IOS");
        assert_eq!(api_platform(ProvisionPlatform::Ios), "IOS");
        assert_eq!(
            api_profile_type(ProvisionPlatform::Tvos),
            "TVOS_APP_DEVELOPMENT"
        );
        assert_eq!(
            api_profile_type(ProvisionPlatform::Ios),
            "IOS_APP_DEVELOPMENT"
        );
    }

    #[test]
    fn expired_certificates_are_not_reused() {
        let certificate = AscCertificate {
            id: "cert".to_string(),
            attributes: AscCertificateAttributes {
                certificate_type: Some("DEVELOPMENT".to_string()),
                display_name: None,
                expiration_date: Some("2000-01-01T00:00:00Z".to_string()),
                name: None,
                platform: Some("IOS".to_string()),
                serial_number: Some("01".to_string()),
                status: Some("Active".to_string()),
                certificate_content: None,
            },
        };
        assert!(!certificate_is_active(&certificate));
    }

    #[test]
    fn certificate_limit_error_is_found_in_a_rootcause_report() {
        let error: Report = rootcause::report!(AppStoreConnectError::Api {
            status: 403,
            code: "CERTIFICATE_LIMIT".to_string(),
            detail: "maximum number of certificates".to_string(),
        })
        .into();
        assert!(is_certificate_limit_error(&error));
    }

    #[test]
    fn profile_names_are_managed_per_platform_and_bundle() {
        let name = profile_name("com.example.app", ProvisionPlatform::Tvos);
        assert!(name.contains("tvOS"));
        assert!(name.contains("com.example.app"));
        assert_ne!(
            name,
            profile_name("com.example.app", ProvisionPlatform::Ios)
        );
    }
}
