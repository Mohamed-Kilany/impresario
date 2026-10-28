//! App Store Connect API client and provisioning resources.
//!
//! The App Store Connect API is Apple's supported unattended path for
//! development certificate and profile management. It authenticates with a
//! team API key (issuer id, key id and a P-256 `.p8` private key) instead of an
//! Apple ID password, trusted-device code or SMS code.
//!
//! This module is deliberately independent of the GrandSlam session. It only
//! speaks JSON: `api.appstoreconnect.apple.com/v1`.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use p256::ecdsa::signature::hazmat::PrehashSigner;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use reqwest::Method;
use reqwest::header::ACCEPT;
use rootcause::prelude::*;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::auth::grandslam::GrandSlam;

const API_ROOT: &str = "https://api.appstoreconnect.apple.com";
const TOKEN_LIFETIME_SECS: u64 = 1_200;
const TOKEN_REFRESH_SKEW_SECS: u64 = 60;

/// An App Store Connect API error, preserved as structured data so callers can
/// distinguish a certificate-limit refusal from a bad request.
#[derive(Debug, thiserror::Error)]
pub enum AppStoreConnectError {
    /// Apple returned an error envelope.
    #[error("App Store Connect API returned HTTP {status}: {detail}")]
    Api {
        status: u16,
        code: String,
        detail: String,
    },
}

impl AppStoreConnectError {
    /// Whether this error is Apple refusing a new certificate because the
    /// team's development certificate slots are full.
    #[must_use]
    pub fn is_certificate_limit(&self) -> bool {
        match self {
            Self::Api {
                status,
                code,
                detail,
            } => {
                let detail = detail.to_lowercase();
                (*status == 403 || *status == 409)
                    && (code.to_uppercase().contains("LIMIT")
                        || detail.contains("limit")
                        || detail.contains("maximum")
                        || detail.contains("too many"))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiErrorEnvelope {
    errors: Vec<ApiError>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    status: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    detail: String,
}

#[derive(Debug, Deserialize)]
struct Links {
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListResponse<T> {
    data: Vec<T>,
    #[serde(default)]
    links: Option<Links>,
}

#[derive(Debug, Deserialize)]
struct DataResponse<T> {
    data: T,
}

struct CachedToken {
    value: String,
    expires_at: u64,
}

/// A typed App Store Connect API client.
///
/// The private key is held in memory for the lifetime of the client. The
/// generated JWT is cached only until shortly before its 20-minute expiry.
pub struct AppStoreConnectClient {
    http: reqwest_middleware::ClientWithMiddleware,
    issuer_id: String,
    key_id: String,
    signing_key: SigningKey,
    token: Mutex<Option<CachedToken>>,
}

impl AppStoreConnectClient {
    /// Build a client from the three values shown in App Store Connect >
    /// Users and Access > Integrations.
    ///
    /// # Errors
    /// Returns an error if an identifier is empty or the `.p8` key cannot be
    /// parsed as a PKCS#8 P-256 private key.
    pub fn new(
        issuer_id: impl Into<String>,
        key_id: impl Into<String>,
        private_key_pem: &str,
    ) -> Result<Self, Report> {
        crate::init_tls();
        let issuer_id = issuer_id.into().trim().to_string();
        let key_id = key_id.into().trim().to_string();
        if issuer_id.is_empty() {
            bail!("the App Store Connect issuer id is empty");
        }
        if key_id.is_empty() {
            bail!("the App Store Connect key id is empty");
        }
        let private_key_pem = Zeroizing::new(private_key_pem.trim().to_string());
        let secret = p256::SecretKey::from_pkcs8_pem(private_key_pem.as_str())
            .context("the App Store Connect API key is not a valid P-256 PKCS#8 .p8 file")?;
        let signing_key = SigningKey::from(&secret);
        let http = GrandSlam::build_reqwest_client(false, None)
            .context("Failed to build the App Store Connect HTTP client")?;
        Ok(Self {
            http,
            issuer_id,
            key_id,
            signing_key,
            token: Mutex::new(None),
        })
    }

    /// Build a client from a downloaded `AuthKey_<KEYID>.p8` file.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed.
    pub fn from_file(
        issuer_id: impl Into<String>,
        key_id: impl Into<String>,
        path: &std::path::Path,
    ) -> Result<Self, Report> {
        let pem = Zeroizing::new(std::fs::read_to_string(path).map_err(|error| {
            report!("read App Store Connect API key {}: {error}", path.display())
        })?);
        Self::new(issuer_id, key_id, &pem)
    }

    fn token(&self) -> Result<String, Report> {
        let now = now_secs();
        let mut cached = self
            .token
            .lock()
            .map_err(|_| report!("the App Store Connect token cache is poisoned"))?;
        if let Some(token) = cached.as_ref()
            && now.saturating_add(TOKEN_REFRESH_SKEW_SECS) < token.expires_at
        {
            return Ok(token.value.clone());
        }

        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "alg": "ES256",
                "kid": self.key_id,
                "typ": "JWT",
            }))
            .context("encode the App Store Connect JWT header")?,
        );
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "iss": self.issuer_id,
                "iat": now,
                "exp": now + TOKEN_LIFETIME_SECS,
                "aud": "appstoreconnect-v1",
            }))
            .context("encode the App Store Connect JWT claims")?,
        );
        let signing_input = format!("{header}.{claims}");
        let digest = Sha256::digest(signing_input.as_bytes());
        let signature: Signature = self
            .signing_key
            .sign_prehash(&digest)
            .context("sign the App Store Connect JWT")?;
        let value = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes().as_slice())
        );
        *cached = Some(CachedToken {
            value: value.clone(),
            expires_at: now + TOKEN_LIFETIME_SECS,
        });
        Ok(value)
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path_or_url: &str,
        body: Option<Value>,
    ) -> Result<T, Report> {
        let text = self.request_text(method, path_or_url, body).await?;
        let value: T = serde_json::from_str(&text)
            .context("App Store Connect API returned a response that is not valid JSON")?;
        Ok(value)
    }

    async fn request_empty(
        &self,
        method: Method,
        path_or_url: &str,
        body: Option<Value>,
    ) -> Result<(), Report> {
        let _ = self.request_text(method, path_or_url, body).await?;
        Ok(())
    }

    async fn request_text(
        &self,
        method: Method,
        path_or_url: &str,
        body: Option<Value>,
    ) -> Result<String, Report> {
        let url = if path_or_url.starts_with("https://") {
            path_or_url.to_string()
        } else {
            format!("{API_ROOT}{path_or_url}")
        };
        let token = self.token()?;
        let mut request = self
            .http
            .request(method, &url)
            .header(ACCEPT, "application/json")
            .bearer_auth(token);
        if let Some(body) = body {
            let body =
                serde_json::to_vec(&body).context("encode the App Store Connect request body")?;
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| report!("send App Store Connect request to {url}: {error}"))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| report!("read the App Store Connect response body: {error}"))?;
        if !status.is_success() {
            return Err(api_error(status.as_u16(), &text));
        }
        Ok(text)
    }

    async fn get_all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, Report> {
        let mut next = Some(format!("{API_ROOT}{path}"));
        let mut values = Vec::new();
        while let Some(url) = next {
            let page: ListResponse<T> = self.request(Method::GET, &url, None).await?;
            values.extend(page.data);
            next = page.links.and_then(|links| links.next).map(|next| {
                if next.starts_with("https://") || next.starts_with('/') {
                    next
                } else if next.starts_with('?') {
                    format!("{path}{next}")
                } else {
                    format!("/{next}")
                }
            });
        }
        Ok(values)
    }

    async fn create<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<T, Report> {
        let response: DataResponse<T> = self.request(Method::POST, path, Some(body)).await?;
        Ok(response.data)
    }

    /// List every registered device.
    pub async fn list_devices(&self) -> Result<Vec<AscDevice>, Report> {
        self.get_all("/v1/devices?limit=200").await
    }

    /// Register a device and return its API id.
    pub async fn register_device(
        &self,
        name: &str,
        udid: &str,
        platform: &str,
    ) -> Result<AscDevice, Report> {
        self.create(
            "/v1/devices",
            json!({
                "data": {
                    "type": "devices",
                    "attributes": {
                        "name": name,
                        "udid": udid,
                        "platform": platform,
                    },
                },
            }),
        )
        .await
    }

    /// Rename an existing device, preserving its registration.
    pub async fn update_device_name(&self, id: &str, name: &str) -> Result<AscDevice, Report> {
        let response: DataResponse<AscDevice> = self
            .request(
                Method::PATCH,
                &format!("/v1/devices/{id}"),
                Some(json!({
                    "data": {
                        "type": "devices",
                        "id": id,
                        "attributes": { "name": name },
                    },
                })),
            )
            .await?;
        Ok(response.data)
    }

    /// List every bundle id (App ID) in the team.
    pub async fn list_bundle_ids(&self) -> Result<Vec<AscBundleId>, Report> {
        self.get_all("/v1/bundleIds?limit=200").await
    }

    /// Register an explicit App ID.
    pub async fn create_bundle_id(
        &self,
        name: &str,
        identifier: &str,
        platform: &str,
    ) -> Result<AscBundleId, Report> {
        self.create(
            "/v1/bundleIds",
            json!({
                "data": {
                    "type": "bundleIds",
                    "attributes": {
                        "identifier": identifier,
                        "name": name,
                        "platform": platform,
                    },
                },
            }),
        )
        .await
    }

    /// List development certificates.
    ///
    /// Apple has both the universal `DEVELOPMENT` (Apple Development) type and
    /// the older platform-specific `IOS_DEVELOPMENT` type. Listing all
    /// certificates and filtering here lets an existing PKCS#12 be reused no
    /// matter which portal flow created it.
    pub async fn list_development_certificates(&self) -> Result<Vec<AscCertificate>, Report> {
        let certificates: Vec<AscCertificate> = self.get_all("/v1/certificates?limit=200").await?;
        Ok(certificates
            .into_iter()
            .filter(|certificate| {
                certificate
                    .attributes
                    .certificate_type
                    .as_deref()
                    .is_none_or(|kind| kind.to_ascii_uppercase().contains("DEVELOPMENT"))
            })
            .collect())
    }

    /// Fetch one certificate, including its content when Apple provides it.
    pub async fn certificate(&self, id: &str) -> Result<AscCertificate, Report> {
        let response: DataResponse<AscCertificate> = self
            .request(Method::GET, &format!("/v1/certificates/{id}"), None)
            .await?;
        Ok(response.data)
    }

    /// Submit a PEM certificate signing request and return the new certificate.
    pub async fn create_development_certificate(
        &self,
        csr_pem: &str,
    ) -> Result<AscCertificate, Report> {
        self.create(
            "/v1/certificates",
            json!({
                "data": {
                    "type": "certificates",
                    "attributes": {
                        "certificateType": "DEVELOPMENT",
                        "csrContent": csr_pem,
                    },
                },
            }),
        )
        .await
    }

    /// Revoke (delete) a certificate by API id.
    pub async fn revoke_certificate(&self, id: &str) -> Result<(), Report> {
        self.request_empty(Method::DELETE, &format!("/v1/certificates/{id}"), None)
            .await
    }

    /// List every provisioning profile in the team.
    pub async fn list_profiles(&self) -> Result<Vec<AscProfile>, Report> {
        self.get_all("/v1/profiles?limit=200").await
    }

    /// Fetch one profile. The response usually includes `profileContent`.
    pub async fn profile(&self, id: &str) -> Result<AscProfile, Report> {
        let response: DataResponse<AscProfile> = self
            .request(Method::GET, &format!("/v1/profiles/{id}"), None)
            .await?;
        Ok(response.data)
    }

    /// Create a development profile bound to one bundle id, certificate and
    /// device. The response includes the base64 profile content.
    pub async fn create_profile(
        &self,
        name: &str,
        profile_type: &str,
        bundle_id: &str,
        certificate_id: &str,
        device_id: Option<&str>,
    ) -> Result<AscProfile, Report> {
        let devices =
            device_id.map_or_else(Vec::new, |id| vec![json!({ "type": "devices", "id": id })]);
        self.create(
            "/v1/profiles",
            json!({
                "data": {
                    "type": "profiles",
                    "attributes": {
                        "name": name,
                        "profileType": profile_type,
                    },
                    "relationships": {
                        "bundleId": {
                            "data": { "type": "bundleIds", "id": bundle_id },
                        },
                        "certificates": {
                            "data": [{ "type": "certificates", "id": certificate_id }],
                        },
                        "devices": { "data": devices },
                    },
                },
            }),
        )
        .await
    }

    /// Delete a provisioning profile.
    pub async fn delete_profile(&self, id: &str) -> Result<(), Report> {
        self.request_empty(Method::DELETE, &format!("/v1/profiles/{id}"), None)
            .await
    }
}

fn api_error(status: u16, text: &str) -> Report {
    let parsed = serde_json::from_str::<ApiErrorEnvelope>(text).ok();
    let (code, detail) = parsed.map_or_else(
        || {
            (
                String::new(),
                text.chars()
                    .take(400)
                    .collect::<String>()
                    .trim()
                    .to_string(),
            )
        },
        |envelope| {
            let errors = envelope.errors;
            let code = errors
                .iter()
                .map(|error| error.code.as_str())
                .filter(|code| !code.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let detail = errors
                .iter()
                .map(|error| {
                    let title = if error.title.is_empty() {
                        error.status.as_str()
                    } else {
                        error.title.as_str()
                    };
                    if error.detail.is_empty() {
                        title.to_string()
                    } else {
                        format!("{title}: {}", error.detail)
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            (code, detail)
        },
    );
    let detail = if detail.is_empty() {
        "no error detail returned".to_string()
    } else {
        detail
    };
    report!(AppStoreConnectError::Api {
        status,
        code,
        detail,
    })
    .into()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// A registered device.
#[derive(Debug, Clone, Deserialize)]
pub struct AscDevice {
    pub id: String,
    pub attributes: AscDeviceAttributes,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AscDeviceAttributes {
    pub name: String,
    pub udid: String,
    pub platform: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub added_date: Option<String>,
}

/// An App ID (bundle id).
#[derive(Debug, Clone, Deserialize)]
pub struct AscBundleId {
    pub id: String,
    pub attributes: AscBundleIdAttributes,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AscBundleIdAttributes {
    pub identifier: String,
    pub name: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub seed_id: Option<String>,
}

/// A development certificate.
#[derive(Debug, Clone, Deserialize)]
pub struct AscCertificate {
    pub id: String,
    pub attributes: AscCertificateAttributes,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AscCertificateAttributes {
    #[serde(default)]
    pub certificate_type: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub expiration_date: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub serial_number: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub certificate_content: Option<String>,
}

impl AscCertificate {
    /// Decode the DER certificate content, when Apple included it.
    #[must_use]
    pub fn certificate_der(&self) -> Option<Vec<u8>> {
        self.attributes
            .certificate_content
            .as_deref()
            .and_then(|content| STANDARD.decode(content).ok())
    }
}

/// A provisioning profile.
#[derive(Debug, Clone, Deserialize)]
pub struct AscProfile {
    pub id: String,
    pub attributes: AscProfileAttributes,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AscProfileAttributes {
    pub name: String,
    #[serde(default)]
    pub profile_type: Option<String>,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub profile_content: String,
    #[serde(default)]
    pub expiration_date: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

impl AscProfile {
    /// Decode the embedded `.mobileprovision`.
    #[must_use]
    pub fn profile_bytes(&self) -> Option<Vec<u8>> {
        STANDARD
            .decode(&self.attributes.profile_content)
            .ok()
            .filter(|bytes| !bytes.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;
    use p256::pkcs8::{EncodePrivateKey, LineEnding};

    fn test_client() -> (AppStoreConnectClient, VerifyingKey) {
        let secret = p256::SecretKey::random(&mut rand::rngs::OsRng);
        let pem = secret.to_pkcs8_pem(LineEnding::LF).unwrap();
        let signing = SigningKey::from(&secret);
        let verifying = VerifyingKey::from(&signing);
        let client = AppStoreConnectClient::new(
            "00000000-0000-0000-0000-000000000000",
            "TESTKEY123",
            pem.as_str(),
        )
        .unwrap();
        (client, verifying)
    }

    #[test]
    fn jwt_has_es256_header_claims_and_a_verifiable_signature() {
        let (client, verifying) = test_client();
        let token = client.token().unwrap();
        let parts = token.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 3);

        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "TESTKEY123");

        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "00000000-0000-0000-0000-000000000000");
        assert_eq!(claims["aud"], "appstoreconnect-v1");

        let signature_bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        let signature = Signature::from_slice(&signature_bytes).unwrap();
        verifying
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn empty_identifiers_and_bad_keys_are_rejected() {
        assert!(AppStoreConnectClient::new("", "KEY", "not-a-key").is_err());
        assert!(AppStoreConnectClient::new("ISSUER", "", "not-a-key").is_err());
        assert!(AppStoreConnectClient::new("ISSUER", "KEY", "not-a-key").is_err());
    }

    #[test]
    fn certificate_limit_errors_are_recognized() {
        let error = AppStoreConnectError::Api {
            status: 403,
            code: "CERTIFICATE_LIMIT".into(),
            detail: "maximum number of certificates".into(),
        };
        assert!(error.is_certificate_limit());
        let error = AppStoreConnectError::Api {
            status: 400,
            code: "BAD_REQUEST".into(),
            detail: "bad CSR".into(),
        };
        assert!(!error.is_certificate_limit());
    }
}
