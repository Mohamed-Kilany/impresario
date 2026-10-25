//! Tizen `.wgt` packaging, W3C widget XML-DSig signing, profile storage and
//! verification (TASK-2 6.4, 7.3, 7.4). Pure Rust: no `tizen`, `openssl` or
//! key-manager process (D-19).
//!
//! The signature layout follows the Tizen signing tool and the MIT-licensed
//! `tizen-sdb` `TizenResigner`: author signature over every original entry
//! plus `#prop`, then distributor signature over `author-signature.xml`, every
//! original entry and its own `#prop`. Digests are SHA-512; `SignedInfo` is
//! signed with RSA PKCS#1 v1.5 / SHA-512 or ECDSA P-256/P-384 / SHA-512.
//!
//! Hardware and Samsung certificate-service acceptance are **not** claimed.
//! The golden tests verify our deterministic output against W3C rules and
//! documented fixed property digests.

#![allow(clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use impresario_core::home::Home;
use p12_keystore::{Certificate, KeyStore, Pkcs12ImportPolicy};
use rsa::pkcs1v15::{Signature as RsaSignature, SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePublicKey};
use rsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha512};
use ua_cryptography_xml_c14n::{canonicalize, Algorithm as C14nAlgorithm, DocumentText};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::*;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

const DSIG_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
const DSIG_PROP_NS: &str = "http://www.w3.org/2009/xmldsig-properties";
const RSA_SHA512: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha512";
const ECDSA_SHA512: &str = "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha512";
const SHA512_URI: &str = "http://www.w3.org/2001/04/xmlenc#sha512";
const EXC_C14N: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
const C14N11: &str = "http://www.w3.org/2006/12/xml-c14n11";
const AUTHOR_SIGNATURE: &str = "author-signature.xml";
const DISTRIBUTOR_SIGNATURE: &str = "signature1.xml";
const AUTHOR_PROP_DIGEST: &str =
    "aXbSAVgmAz0GsBUeZ1UmNDRrxkWhDUVGb45dZcNRq429wX3X+x6kaXT3NdNDTSNVTU+ypkysPMGvQY10fG1EWQ==";
const DISTRIBUTOR_PROP_DIGEST: &str =
    "/r5npk2VVA46QFJnejgONBEh4BWtjrtu9x/IFeLksjWyGmB/cMWKSJWQl7aU3YRQRZ3AesG8gF7qGyvKX9Snig==";
const KEYRING_SERVICE: &str = "impresario.tizen";

/// Signing/packaging failures. Callers turn these into `IMP-SIGN-*` output.
#[derive(Debug, thiserror::Error)]
pub enum SignError {
    #[error("IMP-SIGN-001: {0}")]
    Package(String),
    #[error("IMP-SIGN-002: signing profile '{0}' was not found")]
    ProfileNotFound(String),
    #[error("IMP-SIGN-003: PKCS#12 import failed: {0}")]
    Pkcs12(String),
    #[error("IMP-SIGN-004: certificate has expired or is not yet valid: {0}")]
    Expired(String),
    #[error("IMP-SIGN-005: signing key is unsupported or does not match its certificate: {0}")]
    UnsupportedKey(String),
    #[error("IMP-SIGN-006: DUID '{duid}' is not covered by the distributor certificate")]
    DuidNotCovered { duid: String },
    #[error("IMP-SIGN-007: signature validation failed: {0}")]
    Invalid(String),
    #[error("IMP-SIGN-008: profile password is unavailable: {0}")]
    Password(String),
    #[error("IMP-SIGN-009: {0}")]
    Config(String),
    #[error("IMP-SIGN-010: I/O: {0}")]
    Io(#[from] std::io::Error),
}

impl From<zip::result::ZipError> for SignError {
    fn from(value: zip::result::ZipError) -> Self {
        SignError::Package(format!("zip: {value}"))
    }
}

/// Public information about one imported profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileRecord {
    pub name: String,
    pub author_subject: String,
    pub distributor_subject: String,
    pub author_not_after: String,
    pub distributor_not_after: String,
    /// Unix seconds; `doctor` uses this without opening the PKCS#12 files.
    #[serde(default)]
    pub author_not_after_unix: i64,
    #[serde(default)]
    pub distributor_not_after_unix: i64,
    pub duids: Vec<String>,
    pub created_unix_ms: u64,
}

/// Metadata for a `.wgt` created by `build_wgt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgtInfo {
    pub id: String,
    pub version: String,
    pub entry_count: usize,
}

/// Private key types accepted in Tizen PKCS#12 profiles. Samsung profiles can
/// carry RSA or NIST P-256/P-384 keys; signatures are always SHA-512 and
/// ECDSA signatures are emitted as raw `r||s`, as XML-DSig requires.
enum SigningKeyMaterial {
    Rsa(Box<RsaPrivateKey>),
    P256(p256::SecretKey),
    P384(p384::SecretKey),
}

impl SigningKeyMaterial {
    fn load(der: &[u8]) -> Result<Self, SignError> {
        if let Ok(key) = RsaPrivateKey::from_pkcs8_der(der) {
            return Ok(Self::Rsa(Box::new(key)));
        }
        if let Ok(key) = p256::SecretKey::from_pkcs8_der(der) {
            return Ok(Self::P256(key));
        }
        if let Ok(key) = p384::SecretKey::from_pkcs8_der(der) {
            return Ok(Self::P384(key));
        }
        Err(SignError::UnsupportedKey(
            "expected an RSA, P-256 or P-384 PKCS#8 private key".to_string(),
        ))
    }

    fn public_key_der(&self) -> Result<Vec<u8>, SignError> {
        let der = match self {
            Self::Rsa(key) => key
                .to_public_key()
                .to_public_key_der()
                .map_err(|error| SignError::UnsupportedKey(error.to_string()))?,
            Self::P256(key) => key
                .public_key()
                .to_public_key_der()
                .map_err(|error| SignError::UnsupportedKey(error.to_string()))?,
            Self::P384(key) => key
                .public_key()
                .to_public_key_der()
                .map_err(|error| SignError::UnsupportedKey(error.to_string()))?,
        };
        Ok(der.as_bytes().to_vec())
    }

    fn algorithm(&self) -> &'static str {
        match self {
            Self::Rsa(_) => RSA_SHA512,
            Self::P256(_) | Self::P384(_) => ECDSA_SHA512,
        }
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, SignError> {
        match self {
            Self::Rsa(key) => {
                let signing_key = SigningKey::<Sha512>::new(key.as_ref().clone());
                Ok(signing_key.sign(data).to_vec())
            }
            Self::P256(key) => {
                let signing_key = p256::ecdsa::SigningKey::from(key);
                let digest = Sha512::digest(data);
                let signature: p256::ecdsa::Signature = signing_key
                    .sign_prehash(&digest)
                    .map_err(|error| SignError::UnsupportedKey(error.to_string()))?;
                Ok(signature.to_bytes().to_vec())
            }
            Self::P384(key) => {
                let signing_key = p384::ecdsa::SigningKey::from(key);
                let digest = Sha512::digest(data);
                let signature: p384::ecdsa::Signature = signing_key
                    .sign_prehash(&digest)
                    .map_err(|error| SignError::UnsupportedKey(error.to_string()))?;
                Ok(signature.to_bytes().to_vec())
            }
        }
    }
}

struct SigningCertificate {
    key: SigningKeyMaterial,
    /// Leaf first, then non-root issuers.
    chain: Vec<Certificate>,
}

impl SigningCertificate {
    fn load(p12: &[u8], password: &str, label: &str) -> Result<Self, SignError> {
        let store = KeyStore::from_pkcs12(p12, password, Pkcs12ImportPolicy::Strict)
            .map_err(|error| SignError::Pkcs12(format!("{label}: {error}")))?;
        let (_, chain) = store
            .private_key_chain()
            .ok_or_else(|| SignError::Pkcs12(format!("{label}: no private key chain")))?;
        let key = SigningKeyMaterial::load(chain.key().as_der())
            .map_err(|error| SignError::UnsupportedKey(format!("{label}: {error}")))?;
        let certs = chain.certs();
        let leaf = certs
            .first()
            .ok_or_else(|| SignError::Pkcs12(format!("{label}: no leaf certificate")))?;
        let (_, leaf_parsed) = parse_x509_certificate(leaf.as_der())
            .map_err(|error| SignError::Pkcs12(format!("{label}: {error}")))?;
        let certificate_key = leaf_parsed.public_key().raw;
        let private_key = key
            .public_key_der()
            .map_err(|error| SignError::UnsupportedKey(format!("{label}: {error}")))?;
        if certificate_key != private_key.as_slice() {
            return Err(SignError::UnsupportedKey(format!(
                "{label}: the private key does not match the leaf certificate"
            )));
        }
        let mut output = vec![leaf.clone()];
        for cert in certs.iter().skip(1) {
            if cert.as_der() == leaf.as_der() {
                continue;
            }
            let (_, parsed) = parse_x509_certificate(cert.as_der())
                .map_err(|error| SignError::Pkcs12(format!("{label}: {error}")))?;
            if parsed.subject() == parsed.issuer() {
                continue; // roots are not included in KeyInfo
            }
            output.push(cert.clone());
        }
        Ok(Self { key, chain: output })
    }

    fn leaf(&self) -> Result<&Certificate, SignError> {
        self.chain
            .first()
            .ok_or_else(|| SignError::Pkcs12("empty certificate chain".to_string()))
    }

    fn leaf_info(&self) -> Result<(String, String, i64, Vec<String>), SignError> {
        let leaf = self.leaf()?;
        let (_, cert) = parse_x509_certificate(leaf.as_der())
            .map_err(|error| SignError::Pkcs12(error.to_string()))?;
        let subject = cert.subject().to_string();
        let not_after = cert.validity().not_after.to_string();
        let not_after_unix = cert.validity().not_after.timestamp();
        let mut duids = Vec::new();
        if let Ok(Some(names)) = cert.subject_alternative_name() {
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
        Ok((subject, not_after, not_after_unix, duids))
    }

    fn ensure_valid(&self, label: &str) -> Result<(), SignError> {
        let leaf = self.leaf()?;
        let (_, cert) = parse_x509_certificate(leaf.as_der())
            .map_err(|error| SignError::Pkcs12(error.to_string()))?;
        if !cert.validity().is_valid() {
            return Err(SignError::Expired(format!(
                "{label}: valid {} .. {}",
                cert.validity().not_before,
                cert.validity().not_after
            )));
        }
        Ok(())
    }
}

/// One loaded profile, with private material only in memory.
pub struct LoadedProfile {
    pub record: ProfileRecord,
    author: SigningCertificate,
    distributor: SigningCertificate,
}

impl LoadedProfile {
    /// Validate that the distributor certificate contains the TV's DUID.
    pub fn ensure_duid(&self, duid: &str) -> Result<(), SignError> {
        if self.record.duids.iter().any(|candidate| candidate == duid) {
            return Ok(());
        }
        Err(SignError::DuidNotCovered {
            duid: duid.to_string(),
        })
    }

    /// Validate the profile before use.
    pub fn ensure_valid(&self) -> Result<(), SignError> {
        self.author.ensure_valid("author certificate")?;
        self.distributor.ensure_valid("distributor certificate")?;
        Ok(())
    }
}

/// Read one existing `.wgt` into memory without extracting it.
pub fn read_wgt(path: &Path) -> Result<Vec<u8>, SignError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 4 || &bytes[..2] != b"PK" {
        return Err(SignError::Package(format!(
            "{} is not a zip/.wgt archive",
            path.display()
        )));
    }
    Ok(bytes)
}

/// Inspect `config.xml` inside a `.wgt`.
pub fn inspect_wgt(path: &Path) -> Result<WgtInfo, SignError> {
    let bytes = read_wgt(path)?;
    inspect_wgt_bytes(&bytes)
}

fn inspect_wgt_bytes(bytes: &[u8]) -> Result<WgtInfo, SignError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut config = None;
    let mut entry_count = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        entry_count += 1;
        if entry.name() == "config.xml" {
            let mut text = String::new();
            entry
                .read_to_string(&mut text)
                .map_err(|error| SignError::Package(format!("read config.xml: {error}")))?;
            config = Some(text);
        }
    }
    let config = config.ok_or_else(|| SignError::Package("config.xml is missing".to_string()))?;
    let (id, version) = parse_widget_id(&config)?;
    Ok(WgtInfo {
        id,
        version,
        entry_count,
    })
}

fn parse_widget_id(config: &str) -> Result<(String, String), SignError> {
    let doc = roxmltree::Document::parse(config)
        .map_err(|error| SignError::Config(format!("config.xml: {error}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "widget" {
        return Err(SignError::Config(
            "config.xml root must be <widget>".to_string(),
        ));
    }
    let id = root
        .attribute("id")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SignError::Config("config.xml has no widget id".to_string()))?;
    let version = root.attribute("version").unwrap_or("0.0.0");
    Ok((id.to_string(), version.to_string()))
}

/// Build a deterministic unsigned `.wgt` from a packaged web-app directory.
/// Entries are sorted by path, timestamps are fixed, and existing signature
/// files are omitted so a later signing pass is the only signature source.
pub fn build_wgt(source: &Path, output: &Path) -> Result<WgtInfo, SignError> {
    let config_path = source.join("config.xml");
    let config = std::fs::read_to_string(&config_path)
        .map_err(|error| SignError::Package(format!("read {}: {error}", config_path.display())))?;
    let (id, version) = parse_widget_id(&config)?;
    let mut files = Vec::new();
    collect_files(source, source, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    if !files.iter().any(|(name, _)| name == "config.xml") {
        return Err(SignError::Package(
            "source must contain config.xml at its root".to_string(),
        ));
    }
    let mut entries = Vec::with_capacity(files.len());
    for (name, path) in &files {
        if name == AUTHOR_SIGNATURE || name == DISTRIBUTOR_SIGNATURE {
            continue;
        }
        let data = std::fs::read(path)?;
        entries.push((name.clone(), data));
    }
    let bytes = zip_entries(&entries)?;
    write_atomic(output, &bytes)?;
    Ok(WgtInfo {
        id,
        version,
        entry_count: entries.len(),
    })
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), SignError> {
    let mut children = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(SignError::Package(format!(
                "refusing to package symlink {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| SignError::Package(format!("{} is outside source", path.display())))?
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((relative, path));
        }
    }
    Ok(())
}

fn zip_entries(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, SignError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
        .map_err(|error| SignError::Package(format!("zip timestamp: {error}")))?;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(timestamp)
        .unix_permissions(0o644);
    for (name, data) in entries {
        writer.start_file(name, options)?;
        writer.write_all(data)?;
    }
    let cursor = writer.finish()?;
    Ok(cursor.into_inner())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SignError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Sign an existing `.wgt` with separate author and distributor profiles.
/// Existing signature entries are dropped and regenerated.
pub fn sign_wgt(
    wgt: &[u8],
    author_p12: &[u8],
    author_password: &str,
    distributor_p12: &[u8],
    distributor_password: &str,
) -> Result<Vec<u8>, SignError> {
    let author = SigningCertificate::load(author_p12, author_password, "author")?;
    let distributor =
        SigningCertificate::load(distributor_p12, distributor_password, "distributor")?;
    author.ensure_valid("author certificate")?;
    distributor.ensure_valid("distributor certificate")?;
    sign_wgt_with_certificates(wgt, &author, &distributor)
}

fn sign_wgt_with_certificates(
    wgt: &[u8],
    author: &SigningCertificate,
    distributor: &SigningCertificate,
) -> Result<Vec<u8>, SignError> {
    let originals = original_entries(wgt)?;
    let author_signature = build_signature("AuthorSignature", &originals, author)?;
    let mut distributor_inputs = Vec::with_capacity(originals.len() + 1);
    distributor_inputs.push((AUTHOR_SIGNATURE.to_string(), author_signature.clone()));
    distributor_inputs.extend(originals.iter().cloned());
    let distributor_signature =
        build_signature("DistributorSignature", &distributor_inputs, distributor)?;
    let mut output = Vec::with_capacity(originals.len() + 2);
    output.push((AUTHOR_SIGNATURE.to_string(), author_signature));
    output.push((DISTRIBUTOR_SIGNATURE.to_string(), distributor_signature));
    output.extend(originals);
    zip_entries(&output)
}

fn original_entries(wgt: &[u8]) -> Result<Vec<(String, Vec<u8>)>, SignError> {
    let mut archive = ZipArchive::new(Cursor::new(wgt))?;
    let mut entries = Vec::with_capacity(archive.len());
    let mut seen = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if name == AUTHOR_SIGNATURE || name == DISTRIBUTOR_SIGNATURE {
            continue;
        }
        if !seen.insert(name.clone()) {
            return Err(SignError::Package(format!("duplicate zip entry '{name}'")));
        }
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;
        entries.push((name, data));
    }
    if !seen.contains("config.xml") {
        return Err(SignError::Package("config.xml is missing".to_string()));
    }
    // Signing must be a pure function of the entry set, not of the order in
    // which an upstream packager happened to write the input zip.
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

#[derive(Debug, Clone)]
struct Reference {
    uri: String,
    digest: String,
}

fn build_signature(
    id: &str,
    inputs: &[(String, Vec<u8>)],
    certificate: &SigningCertificate,
) -> Result<Vec<u8>, SignError> {
    let role = if id == "AuthorSignature" {
        "author"
    } else {
        "distributor"
    };
    let prop_digest = if role == "author" {
        AUTHOR_PROP_DIGEST
    } else {
        DISTRIBUTOR_PROP_DIGEST
    };
    let mut references = Vec::with_capacity(inputs.len() + 1);
    for (name, data) in inputs {
        references.push(Reference {
            uri: name.clone(),
            digest: base64_encode(&Sha512::digest(data)),
        });
    }
    references.push(Reference {
        uri: "#prop".to_string(),
        digest: prop_digest.to_string(),
    });
    let signed_info = canonical_signed_info(&references, certificate.key.algorithm());
    let signature = certificate.key.sign(signed_info.as_bytes())?;
    let signature64 = base64_encode(&signature);

    let mut xml = String::new();
    xml.push_str(&format!("<Signature xmlns=\"{DSIG_NS}\" Id=\"{id}\">"));
    xml.push_str(&signed_info);
    xml.push_str(&format!(
        "<SignatureValue>{signature64}</SignatureValue><KeyInfo><X509Data>"
    ));
    for cert in &certificate.chain {
        xml.push_str(&format!(
            "<X509Certificate>{}</X509Certificate>",
            certificate_text(cert.as_der())
        ));
    }
    xml.push_str("</X509Data></KeyInfo>");
    xml.push_str(&canonical_prop_object(id, role));
    xml.push_str("</Signature>");
    Ok(xml.into_bytes())
}

fn canonical_signed_info(references: &[Reference], signature_method: &str) -> String {
    let mut output = String::new();
    output.push_str(&format!("<SignedInfo xmlns=\"{DSIG_NS}\">"));
    output.push_str(&format!(
        "<CanonicalizationMethod Algorithm=\"{EXC_C14N}\"></CanonicalizationMethod>"
    ));
    output.push_str(&format!(
        "<SignatureMethod Algorithm=\"{signature_method}\"></SignatureMethod>"
    ));
    for reference in references {
        let uri = if reference.uri == "#prop" {
            // A same-document fragment is part of the XML-DSig reference
            // syntax, not a file name; percent-encoding '#' would turn it
            // into a different URI (%23prop).
            reference.uri.clone()
        } else {
            percent_encode_uri(&reference.uri)
        };
        output.push_str(&format!("<Reference URI=\"{}\">", xml_escape(&uri)));
        if reference.uri == "#prop" {
            output.push_str(&format!(
                "<Transforms><Transform Algorithm=\"{C14N11}\"></Transform></Transforms>"
            ));
        }
        output.push_str(&format!(
            "<DigestMethod Algorithm=\"{SHA512_URI}\"></DigestMethod>\
             <DigestValue>{}</DigestValue></Reference>",
            reference.digest
        ));
    }
    output.push_str("</SignedInfo>");
    output
}

fn canonical_prop_object(id: &str, role: &str) -> String {
    format!(
        "<Object xmlns=\"{DSIG_NS}\" Id=\"prop\"><SignatureProperties \
         xmlns:dsp=\"{DSIG_PROP_NS}\"><SignatureProperty Id=\"profile\" \
         Target=\"#{id}\"><dsp:Profile \
         URI=\"http://www.w3.org/ns/widgets-digsig#profile\"></dsp:Profile>\
         </SignatureProperty><SignatureProperty Id=\"role\" Target=\"#{id}\">\
         <dsp:Role URI=\"http://www.w3.org/ns/widgets-digsig#role-{role}\">\
         </dsp:Role></SignatureProperty><SignatureProperty Id=\"identifier\" \
         Target=\"#{id}\"><dsp:Identifier></dsp:Identifier></SignatureProperty>\
         </SignatureProperties></Object>"
    )
}

fn certificate_text(der: &[u8]) -> String {
    let encoded = base64_encode(der);
    let mut wrapped = String::with_capacity(encoded.len() + encoded.len() / 76 + 4);
    wrapped.push('\n');
    for chunk in encoded.as_bytes().chunks(76) {
        wrapped.push_str(&String::from_utf8_lossy(chunk));
        wrapped.push('\n');
    }
    wrapped
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// RFC 3986 path encoding for a Reference URI. Unreserved characters and `/`
/// stay literal; everything else (including spaces, `#`, `?` and non-ASCII
/// UTF-8 bytes) becomes `%HH`.
fn percent_encode_uri(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn percent_decode_uri(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            let high = char::from(high).to_digit(16)?;
            let low = char::from(low).to_digit(16)?;
            output.push(u8::try_from(high * 16 + low).ok()?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Verify our own signed `.wgt`: every original file and `#prop` is referenced
/// by both signatures, digests match, and both `SignatureValue`s verify with
/// the certificate embedded in the corresponding signature.
pub fn validate_signed_wgt(bytes: &[u8]) -> Result<(), SignError> {
    let entries = original_entries(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut author = None;
    let mut distributor = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        match entry.name() {
            AUTHOR_SIGNATURE => {
                let mut text = String::new();
                entry.read_to_string(&mut text)?;
                author = Some(text);
            }
            DISTRIBUTOR_SIGNATURE => {
                let mut text = String::new();
                entry.read_to_string(&mut text)?;
                distributor = Some(text);
            }
            _ => {}
        }
    }
    let author =
        author.ok_or_else(|| SignError::Invalid("author-signature.xml is missing".to_string()))?;
    let distributor =
        distributor.ok_or_else(|| SignError::Invalid("signature1.xml is missing".to_string()))?;
    let mut distributor_entries = vec![(AUTHOR_SIGNATURE.to_string(), author.as_bytes().to_vec())];
    distributor_entries.extend(entries.iter().cloned());
    verify_signature(&author, "AuthorSignature", &entries, AUTHOR_PROP_DIGEST)?;
    verify_signature(
        &distributor,
        "DistributorSignature",
        &distributor_entries,
        DISTRIBUTOR_PROP_DIGEST,
    )?;
    Ok(())
}

fn verify_signature(
    signature_xml: &str,
    expected_id: &str,
    entries: &[(String, Vec<u8>)],
    prop_digest: &str,
) -> Result<(), SignError> {
    let doc = roxmltree::Document::parse(signature_xml)
        .map_err(|error| SignError::Invalid(format!("{expected_id}: XML: {error}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "Signature" {
        return Err(SignError::Invalid(format!(
            "{expected_id}: root is not Signature"
        )));
    }
    if root.attribute("Id") != Some(expected_id) {
        return Err(SignError::Invalid(format!(
            "{expected_id}: wrong Id {:?}",
            root.attribute("Id")
        )));
    }
    let signed_info = root
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "SignedInfo")
        .ok_or_else(|| SignError::Invalid(format!("{expected_id}: no SignedInfo")))?;
    let mut referenced = BTreeSet::new();
    for reference in signed_info
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "Reference")
    {
        let uri = reference
            .attribute("URI")
            .ok_or_else(|| SignError::Invalid(format!("{expected_id}: Reference without URI")))?;
        let digest = reference
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "DigestValue")
            .and_then(|node| node.text())
            .unwrap_or_default()
            .trim();
        let decoded_uri = percent_decode_uri(uri).ok_or_else(|| {
            SignError::Invalid(format!(
                "{expected_id}: malformed percent-encoded URI '{uri}'"
            ))
        })?;
        if decoded_uri == "#prop" {
            if digest != prop_digest {
                return Err(SignError::Invalid(format!(
                    "{expected_id}: #prop digest is {digest}, expected {prop_digest}"
                )));
            }
            continue;
        }
        let data = entries
            .iter()
            .find(|(name, _)| name == &decoded_uri)
            .map(|(_, data)| data)
            .ok_or_else(|| {
                SignError::Invalid(format!("{expected_id}: reference '{uri}' has no entry"))
            })?;
        let expected = base64_encode(&Sha512::digest(data));
        if digest != expected {
            return Err(SignError::Invalid(format!(
                "{expected_id}: digest mismatch for '{uri}'"
            )));
        }
        referenced.insert(decoded_uri);
    }
    for (name, _) in entries {
        if !referenced.contains(name) {
            return Err(SignError::Invalid(format!(
                "{expected_id}: '{name}' is not referenced"
            )));
        }
    }
    let certificate = root
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "X509Certificate")
        .and_then(|node| node.text())
        .ok_or_else(|| SignError::Invalid(format!("{expected_id}: no X509Certificate")))?;
    let der = base64::engine::general_purpose::STANDARD
        .decode(certificate.split_whitespace().collect::<String>())
        .map_err(|error| {
            SignError::Invalid(format!("{expected_id}: certificate base64: {error}"))
        })?;
    let (_, parsed) = parse_x509_certificate(&der)
        .map_err(|error| SignError::Invalid(format!("{expected_id}: certificate: {error}")))?;
    let signature_method = signed_info
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "SignatureMethod")
        .and_then(|node| node.attribute("Algorithm"))
        .ok_or_else(|| SignError::Invalid(format!("{expected_id}: no SignatureMethod")))?;
    let canonicalization = signed_info
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "CanonicalizationMethod")
        .and_then(|node| node.attribute("Algorithm"))
        .ok_or_else(|| SignError::Invalid(format!("{expected_id}: no CanonicalizationMethod")))?;
    if canonicalization != EXC_C14N {
        return Err(SignError::Invalid(format!(
            "{expected_id}: unsupported SignedInfo canonicalization {canonicalization}"
        )));
    }
    // Canonicalize the parsed SignedInfo subtree instead of trusting the
    // original whitespace. Exclusive C14N drops unused inherited namespaces.
    let canonical = DocumentText::decode(signature_xml.as_bytes())
        .map_err(|error| SignError::Invalid(format!("{expected_id}: XML: {error}")))?
        .parse()
        .map(|document| {
            document
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "SignedInfo")
                .map(|node| canonicalize(node, C14nAlgorithm::Exclusive, &[], None))
        })
        .ok()
        .flatten()
        .ok_or_else(|| SignError::Invalid(format!("{expected_id}: no canonical SignedInfo")))?;
    let signature = root
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "SignatureValue")
        .and_then(|node| node.text())
        .ok_or_else(|| SignError::Invalid(format!("{expected_id}: no SignatureValue")))?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature.trim())
        .map_err(|error| SignError::Invalid(format!("{expected_id}: signature base64: {error}")))?;
    match signature_method {
        RSA_SHA512 => {
            let public =
                RsaPublicKey::from_public_key_der(parsed.public_key().raw).map_err(|error| {
                    SignError::Invalid(format!("{expected_id}: RSA public key: {error}"))
                })?;
            let signature = RsaSignature::try_from(signature.as_slice()).map_err(|error| {
                SignError::Invalid(format!("{expected_id}: signature: {error}"))
            })?;
            let verifying = VerifyingKey::<Sha512>::new(public);
            verifying.verify(&canonical, &signature).map_err(|error| {
                SignError::Invalid(format!(
                    "{expected_id}: SignatureValue does not verify: {error}"
                ))
            })?;
        }
        ECDSA_SHA512 => {
            let digest = Sha512::digest(&canonical);
            let verified = if let Ok(public) =
                p256::ecdsa::VerifyingKey::from_public_key_der(parsed.public_key().raw)
            {
                p256::ecdsa::Signature::from_slice(&signature)
                    .ok()
                    .is_some_and(|signature| public.verify_prehash(&digest, &signature).is_ok())
            } else if let Ok(public) =
                p384::ecdsa::VerifyingKey::from_public_key_der(parsed.public_key().raw)
            {
                p384::ecdsa::Signature::from_slice(&signature)
                    .ok()
                    .is_some_and(|signature| public.verify_prehash(&digest, &signature).is_ok())
            } else {
                return Err(SignError::Invalid(format!(
                    "{expected_id}: certificate is not a P-256 or P-384 key"
                )));
            };
            if !verified {
                return Err(SignError::Invalid(format!(
                    "{expected_id}: ECDSA SignatureValue does not verify"
                )));
            }
        }
        other => {
            return Err(SignError::Invalid(format!(
                "{expected_id}: unsupported SignatureMethod {other}"
            )));
        }
    }
    Ok(())
}

/// Create/import a profile from two encrypted PKCS#12 files. The encrypted
/// files are copied under `signing/tizen/<name>/`; passwords go to the OS
/// keychain and are never written to YAML or JSON.
pub fn import_profile(
    home: &Home,
    name: &str,
    author_p12: &Path,
    author_password: &str,
    distributor_p12: &Path,
    distributor_password: &str,
) -> Result<ProfileRecord, SignError> {
    validate_profile_name(name)?;
    let author_bytes = std::fs::read(author_p12)?;
    let distributor_bytes = std::fs::read(distributor_p12)?;
    let author = SigningCertificate::load(&author_bytes, author_password, "author")?;
    let distributor =
        SigningCertificate::load(&distributor_bytes, distributor_password, "distributor")?;
    author.ensure_valid("author certificate")?;
    distributor.ensure_valid("distributor certificate")?;
    let (author_subject, author_not_after, author_not_after_unix, _) = author.leaf_info()?;
    let (distributor_subject, distributor_not_after, distributor_not_after_unix, duids) =
        distributor.leaf_info()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let record = ProfileRecord {
        name: name.to_string(),
        author_subject,
        distributor_subject,
        author_not_after,
        distributor_not_after,
        author_not_after_unix,
        distributor_not_after_unix,
        duids,
        created_unix_ms: now,
    };
    let directory = profile_dir(home, name);
    let parent = directory
        .parent()
        .ok_or_else(|| SignError::Config("profile path has no parent".to_string()))?;
    std::fs::create_dir_all(parent)?;
    // Import into a staging directory first so a failed certificate write or
    // an unavailable keychain does not leave a profile that lists but cannot
    // sign. The old profile (if any) is only replaced once passwords are
    // stored successfully.
    let staging = parent.join(format!(".{name}.{}.import", std::process::id()));
    if staging.is_dir() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    // Preserve the previous keychain entries so any later failure can restore
    // the old profile instead of leaving it listed but unusable.
    let old_author = read_password(name, "author");
    let old_distributor = read_password(name, "distributor");
    let staged = (|| -> Result<(), SignError> {
        std::fs::write(staging.join("author.p12"), &author_bytes)?;
        std::fs::write(staging.join("distributor.p12"), &distributor_bytes)?;
        std::fs::write(
            staging.join("profile.json"),
            serde_json::to_vec_pretty(&record)
                .map_err(|error| SignError::Config(format!("serialize profile: {error}")))?,
        )?;
        set_private_permissions(&staging.join("author.p12"));
        set_private_permissions(&staging.join("distributor.p12"));
        store_password(name, "author", author_password)?;
        store_password(name, "distributor", distributor_password)?;
        Ok(())
    })();
    if let Err(error) = staged {
        let _ = std::fs::remove_dir_all(&staging);
        restore_password(name, "author", old_author.as_deref());
        restore_password(name, "distributor", old_distributor.as_deref());
        return Err(error);
    }
    let backup = parent.join(format!(".{name}.{}.backup", std::process::id()));
    if backup.is_dir() {
        std::fs::remove_dir_all(&backup)?;
    }
    let had_old = directory.is_dir();
    if had_old {
        if let Err(error) = std::fs::rename(&directory, &backup) {
            restore_password(name, "author", old_author.as_deref());
            restore_password(name, "distributor", old_distributor.as_deref());
            let _ = std::fs::remove_dir_all(&staging);
            return Err(SignError::Io(error));
        }
    }
    if let Err(error) = std::fs::rename(&staging, &directory) {
        if had_old {
            let _ = std::fs::rename(&backup, &directory);
        }
        let _ = std::fs::remove_dir_all(&staging);
        restore_password(name, "author", old_author.as_deref());
        restore_password(name, "distributor", old_distributor.as_deref());
        return Err(SignError::Io(error));
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(record)
}

/// List imported profiles, sorted by name.
pub fn list_profiles(home: &Home) -> Result<Vec<ProfileRecord>, SignError> {
    let root = home.signing_dir().join("tizen");
    let mut records = Vec::new();
    if !root.is_dir() {
        return Ok(records);
    }
    let mut entries = std::fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("profile.json");
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(record) = serde_json::from_slice::<ProfileRecord>(&bytes) {
                records.push(record);
            }
        }
    }
    Ok(records)
}

/// Show one imported profile (no secrets).
pub fn show_profile(home: &Home, name: &str) -> Result<ProfileRecord, SignError> {
    validate_profile_name(name)?;
    let path = profile_dir(home, name).join("profile.json");
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SignError::ProfileNotFound(name.to_string())
        } else {
            SignError::Io(error)
        }
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|error| SignError::Config(format!("parse {}: {error}", path.display())))
}

/// Remove a profile, its certificates and keychain passwords.
pub fn remove_profile(home: &Home, name: &str) -> Result<(), SignError> {
    validate_profile_name(name)?;
    let directory = profile_dir(home, name);
    if !directory.is_dir() {
        return Err(SignError::ProfileNotFound(name.to_string()));
    }
    std::fs::remove_dir_all(directory)?;
    let _ = delete_password(name, "author");
    let _ = delete_password(name, "distributor");
    Ok(())
}

/// Load private material for signing. Passwords come from
/// `IMPRESARIO_TIZEN_AUTHOR_PASSWORD` / `..._DISTRIBUTOR_PASSWORD` for
/// automation, otherwise from the OS keychain.
pub fn load_profile(home: &Home, name: &str) -> Result<LoadedProfile, SignError> {
    let record = show_profile(home, name)?;
    let directory = profile_dir(home, name);
    let author_p12 = std::fs::read(directory.join("author.p12"))?;
    let distributor_p12 = std::fs::read(directory.join("distributor.p12"))?;
    let author_password = profile_password(name, "author")?;
    let distributor_password = profile_password(name, "distributor")?;
    let author = SigningCertificate::load(&author_p12, &author_password, "author")?;
    let distributor =
        SigningCertificate::load(&distributor_p12, &distributor_password, "distributor")?;
    Ok(LoadedProfile {
        record,
        author,
        distributor,
    })
}

/// Sign for a specific device, verify the result and cache it by
/// `(package SHA-256, profile import, DUID)`. The returned path is safe to
/// reuse. Re-importing a profile changes its import timestamp, so cached
/// signatures made with replaced certificates are never reused.
pub fn sign_package_for_device(
    home: &Home,
    profile_name: &str,
    wgt_path: &Path,
    duid: &str,
) -> Result<PathBuf, SignError> {
    let profile = load_profile(home, profile_name)?;
    profile.ensure_valid()?;
    profile.ensure_duid(duid)?;
    let bytes = read_wgt(wgt_path)?;
    let source_hash = hex(&sha2::Sha256::digest(&bytes));
    let cache = home.cache_dir().join("tizen-sign");
    std::fs::create_dir_all(&cache)?;
    let output = cache.join(format!(
        "{}-{}-{}-{}.wgt",
        &source_hash[..16.min(source_hash.len())],
        sanitize(name_segment(profile_name)),
        sanitize(duid),
        profile.record.created_unix_ms
    ));
    if output.is_file() {
        let cached = std::fs::read(&output)?;
        if validate_signed_wgt(&cached).is_ok() {
            return Ok(output);
        }
    }
    let signed = sign_wgt_with_certificates(&bytes, &profile.author, &profile.distributor)?;
    validate_signed_wgt(&signed)?;
    write_atomic(&output, &signed)?;
    Ok(output)
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub fn profile_dir(home: &Home, name: &str) -> PathBuf {
    home.signing_dir().join("tizen").join(sanitize(name))
}

fn validate_profile_name(name: &str) -> Result<(), SignError> {
    if name.is_empty()
        || name.len() > 64
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(SignError::Config(
            "profile names use 1-64 ASCII letters, digits, '.', '-' or '_'".to_string(),
        ));
    }
    Ok(())
}

fn name_segment(value: &str) -> &str {
    value
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn keyring_user(name: &str, side: &str) -> String {
    format!("profile:{name}:{side}")
}

fn store_password(name: &str, side: &str, password: &str) -> Result<(), SignError> {
    keyring::Entry::new(KEYRING_SERVICE, &keyring_user(name, side))
        .and_then(|entry| entry.set_password(password))
        .map_err(|error| {
            SignError::Password(format!(
                "cannot store {side} password in the OS keychain: {error}; \
                 set IMPRESARIO_TIZEN_{side_upper}_PASSWORD for a headless session",
                side_upper = side.to_ascii_uppercase()
            ))
        })
}

fn delete_password(name: &str, side: &str) -> Result<(), keyring::Error> {
    keyring::Entry::new(KEYRING_SERVICE, &keyring_user(name, side))?.delete_credential()
}

fn read_password(name: &str, side: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, &keyring_user(name, side))
        .and_then(|entry| entry.get_password())
        .ok()
}

/// Best-effort rollback of one keychain entry to its previous value.
fn restore_password(name: &str, side: &str, previous: Option<&str>) {
    match previous {
        Some(password) => {
            let _ = store_password(name, side, password);
        }
        None => {
            let _ = delete_password(name, side);
        }
    }
}

fn profile_password(name: &str, side: &str) -> Result<String, SignError> {
    let variable = format!("IMPRESARIO_TIZEN_{}_PASSWORD", side.to_ascii_uppercase());
    if let Ok(password) = std::env::var(&variable) {
        if !password.is_empty() {
            return Ok(password);
        }
    }
    keyring::Entry::new(KEYRING_SERVICE, &keyring_user(name, side))
        .and_then(|entry| entry.get_password())
        .map_err(|error| {
            SignError::Password(format!(
                "cannot read {side} password from the OS keychain: {error}; \
                 set {variable} for a headless session"
            ))
        })
}

fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use p12_keystore::{KeyStoreEntry, LocalKeyId, PrivateKey, PrivateKeyChain};

    #[test]
    fn prop_canonical_digest_matches_tizen_constants() {
        let author = canonical_prop_object("AuthorSignature", "author");
        let digest = base64_encode(&Sha512::digest(author.as_bytes()));
        assert_eq!(digest, AUTHOR_PROP_DIGEST);
        let distributor = canonical_prop_object("DistributorSignature", "distributor");
        let digest = base64_encode(&Sha512::digest(distributor.as_bytes()));
        assert_eq!(digest, DISTRIBUTOR_PROP_DIGEST);
    }

    #[test]
    fn widget_id_and_version_are_required() {
        let (id, version) = parse_widget_id(
            r#"<widget xmlns="http://www.w3.org/ns/widgets" id="abc.App" version="1.2.3"/>"#,
        )
        .expect("valid widget");
        assert_eq!(id, "abc.App");
        assert_eq!(version, "1.2.3");
        assert!(parse_widget_id("<widget version=\"1\"/>").is_err());
    }

    #[test]
    fn build_and_sign_roundtrip_with_generated_rsa_certificates(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("app");
        std::fs::create_dir_all(source.join("js"))?;
        std::fs::write(
            source.join("config.xml"),
            "<widget xmlns=\"http://www.w3.org/ns/widgets\" id=\"org.example.app\" version=\"1.0.0\"><name>Example</name></widget>",
        )?;
        std::fs::write(
            source.join("index.html"),
            "<!doctype html><title>Example</title>",
        )?;
        std::fs::write(source.join("js/app.js"), "console.log('ok');")?;
        let unsigned = temporary.path().join("app.wgt");
        let info = build_wgt(&source, &unsigned)?;
        assert_eq!(info.id, "org.example.app");
        assert_eq!(info.entry_count, 3);
        let bytes = std::fs::read(&unsigned)?;

        let (author_p12, author_password) = test_p12("author", "URN:tizen:deviceid=TESTDUID1234")?;
        let (distributor_p12, distributor_password) =
            test_p12("distributor", "URN:tizen:deviceid=TESTDUID1234")?;
        let signed = sign_wgt(
            &bytes,
            &author_p12,
            &author_password,
            &distributor_p12,
            &distributor_password,
        )?;
        validate_signed_wgt(&signed)?;
        assert_ne!(signed, bytes);
        Ok(())
    }

    #[test]
    fn packaging_is_deterministic_and_signature_references_are_encoded(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("app");
        std::fs::create_dir_all(&source)?;
        std::fs::write(
            source.join("config.xml"),
            "<widget xmlns=\"http://www.w3.org/ns/widgets\" id=\"org.example.app\" version=\"1.0.0\"/>",
        )?;
        std::fs::write(source.join("index page.html"), "<!doctype html>")?;
        let first = temporary.path().join("first.wgt");
        let second = temporary.path().join("second.wgt");
        let info = build_wgt(&source, &first)?;
        build_wgt(&source, &second)?;
        assert_eq!(info.entry_count, 2);
        assert_eq!(std::fs::read(&first)?, std::fs::read(&second)?);

        let mut archive = ZipArchive::new(Cursor::new(std::fs::read(&first)?))?;
        let original_names = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .map(|entry| entry.name().to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(original_names, vec!["config.xml", "index page.html"]);

        let (author_p12, author_password) = test_p12("author", "URN:tizen:deviceid=TESTDUID1234")?;
        let (distributor_p12, distributor_password) =
            test_p12("distributor", "URN:tizen:deviceid=TESTDUID1234")?;
        let signed = sign_wgt(
            &std::fs::read(&first)?,
            &author_p12,
            &author_password,
            &distributor_p12,
            &distributor_password,
        )?;
        validate_signed_wgt(&signed)?;
        let mut archive = ZipArchive::new(Cursor::new(&signed))?;
        let names = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .map(|entry| entry.name().to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            names,
            vec![
                AUTHOR_SIGNATURE,
                DISTRIBUTOR_SIGNATURE,
                "config.xml",
                "index page.html"
            ]
        );
        let mut signature = String::new();
        archive
            .by_name(AUTHOR_SIGNATURE)?
            .read_to_string(&mut signature)?;
        let document = roxmltree::Document::parse(&signature)?;
        let references = document
            .descendants()
            .filter(|node| node.has_tag_name((DSIG_NS, "Reference")))
            .filter_map(|node| node.attribute("URI"))
            .collect::<Vec<_>>();
        assert!(references.contains(&"index%20page.html"));
        assert!(references.contains(&"#prop"));
        assert!(!references.iter().any(|uri| uri.contains(' ')));
        Ok(())
    }

    #[test]
    fn ecdsa_p256_and_p384_profiles_sign_and_verify() -> Result<(), Box<dyn std::error::Error>> {
        use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384};
        let temporary = tempfile::tempdir()?;
        let unsigned = make_unsigned(&temporary)?;
        let bytes = std::fs::read(&unsigned)?;
        for algorithm in [&PKCS_ECDSA_P256_SHA256, &PKCS_ECDSA_P384_SHA384] {
            let (author_p12, author_password) =
                test_ec_p12("author", "URN:tizen:deviceid=TESTDUID1234", algorithm)?;
            let (distributor_p12, distributor_password) =
                test_ec_p12("distributor", "URN:tizen:deviceid=TESTDUID1234", algorithm)?;
            let signed = sign_wgt(
                &bytes,
                &author_p12,
                &author_password,
                &distributor_p12,
                &distributor_password,
            )?;
            validate_signed_wgt(&signed)?;
            let mut archive = ZipArchive::new(Cursor::new(&signed))?;
            let mut signature = String::new();
            archive
                .by_name(AUTHOR_SIGNATURE)?
                .read_to_string(&mut signature)?;
            assert!(signature.contains(ECDSA_SHA512));
            assert!(signature.contains("Id=\"AuthorSignature\""));
            assert!(KeyPair::generate_for(algorithm).is_ok());
        }
        Ok(())
    }

    #[test]
    fn tampering_with_an_entry_fails_validation() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let unsigned = make_unsigned(&temporary)?;
        let bytes = std::fs::read(&unsigned)?;
        let (author_p12, author_password) = test_p12("author", "URN:tizen:deviceid=TESTDUID1234")?;
        let (distributor_p12, distributor_password) =
            test_p12("distributor", "URN:tizen:deviceid=TESTDUID1234")?;
        let signed = sign_wgt(
            &bytes,
            &author_p12,
            &author_password,
            &distributor_p12,
            &distributor_password,
        )?;
        let mut archive = ZipArchive::new(Cursor::new(signed))?;
        let mut entries = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name().to_string();
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            if name == "index.html" {
                data.extend_from_slice(b"tampered");
            }
            entries.push((name, data));
        }
        drop(archive);
        let tampered = zip_entries(&entries)?;
        assert!(validate_signed_wgt(&tampered).is_err());
        Ok(())
    }

    fn make_unsigned(temporary: &tempfile::TempDir) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let source = temporary.path().join("app");
        std::fs::create_dir_all(&source)?;
        std::fs::write(
            source.join("config.xml"),
            "<widget xmlns=\"http://www.w3.org/ns/widgets\" id=\"org.example.app\" version=\"1.0.0\"/>",
        )?;
        std::fs::write(source.join("index.html"), "<!doctype html>")?;
        let unsigned = temporary.path().join("app.wgt");
        build_wgt(&source, &unsigned)?;
        Ok(unsigned)
    }

    fn test_p12(
        common_name: &str,
        uri: &str,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
        use rcgen::{KeyPair, PKCS_RSA_SHA256};
        use rsa::pkcs8::EncodePrivateKey as _;
        let rsa_key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048)?;
        let rsa_der = rsa_key.to_pkcs8_der()?;
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64_encode(rsa_der.as_bytes())
        );
        let key = KeyPair::from_pem_and_sign_algo(&pem, &PKCS_RSA_SHA256)?;
        store_p12(common_name, uri, &key)
    }

    fn test_ec_p12(
        common_name: &str,
        uri: &str,
        algorithm: &'static rcgen::SignatureAlgorithm,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
        let key = rcgen::KeyPair::generate_for(algorithm)?;
        store_p12(common_name, uri, &key)
    }

    fn store_p12(
        common_name: &str,
        uri: &str,
        key: &rcgen::KeyPair,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
        use rcgen::{CertificateParams, DnType, SanType};
        let mut params = CertificateParams::new(Vec::<String>::new())?;
        params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        params.subject_alt_names.push(SanType::URI(uri.try_into()?));
        let certificate = params.self_signed(key)?;
        let cert = Certificate::from_der(certificate.der())?;
        let private = PrivateKey::from_der(&key.serialize_der())?;
        let chain = PrivateKeyChain::new(
            LocalKeyId::from(common_name.as_bytes()),
            private,
            vec![cert],
        );
        let mut store = KeyStore::new();
        store.add_entry(common_name, KeyStoreEntry::PrivateKeyChain(chain));
        let password = "test-password";
        let bytes = store.writer(password).write()?;
        Ok((bytes, password.to_string()))
    }
}
