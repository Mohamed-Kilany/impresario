//! Samsung-issued Tizen TV certificate profiles without Tizen Studio.
//!
//! Samsung's Certificate Manager is a Java front end around a public browser
//! login and certificate service. This module reproduces the useful part in
//! Rust: it discovers the current service endpoints from Samsung's certificate
//! extension, waits for the user to sign in through the normal browser, and
//! turns the returned token into the author/distributor PKCS#12 pair that the
//! Tizen signer already understands.
//!
//! No Samsung password is read or stored here. The browser owns the account
//! login; the short-lived access token is kept in memory and zeroized. The
//! generated PKCS#12 password is random and is stored only by the existing
//! keychain-backed profile import.
//!
//! The service endpoints are discovered because Samsung has moved them before
//! (v2 -> v3) and the extension remains the source of truth used by Tizen
//! Studio itself. A seven-day cache avoids a 44 MB download on every run.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::{Cursor, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use impresario_core::home::Home;
use p12_keystore::{
    Certificate, EncryptionAlgorithm, KeyStore, KeyStoreEntry, LocalKeyId, MacAlgorithm,
    PrivateKey, PrivateKeyChain,
};
use rcgen::{CertificateParams, DnType, DnValue, KeyPair, SanType, PKCS_RSA_SHA512};
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::rand_core::OsRng;
use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use url::form_urlencoded;
use x509_parser::prelude::*;
use zeroize::Zeroizing;

use crate::tizen_sign::{self, ProfileRecord, SignError};

const EXTENSION_INFO_URL: &str =
    "https://download.tizen.org/sdk/tizenstudio/official/extension_info.xml";
const DEFAULT_EXTENSION_URL: &str =
    "https://download.tizen.org/sdk/extensions/tizen-certificate-extension_2.0.75.zip";
const DEFAULT_LOGIN_URL: &str = "https://account.samsung.com/mobile/account/check.do?serviceID=v285zxnl3h&actionID=StartOAuth2&accessToken=Y&redirect_uri=http://localhost:4794/signin/callback";
const DEFAULT_AUTHOR_URL: &str = "https://svdca.samsungqbe.com/apis/v3/authors";
const DEFAULT_DISTRIBUTOR_URL: &str = "https://svdca.samsungqbe.com/apis/v3/distributors";
const AUTHOR_CA_NAME: &str = "vd_tizen_dev_author_ca.cer";
const DISTRIBUTOR_CA_NAME: &str = "vd_tizen_dev_public2.crt";
const CACHE_FILE: &str = "tizen-samsung.json";
const CACHE_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CERTIFICATE_BYTES: u64 = 1024 * 1024;
const MAX_HTTP_REQUEST_BYTES: usize = 64 * 1024;
const P12_ITERATIONS: u32 = 2048;
const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// The Samsung endpoints and CA chain needed to create a TV profile.
#[derive(Debug, Clone)]
pub struct SamsungConfig {
    pub service_id: String,
    pub login_url: String,
    pub author_url: String,
    pub distributor_url: String,
    pub author_ca_der: Vec<u8>,
    pub distributor_ca_der: Vec<u8>,
}

/// Options for the interactive Samsung profile creation flow.
#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub profile_name: String,
    pub duids: Vec<String>,
    pub email: Option<String>,
    pub refresh: bool,
    pub open_browser: bool,
    pub extension_url: Option<String>,
    pub login_timeout: Duration,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            profile_name: "samsung".to_string(),
            duids: Vec::new(),
            email: None,
            refresh: false,
            open_browser: true,
            extension_url: None,
            login_timeout: DEFAULT_LOGIN_TIMEOUT,
        }
    }
}

/// Samsung certificate-service failures. Messages never include access
/// tokens, callback payloads, or private keys.
#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("Samsung certificate network error: {0}")]
    Network(String),
    #[error("Samsung certificate extension error: {0}")]
    Extension(String),
    #[error("Samsung login error: {0}")]
    Login(String),
    #[error("Samsung certificate request failed: {0}")]
    Certificate(String),
    #[error("Samsung profile configuration error: {0}")]
    Config(String),
    #[error(transparent)]
    Sign(#[from] SignError),
    #[error("Samsung profile I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Samsung profile JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Samsung profile ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedConfig {
    service_id: String,
    login_url: String,
    author_url: String,
    distributor_url: String,
    author_ca_der_base64: String,
    distributor_ca_der_base64: String,
    fetched_unix_seconds: u64,
}

impl CachedConfig {
    fn from_config(config: &SamsungConfig) -> Self {
        Self {
            service_id: config.service_id.clone(),
            login_url: config.login_url.clone(),
            author_url: config.author_url.clone(),
            distributor_url: config.distributor_url.clone(),
            author_ca_der_base64: base64::engine::general_purpose::STANDARD
                .encode(&config.author_ca_der),
            distributor_ca_der_base64: base64::engine::general_purpose::STANDARD
                .encode(&config.distributor_ca_der),
            fetched_unix_seconds: now_unix_seconds(),
        }
    }

    fn into_config(self) -> Result<SamsungConfig, CertError> {
        let author_ca_der = base64::engine::general_purpose::STANDARD
            .decode(self.author_ca_der_base64)
            .map_err(|error| CertError::Config(format!("cached author CA: {error}")))?;
        let distributor_ca_der = base64::engine::general_purpose::STANDARD
            .decode(self.distributor_ca_der_base64)
            .map_err(|error| CertError::Config(format!("cached distributor CA: {error}")))?;
        Ok(SamsungConfig {
            service_id: self.service_id,
            login_url: self.login_url,
            author_url: self.author_url,
            distributor_url: self.distributor_url,
            author_ca_der,
            distributor_ca_der,
        })
    }

    fn is_fresh(&self) -> bool {
        let now = now_unix_seconds();
        self.fetched_unix_seconds > 0
            && now.saturating_sub(self.fetched_unix_seconds) < CACHE_TTL_SECONDS
            && !self.service_id.is_empty()
            && !self.login_url.is_empty()
            && !self.author_ca_der_base64.is_empty()
            && !self.distributor_ca_der_base64.is_empty()
    }
}

#[derive(Debug)]
struct GeneratedMaterial {
    key_der: Zeroizing<Vec<u8>>,
    csr_pem: String,
}

#[derive(Debug)]
struct Login {
    access_token: Zeroizing<String>,
    user_id: String,
    email: Option<String>,
}

#[derive(Debug)]
struct Callback {
    port: u16,
    path: String,
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    target: String,
    body: Vec<u8>,
}

/// Create and import a Samsung-issued Tizen profile for one or more TV DUIDs.
///
/// The function blocks while the browser login is pending, but all certificate
/// work is native Rust and all private material stays in memory until the
/// existing atomic profile import writes the encrypted PKCS#12 files.
pub fn create_profile(home: &Home, options: &CreateOptions) -> Result<ProfileRecord, CertError> {
    let duids = normalize_duids(&options.duids)?;
    let config = load_samsung_config(home, options.refresh, options.extension_url.as_deref())?;
    let login = wait_for_login(
        &config.login_url,
        options.open_browser,
        options.login_timeout,
    )?;
    let email = login
        .email
        .clone()
        .or_else(|| options.email.clone())
        .ok_or_else(|| {
            CertError::Login(
                "Samsung login did not return an email address; pass --email <address>".to_string(),
            )
        })?;
    validate_email(&email)?;

    let author = generate_material(&email, &duids, false)?;
    let distributor = generate_material(&email, &duids, true)?;
    let author_response = request_certificate(
        &config.author_url,
        &login.access_token,
        &login.user_id,
        &author.csr_pem,
        false,
    )?;
    let distributor_response = request_certificate(
        &config.distributor_url,
        &login.access_token,
        &login.user_id,
        &distributor.csr_pem,
        true,
    )?;

    let author_chain = certificate_chain(&author_response, &config.author_ca_der)?;
    let distributor_chain = certificate_chain(&distributor_response, &config.distributor_ca_der)?;
    let distributor_leaf = distributor_chain.first().ok_or_else(|| {
        CertError::Certificate("Samsung returned no distributor certificate".into())
    })?;
    let covered = certificate_duids(distributor_leaf)?;
    for duid in &duids {
        if !covered.iter().any(|candidate| candidate == duid) {
            return Err(CertError::Certificate(format!(
                "Samsung's distributor certificate does not cover DUID {duid}"
            )));
        }
    }

    let password = random_password();
    let author_p12 = build_pkcs12(&author.key_der, &author_chain, &password, "usercertificate")?;
    let distributor_p12 = build_pkcs12(
        &distributor.key_der,
        &distributor_chain,
        &password,
        "usercertificate",
    )?;
    let record = tizen_sign::import_profile_bytes(
        home,
        &options.profile_name,
        &author_p12,
        &password,
        &distributor_p12,
        &password,
    )?;
    Ok(record)
}

/// Load the Samsung service metadata, using the seven-day cache unless
/// `refresh` is set or an explicit extension URL is supplied.
pub fn load_samsung_config(
    home: &Home,
    refresh: bool,
    extension_url: Option<&str>,
) -> Result<SamsungConfig, CertError> {
    let cache_path = home.cache_dir().join(CACHE_FILE);
    if !refresh && extension_url.is_none() {
        if let Ok(bytes) = std::fs::read(&cache_path) {
            if let Ok(cached) = serde_json::from_slice::<CachedConfig>(&bytes) {
                if cached.is_fresh() {
                    if let Ok(config) = cached.into_config() {
                        return Ok(config);
                    }
                }
            }
        }
    }

    let extension_url = match extension_url {
        Some(url) => url.to_string(),
        None => match discover_extension_url() {
            Ok(url) => url,
            Err(error) => {
                eprintln!("warning: {error}; using the known certificate extension");
                DEFAULT_EXTENSION_URL.to_string()
            }
        },
    };
    let archive = download(&extension_url)?;
    let parsed = parse_extension_archive(&archive)?;
    let config = SamsungConfig {
        service_id: parsed.service_id,
        login_url: parsed.login_url,
        author_url: parsed.author_url,
        distributor_url: parsed.distributor_url,
        author_ca_der: parsed.author_ca_der,
        distributor_ca_der: parsed.distributor_ca_der,
    };
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&CachedConfig::from_config(&config)) {
        let _ = std::fs::write(&cache_path, bytes);
    }
    Ok(config)
}

fn discover_extension_url() -> Result<String, CertError> {
    let xml = download_text(EXTENSION_INFO_URL)?;
    extension_repository(&xml).ok_or_else(|| {
        CertError::Extension(
            "the Samsung Certificate Extension is not listed in Tizen's extension index".into(),
        )
    })
}

fn extension_repository(xml: &str) -> Option<String> {
    let document = roxmltree::Document::parse(xml).ok()?;
    for extension in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "extension")
    {
        let name = extension
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "name")
            .and_then(|node| node.text())
            .map(str::trim);
        if name != Some("Samsung Certificate Extension") {
            continue;
        }
        return extension
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "repository")
            .and_then(|node| node.text())
            .map(|text| text.trim().to_string());
    }
    None
}

#[derive(Debug)]
struct ExtensionData {
    service_id: String,
    login_url: String,
    author_url: String,
    distributor_url: String,
    author_ca_der: Vec<u8>,
    distributor_ca_der: Vec<u8>,
}

fn parse_extension_archive(bytes: &[u8]) -> Result<ExtensionData, CertError> {
    let mut outer = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut inner_bytes = None;
    for index in 0..outer.len() {
        let name = outer
            .by_index(index)
            .map(|file| file.name().to_string())
            .map_err(|error| CertError::Extension(format!("outer extension archive: {error}")))?;
        if name.contains("cert-add-on") && name.ends_with("_ubuntu-64.zip") {
            let mut file = outer.by_index(index)?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)?;
            inner_bytes = Some(buffer);
            break;
        }
    }
    let inner_bytes = inner_bytes.ok_or_else(|| {
        CertError::Extension("the certificate extension has no ubuntu certificate archive".into())
    })?;

    let mut inner = zip::ZipArchive::new(Cursor::new(inner_bytes))?;
    let mut jar_bytes = None;
    for index in 0..inner.len() {
        let name = inner
            .by_index(index)
            .map(|file| file.name().to_string())
            .map_err(|error| CertError::Extension(format!("certificate archive: {error}")))?;
        let basename = name.rsplit('/').next().unwrap_or(&name);
        if basename.starts_with("org.tizen.common.cert") && basename.ends_with(".jar") {
            let mut file = inner.by_index(index)?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)?;
            jar_bytes = Some(buffer);
            break;
        }
    }
    let jar_bytes = jar_bytes.ok_or_else(|| {
        CertError::Extension("the certificate archive has no org.tizen.common.cert JAR".into())
    })?;

    let mut jar = zip::ZipArchive::new(Cursor::new(jar_bytes))?;
    let mut constants = Vec::new();
    let mut author_ca = None;
    let mut distributor_ca = None;
    for index in 0..jar.len() {
        let name = jar
            .by_index(index)
            .map(|file| file.name().to_string())
            .map_err(|error| CertError::Extension(format!("certificate JAR: {error}")))?;
        let basename = name.rsplit('/').next().unwrap_or(&name);
        if basename == "SigninDialog.class" || basename == "CertConstant.class" {
            let mut file = jar.by_index(index)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            constants.extend(class_constants(&bytes)?);
        } else if basename == AUTHOR_CA_NAME {
            let mut file = jar.by_index(index)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            author_ca = Some(bytes);
        } else if basename == DISTRIBUTOR_CA_NAME {
            let mut file = jar.by_index(index)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            distributor_ca = Some(bytes);
        }
    }

    let service_id =
        pair_value(&constants, "SERVICE_ID").unwrap_or_else(|| "v285zxnl3h".to_string());
    let login_url =
        pair_value(&constants, "loginUrl").unwrap_or_else(|| DEFAULT_LOGIN_URL.to_string());
    let author_url =
        pair_value(&constants, "AUTHOR_URL").unwrap_or_else(|| DEFAULT_AUTHOR_URL.to_string());
    let distributor_url = pair_value(&constants, "DISTRIBUTOR_URL")
        .unwrap_or_else(|| DEFAULT_DISTRIBUTOR_URL.to_string());
    let author_ca = author_ca.ok_or_else(|| {
        CertError::Extension(format!("the certificate JAR has no {AUTHOR_CA_NAME}"))
    })?;
    let distributor_ca = distributor_ca.ok_or_else(|| {
        CertError::Extension(format!("the certificate JAR has no {DISTRIBUTOR_CA_NAME}"))
    })?;
    Ok(ExtensionData {
        service_id,
        login_url,
        author_url,
        distributor_url,
        author_ca_der: single_certificate(&author_ca)?,
        distributor_ca_der: single_certificate(&distributor_ca)?,
    })
}

fn pair_value(constants: &[String], key: &str) -> Option<String> {
    constants.windows(2).find_map(|pair| {
        if pair.first().map(String::as_str) == Some(key) {
            pair.get(1).cloned()
        } else {
            None
        }
    })
}

fn class_constants(bytes: &[u8]) -> Result<Vec<String>, CertError> {
    if bytes.len() < 10 || bytes.get(..4) != Some(&[0xca, 0xfe, 0xba, 0xbe]) {
        return Err(CertError::Extension(
            "certificate extension contains an invalid Java class".into(),
        ));
    }
    let mut offset = 8usize;
    let count = read_u16(bytes, &mut offset)? as usize;
    let mut constants = Vec::new();
    let mut index = 1usize;
    while index < count {
        let tag = read_u8(bytes, &mut offset)?;
        match tag {
            0 => {}
            1 => {
                let length = read_u16(bytes, &mut offset)? as usize;
                let end = offset
                    .checked_add(length)
                    .ok_or_else(|| CertError::Extension("class constant overflow".into()))?;
                let value = bytes
                    .get(offset..end)
                    .ok_or_else(|| CertError::Extension("truncated Java UTF-8 constant".into()))?;
                constants.push(String::from_utf8_lossy(value).into_owned());
                offset = end;
            }
            3 | 4 => skip(bytes, &mut offset, 4)?,
            5 | 6 => {
                skip(bytes, &mut offset, 8)?;
                index += 1;
            }
            7 | 8 | 16 | 19 | 20 => skip(bytes, &mut offset, 2)?,
            9 | 10 | 11 | 12 | 17 | 18 => skip(bytes, &mut offset, 4)?,
            15 => skip(bytes, &mut offset, 3)?,
            other => {
                return Err(CertError::Extension(format!(
                    "unsupported Java class constant tag {other}"
                )))
            }
        }
        index += 1;
    }
    Ok(constants)
}

fn read_u8(bytes: &[u8], offset: &mut usize) -> Result<u8, CertError> {
    let value = *bytes
        .get(*offset)
        .ok_or_else(|| CertError::Extension("truncated Java class".into()))?;
    *offset += 1;
    Ok(value)
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, CertError> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| CertError::Extension("Java class offset overflow".into()))?;
    let slice = bytes
        .get(*offset..end)
        .ok_or_else(|| CertError::Extension("truncated Java class".into()))?;
    *offset = end;
    let high = *slice
        .first()
        .ok_or_else(|| CertError::Extension("truncated Java class".into()))?;
    let low = *slice
        .get(1)
        .ok_or_else(|| CertError::Extension("truncated Java class".into()))?;
    Ok(u16::from_be_bytes([high, low]))
}

fn skip(bytes: &[u8], offset: &mut usize, length: usize) -> Result<(), CertError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| CertError::Extension("Java class offset overflow".into()))?;
    if end > bytes.len() {
        return Err(CertError::Extension("truncated Java class".into()));
    }
    *offset = end;
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>, CertError> {
    let response = ureq::get(url)
        .call()
        .map_err(|error| CertError::Network(format!("GET {url}: {error}")))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(CertError::Extension(format!(
            "{url} is larger than {} MiB",
            MAX_ARCHIVE_BYTES / (1024 * 1024)
        )));
    }
    Ok(bytes)
}

fn download_text(url: &str) -> Result<String, CertError> {
    let bytes = download(url)?;
    String::from_utf8(bytes)
        .map_err(|error| CertError::Extension(format!("{url} is not UTF-8: {error}")))
}

fn normalize_duids(duids: &[String]) -> Result<Vec<String>, CertError> {
    let mut unique = BTreeSet::new();
    for raw in duids {
        for part in raw.split(',') {
            let duid = part.trim();
            if duid.is_empty() {
                continue;
            }
            if duid.len() > 64
                || !duid.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                })
            {
                return Err(CertError::Config(format!(
                    "invalid Tizen DUID '{duid}'; expected ASCII letters, digits, '-' or '_'"
                )));
            }
            unique.insert(duid.to_string());
        }
    }
    if unique.is_empty() {
        return Err(CertError::Config(
            "at least one TV DUID is required (use --duid <DUID>)".to_string(),
        ));
    }
    if unique.len() > 50 {
        return Err(CertError::Config(
            "Samsung distributor certificates support at most 50 DUIDs".to_string(),
        ));
    }
    Ok(unique.into_iter().collect())
}

fn validate_email(email: &str) -> Result<(), CertError> {
    let email = email.trim();
    if email.len() > 254
        || !email.contains('@')
        || email.chars().any(|character| character.is_control())
    {
        return Err(CertError::Config(format!(
            "invalid Samsung account email '{email}'"
        )));
    }
    Ok(())
}

fn wait_for_login(
    login_url: &str,
    open_browser: bool,
    timeout: Duration,
) -> Result<Login, CertError> {
    let callback = callback_from_login_url(login_url)?;
    let listeners = bind_callback(callback.port)?;
    println!("Open this URL to sign in to your Samsung account:");
    println!("  {login_url}");
    if open_browser {
        if open_browser_url(login_url) {
            println!(
                "The browser has been opened. Waiting up to {} seconds...",
                timeout.as_secs()
            );
        } else {
            println!("Could not open a browser automatically; open the URL above.");
        }
    } else {
        println!(
            "Open the URL above in a browser; waiting up to {} seconds...",
            timeout.as_secs()
        );
    }

    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err(CertError::Login(format!(
                "timed out after {} seconds waiting for the Samsung login",
                timeout.as_secs()
            )));
        }
        for listener in &listeners {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(login) = handle_callback(stream, &callback.path)? {
                        return Ok(login);
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => {
                    return Err(CertError::Login(format!(
                        "cannot accept the Samsung login callback: {error}"
                    )))
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn callback_from_login_url(login_url: &str) -> Result<Callback, CertError> {
    let url = url::Url::parse(login_url)
        .map_err(|error| CertError::Login(format!("invalid Samsung login URL: {error}")))?;
    let redirect = url
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| CertError::Login("Samsung login URL has no redirect_uri".into()))?;
    let redirect = url::Url::parse(&redirect)
        .map_err(|error| CertError::Login(format!("invalid Samsung redirect URI: {error}")))?;
    let port = redirect
        .port_or_known_default()
        .ok_or_else(|| CertError::Login("Samsung redirect URI has no port".into()))?;
    let path = redirect.path().to_string();
    if path.is_empty() {
        return Err(CertError::Login(
            "Samsung redirect URI has no callback path".into(),
        ));
    }
    Ok(Callback { port, path })
}

fn bind_callback(port: u16) -> Result<Vec<TcpListener>, CertError> {
    let mut listeners = Vec::new();
    let mut errors = Vec::new();
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listeners.push(listener),
        Err(error) => errors.push(format!("127.0.0.1:{port}: {error}")),
    }
    match TcpListener::bind(("::1", port)) {
        Ok(listener) => listeners.push(listener),
        Err(error) => errors.push(format!("[::1]:{port}: {error}")),
    }
    if listeners.is_empty() {
        return Err(CertError::Login(format!(
            "cannot bind the Samsung login callback on port {port} ({}). Close Tizen Studio's Certificate Manager if it is running and retry.",
            errors.join("; ")
        )));
    }
    for listener in &listeners {
        listener.set_nonblocking(true)?;
    }
    Ok(listeners)
}

fn handle_callback(mut stream: TcpStream, expected_path: &str) -> Result<Option<Login>, CertError> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let request = match read_http_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = respond(&mut stream, 400, "Bad request");
            return Err(CertError::Login(format!(
                "cannot read the Samsung login callback: {error}"
            )));
        }
    };
    let (path, query) = request
        .target
        .split_once('?')
        .map_or((request.target.as_str(), ""), |(path, query)| (path, query));
    if path != expected_path {
        let _ = respond(&mut stream, 404, "Not found");
        return Ok(None);
    }
    let mut pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    if request.method.eq_ignore_ascii_case("POST") {
        pairs.extend(form_urlencoded::parse(&request.body).into_owned());
    }
    if let Some((_, error)) = pairs.iter().find(|(key, _)| key == "error") {
        let _ = respond(&mut stream, 400, "Samsung login failed");
        return Err(CertError::Login(format!("Samsung login returned: {error}")));
    }
    let Some((_, code)) = pairs.into_iter().find(|(key, _)| key == "code") else {
        let _ = respond(&mut stream, 400, "Missing Samsung login code");
        return Err(CertError::Login(
            "Samsung login callback did not contain a code".to_string(),
        ));
    };
    let login = parse_login_payload(&code)?;
    let _ = respond(
        &mut stream,
        200,
        "Samsung login received. You can close this tab and return to the terminal.",
    );
    Ok(Some(login))
}

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<HttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "connection closed before the request headers",
            ));
        }
        let Some(bytes) = chunk.get(..read) else {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "invalid HTTP read length",
            ));
        };
        buffer.extend_from_slice(bytes);
        if buffer.len() > MAX_HTTP_REQUEST_BYTES {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "HTTP request is too large",
            ));
        }
        let Some(header_end) = find_bytes(&buffer, b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(
            buffer
                .get(..header_end)
                .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidData, "invalid headers"))?,
        );
        let mut lines = header.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap_or_default().to_string();
        let target = request_parts.next().unwrap_or_default().to_string();
        let content_length = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .next()
            .unwrap_or(0);
        let body_start = header_end.saturating_add(4);
        let mut body = buffer
            .get(body_start..)
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidData, "invalid request body"))?
            .to_vec();
        while body.len() < content_length {
            let read = stream.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            let Some(bytes) = chunk.get(..read) else {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    "invalid HTTP read length",
                ));
            };
            body.extend_from_slice(bytes);
            if body.len() > MAX_HTTP_REQUEST_BYTES {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    "HTTP request body is too large",
                ));
            }
        }
        body.truncate(content_length.min(body.len()));
        return Ok(HttpRequest {
            method,
            target,
            body,
        });
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn respond(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    let body =
        format!("<!doctype html><meta charset=\"utf-8\"><title>Impresario</title><p>{message}</p>");
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

fn parse_login_payload(code: &str) -> Result<Login, CertError> {
    let mut candidate = code.to_string();
    for _ in 0..2 {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) {
            let access_token = value
                .get("access_token")
                .or_else(|| value.get("accessToken"))
                .and_then(serde_json::Value::as_str)
                .filter(|token| !token.is_empty())
                .ok_or_else(|| CertError::Login("Samsung login code has no access token".into()))?;
            let user_id = value
                .get("userId")
                .or_else(|| value.get("user_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CertError::Login("Samsung login code has no user id".into()))?;
            let email = value
                .get("email")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|email| !email.is_empty())
                .map(str::to_string);
            return Ok(Login {
                access_token: Zeroizing::new(access_token.to_string()),
                user_id: user_id.to_string(),
                email,
            });
        }
        let decoded = form_urlencoded::parse(format!("value={candidate}").as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        if decoded == candidate || decoded.is_empty() {
            break;
        }
        candidate = decoded;
    }
    Err(CertError::Login(
        "Samsung login callback returned an invalid code".to_string(),
    ))
}

fn open_browser_url(url: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        return Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .status()
            .is_ok();
    }
    #[cfg(target_os = "macos")]
    {
        return Command::new("open").arg(url).status().is_ok();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return Command::new("xdg-open").arg(url).status().is_ok();
    }
    #[allow(unreachable_code)]
    false
}

fn generate_material(
    email: &str,
    duids: &[String],
    distributor: bool,
) -> Result<GeneratedMaterial, CertError> {
    let key = RsaPrivateKey::new(&mut OsRng, 2048)
        .map_err(|error| CertError::Config(format!("generate RSA key: {error}")))?;
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| CertError::Config(format!("encode RSA key: {error}")))?;
    let key_pair = KeyPair::from_pem_and_sign_algo(pem.as_str(), &PKCS_RSA_SHA512)
        .map_err(|error| CertError::Config(format!("load RSA key: {error}")))?;
    let mut params = CertificateParams::new(Vec::<String>::new())
        .map_err(|error| CertError::Config(format!("prepare CSR: {error}")))?;
    if distributor {
        params
            .distinguished_name
            .push(DnType::CommonName, "TizenSDK");
        let email_value = DnValue::Ia5String(
            email
                .try_into()
                .map_err(|error| CertError::Config(format!("CSR email: {error}")))?,
        );
        params.distinguished_name.push(
            DnType::CustomDnType(vec![1, 2, 840, 113549, 1, 9, 1]),
            email_value,
        );
        params.subject_alt_names.push(SanType::URI(
            "URN:tizen:packageid="
                .try_into()
                .map_err(|error| CertError::Config(format!("CSR package id: {error}")))?,
        ));
        for duid in duids {
            let uri = format!("URN:tizen:deviceid={duid}");
            params.subject_alt_names.push(SanType::URI(
                uri.as_str()
                    .try_into()
                    .map_err(|error| CertError::Config(format!("CSR DUID: {error}")))?,
            ));
        }
    } else {
        params.distinguished_name.push(DnType::CommonName, email);
    }
    let csr = params
        .serialize_request(&key_pair)
        .map_err(|error| CertError::Config(format!("create CSR: {error}")))?;
    let csr_pem = csr
        .pem()
        .map_err(|error| CertError::Config(format!("encode CSR: {error}")))?;
    Ok(GeneratedMaterial {
        key_der: Zeroizing::new(key_pair.serialize_der()),
        csr_pem,
    })
}

fn request_certificate(
    url: &str,
    access_token: &str,
    user_id: &str,
    csr_pem: &str,
    distributor: bool,
) -> Result<Vec<u8>, CertError> {
    let boundary = format!("----impresario-{}", random_hex());
    let mut body = Vec::new();
    add_form_field(&mut body, &boundary, "access_token", access_token);
    add_form_field(&mut body, &boundary, "user_id", user_id);
    add_form_field(&mut body, &boundary, "platform", "VD");
    if distributor {
        add_form_field(&mut body, &boundary, "privilege_level", "Public");
        add_form_field(&mut body, &boundary, "developer_type", "Individual");
    }
    add_form_file(
        &mut body,
        &boundary,
        "csr",
        "request.csr",
        csr_pem.as_bytes(),
    );
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = ureq::post(url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send_bytes(&body);
    match response {
        Ok(response) => {
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(MAX_CERTIFICATE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.is_empty() {
                return Err(CertError::Certificate(
                    "Samsung returned an empty certificate response".into(),
                ));
            }
            if bytes.len() as u64 > MAX_CERTIFICATE_BYTES {
                return Err(CertError::Certificate(
                    "Samsung's certificate response is too large".into(),
                ));
            }
            Ok(bytes)
        }
        Err(ureq::Error::Status(status, response)) => {
            let mut detail = String::new();
            let _ = response
                .into_reader()
                .take(4096)
                .read_to_string(&mut detail);
            Err(CertError::Certificate(format!(
                "Samsung returned HTTP {status}{}",
                sanitized_detail(&detail)
            )))
        }
        Err(error) => Err(CertError::Network(format!(
            "Samsung certificate API request failed: {error}"
        ))),
    }
}

fn add_form_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

fn add_form_file(body: &mut Vec<u8>, boundary: &str, name: &str, filename: &str, bytes: &[u8]) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
}

fn sanitized_detail(detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty()
        || detail.to_ascii_lowercase().contains("access_token")
        || detail.to_ascii_lowercase().contains("bearer ")
    {
        return String::new();
    }
    let mut value: String = detail.chars().take(300).collect();
    value.retain(|character| !character.is_control());
    format!(": {value}")
}

fn certificate_chain(response: &[u8], ca_der: &[u8]) -> Result<Vec<Vec<u8>>, CertError> {
    let mut certificates = certificate_ders(response)?;
    if certificates.is_empty() {
        return Err(CertError::Certificate(
            "Samsung returned no certificate in the response".into(),
        ));
    }
    if !certificates.iter().any(|certificate| certificate == ca_der) {
        certificates.push(ca_der.to_vec());
    }
    Ok(certificates)
}

fn single_certificate(bytes: &[u8]) -> Result<Vec<u8>, CertError> {
    certificate_ders(bytes)?
        .into_iter()
        .next()
        .ok_or_else(|| CertError::Certificate("no certificate found in CA file".into()))
}

fn certificate_ders(bytes: &[u8]) -> Result<Vec<Vec<u8>>, CertError> {
    let text = String::from_utf8_lossy(bytes);
    let mut certificates = Vec::new();
    let mut rest = text.as_ref();
    while let Some(begin) = rest.find("-----BEGIN CERTIFICATE-----") {
        let after_begin = &rest[begin + "-----BEGIN CERTIFICATE-----".len()..];
        let Some(end) = after_begin.find("-----END CERTIFICATE-----") else {
            return Err(CertError::Certificate(
                "unterminated PEM certificate".into(),
            ));
        };
        let encoded: String = after_begin[..end]
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| CertError::Certificate(format!("PEM certificate: {error}")))?;
        certificates.push(der);
        rest = &after_begin[end + "-----END CERTIFICATE-----".len()..];
    }
    if certificates.is_empty() && bytes.starts_with(&[0x30]) {
        certificates.push(bytes.to_vec());
    }
    if certificates.is_empty() {
        return Err(CertError::Certificate(
            "Samsung returned an unrecognized certificate format".into(),
        ));
    }
    for certificate in &certificates {
        parse_x509_certificate(certificate)
            .map_err(|error| CertError::Certificate(format!("parse certificate: {error}")))?;
    }
    Ok(certificates)
}

fn certificate_duids(der: &[u8]) -> Result<Vec<String>, CertError> {
    let (_, certificate) = parse_x509_certificate(der).map_err(|error| {
        CertError::Certificate(format!("parse distributor certificate: {error}"))
    })?;
    let mut duids = Vec::new();
    if let Ok(Some(names)) = certificate.subject_alternative_name() {
        for name in &names.value.general_names {
            if let GeneralName::URI(uri) = name {
                if let Some(duid) = uri.strip_prefix("URN:tizen:deviceid=") {
                    if !duid.is_empty() {
                        duids.push(duid.to_string());
                    }
                }
            }
        }
    }
    Ok(duids)
}

fn build_pkcs12(
    key_der: &[u8],
    chain: &[Vec<u8>],
    password: &str,
    alias: &str,
) -> Result<Vec<u8>, CertError> {
    let private = PrivateKey::from_der(key_der)
        .map_err(|error| CertError::Certificate(format!("PKCS#12 key: {error}")))?;
    let certificates = chain
        .iter()
        .map(|der| {
            Certificate::from_der(der)
                .map_err(|error| CertError::Certificate(format!("PKCS#12 certificate: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let key_chain = PrivateKeyChain::new(LocalKeyId::from(alias.as_bytes()), private, certificates);
    let mut store = KeyStore::new();
    store.add_entry(alias, KeyStoreEntry::PrivateKeyChain(key_chain));
    store
        .writer(password)
        .encryption_algorithm(EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc)
        .encryption_iterations(P12_ITERATIONS)
        .mac_algorithm(MacAlgorithm::HmacSha1)
        .mac_iterations(P12_ITERATIONS)
        .write()
        .map_err(|error| CertError::Certificate(format!("write PKCS#12: {error}")))
}

fn random_password() -> String {
    let mut bytes = [0u8; 32];
    use rsa::rand_core::RngCore;
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    use rsa::rand_core::RngCore;
    OsRng.fill_bytes(&mut bytes);
    let mut value = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_with_constants(constants: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61]);
        bytes.extend_from_slice(&((constants.len() + 1) as u16).to_be_bytes());
        for constant in constants {
            bytes.push(1);
            bytes.extend_from_slice(&(constant.len() as u16).to_be_bytes());
            bytes.extend_from_slice(constant.as_bytes());
        }
        bytes
    }

    #[test]
    fn reads_java_constant_pool_pairs() {
        let class = class_with_constants(&[
            "SERVICE_ID",
            "service-123",
            "loginUrl",
            "https://account.example/check",
            "AUTHOR_URL",
            "https://ca.example/authors",
            "DISTRIBUTOR_URL",
            "https://ca.example/distributors",
        ]);
        let constants = class_constants(&class).unwrap();
        assert_eq!(
            pair_value(&constants, "SERVICE_ID").as_deref(),
            Some("service-123")
        );
        assert_eq!(
            pair_value(&constants, "AUTHOR_URL").as_deref(),
            Some("https://ca.example/authors")
        );
    }

    fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::{CompressionMethod, ZipWriter};

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn test_certificate_der() -> Vec<u8> {
        let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let key_pair = KeyPair::from_pem_and_sign_algo(pem.as_str(), &PKCS_RSA_SHA512).unwrap();
        let params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.self_signed(&key_pair).unwrap().der().to_vec()
    }

    #[test]
    fn parses_nested_extension_archive() {
        let class = class_with_constants(&[
            "SERVICE_ID",
            "service-123",
            "loginUrl",
            "https://account.example/check",
            "AUTHOR_URL",
            "https://ca.example/authors",
            "DISTRIBUTOR_URL",
            "https://ca.example/distributors",
        ]);
        let certificate = test_certificate_der();
        let jar = zip_bytes(&[
            ("org/tizen/common/cert/ui/SigninDialog.class", class.clone()),
            ("org/tizen/common/cert/model/CertConstant.class", class),
            ("res/ca/vd_tizen_dev_author_ca.cer", certificate.clone()),
            ("res/ca/vd_tizen_dev_public2.crt", certificate.clone()),
        ]);
        let inner = zip_bytes(&[("data/ide/plugins/org.tizen.common.cert_test.jar", jar)]);
        let outer = zip_bytes(&[("binary/cert-add-on_test_ubuntu-64.zip", inner)]);
        let parsed = parse_extension_archive(&outer).unwrap();
        assert_eq!(parsed.service_id, "service-123");
        assert_eq!(parsed.author_url, "https://ca.example/authors");
        assert_eq!(parsed.distributor_url, "https://ca.example/distributors");
        assert_eq!(parsed.author_ca_der, certificate);
    }

    #[test]
    fn finds_extension_repository() {
        let xml = r#"
            <extension_info>
              <extension>
                <name>Samsung Certificate Extension</name>
                <repository> https://example.test/cert.zip </repository>
              </extension>
            </extension_info>
        "#;
        assert_eq!(
            extension_repository(xml).as_deref(),
            Some("https://example.test/cert.zip")
        );
    }

    #[test]
    fn parses_callback_url_and_payload() {
        let login_url = "https://account.example/check.do?serviceID=x&redirect_uri=http%3A%2F%2Flocalhost%3A4794%2Fsignin%2Fcallback";
        let callback = callback_from_login_url(login_url).unwrap();
        assert_eq!(callback.port, 4794);
        assert_eq!(callback.path, "/signin/callback");
        let login = parse_login_payload(
            r#"{"access_token":"token","userId":"user","email":"dev@example.com"}"#,
        )
        .unwrap();
        assert_eq!(login.user_id, "user");
        assert_eq!(login.email.as_deref(), Some("dev@example.com"));
        assert_eq!(login.access_token.as_str(), "token");
    }

    #[test]
    fn rejects_bad_login_payload_without_echoing_it() {
        let error = parse_login_payload("not-json").unwrap_err().to_string();
        assert!(!error.contains("not-json"));
    }

    #[test]
    fn accepts_the_local_browser_callback() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .write_all(
                    b"GET /signin/callback?code=%7B%22access_token%22%3A%22token%22%2C%22userId%22%3A%22user%22%7D HTTP/1.1\r\nHost: localhost\r\n\r\n",
                )
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        let (server, _) = listener.accept().unwrap();
        let login = handle_callback(server, "/signin/callback")
            .unwrap()
            .expect("callback accepted");
        assert_eq!(login.user_id, "user");
        assert_eq!(login.access_token.as_str(), "token");
        let response = client.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn builds_legacy_pkcs12_that_imports() {
        let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let key_pair = KeyPair::from_pem_and_sign_algo(pem.as_str(), &PKCS_RSA_SHA512).unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(DnType::CommonName, "author");
        let certificate = params.self_signed(&key_pair).unwrap();
        let p12 = build_pkcs12(
            &key_pair.serialize_der(),
            &[certificate.der().to_vec()],
            "test-password",
            "usercertificate",
        )
        .unwrap();
        let store = KeyStore::from_pkcs12(
            &p12,
            "test-password",
            p12_keystore::Pkcs12ImportPolicy::Strict,
        )
        .unwrap();
        assert!(store.private_key_chain().is_some());
    }

    #[test]
    fn generates_distributor_csr_with_duid_sans() {
        use x509_parser::certification_request::X509CertificationRequest;
        use x509_parser::extensions::ParsedExtension;

        fn pem_body(pem: &str, label: &str) -> Vec<u8> {
            let begin = format!("-----BEGIN {label}-----");
            let end = format!("-----END {label}-----");
            let start = pem.find(&begin).unwrap() + begin.len();
            let stop = pem[start..].find(&end).unwrap() + start;
            let encoded: String = pem[start..stop]
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap()
        }

        let duids = vec!["DUID123".to_string()];
        let material = generate_material("dev@example.com", &duids, true).unwrap();
        let der = pem_body(&material.csr_pem, "CERTIFICATE REQUEST");
        let (_, csr) = X509CertificationRequest::from_der(&der).unwrap();
        let san = csr
            .requested_extensions()
            .unwrap()
            .find_map(|extension| match extension {
                ParsedExtension::SubjectAlternativeName(san) => Some(san),
                _ => None,
            })
            .expect("distributor CSR has SANs");
        assert!(san.general_names.iter().any(|name| {
            matches!(name, GeneralName::URI(uri) if *uri == "URN:tizen:deviceid=DUID123")
        }));
    }

    #[test]
    fn parses_pem_and_der_certificates() {
        let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let pem = key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let key_pair = KeyPair::from_pem_and_sign_algo(pem.as_str(), &PKCS_RSA_SHA512).unwrap();
        let params = CertificateParams::new(Vec::<String>::new()).unwrap();
        let certificate = params.self_signed(&key_pair).unwrap();
        let pem = certificate.pem();
        assert_eq!(certificate_ders(pem.as_bytes()).unwrap().len(), 1);
        assert_eq!(certificate_ders(certificate.der()).unwrap().len(), 1);
    }
}
