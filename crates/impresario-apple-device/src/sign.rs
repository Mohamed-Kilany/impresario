//! Code signing through the `apple-codesign` library.
//!
//! This is the same two-pass layout the physical tvOS P0 used: the provisioning
//! profile must be inside the bundle before signing, nested XCUITest bundles
//! must be signed before the outer runner, and the outer bundle is then sealed
//! shallowly so the nested bytes match their recorded seals.

use std::path::{Path, PathBuf};

use apple_codesign::cryptography::InMemoryPrivateKey;
use apple_codesign::{BundleSigner, SettingsScope, SigningSettings};
use p12_keystore::{KeyStore, Pkcs12ImportPolicy};
use uuid::Uuid;
use x509_certificate::CapturedX509Certificate;

use crate::{Error, Result};

/// A PKCS#12 signing identity. Its debug output never includes key material.
pub struct SigningIdentity {
    key: InMemoryPrivateKey,
    cert: CapturedX509Certificate,
    chain: Vec<CapturedX509Certificate>,
}

impl std::fmt::Debug for SigningIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningIdentity")
            .field("certificate", &"<redacted>")
            .field("chain", &self.chain.len())
            .finish_non_exhaustive()
    }
}

impl SigningIdentity {
    /// Whether two PKCS#12 files hold the same leaf certificate.
    ///
    /// Exported `.p12` files for one Apple development certificate can differ
    /// byte-for-byte (salt, MAC, chain order) while being the same identity.
    /// Impresario signs a product set with one certificate, so import accepts
    /// containers only when this check passes.
    #[must_use]
    pub fn same_certificate(&self, other: &Self) -> bool {
        self.cert == other.cert
    }
}

/// Load the first private-key chain from a PKCS#12 file.
pub fn load_identity(p12: &Path, password: &str) -> Result<SigningIdentity> {
    let data =
        std::fs::read(p12).map_err(|error| Error::io(format!("read {}", p12.display()), error))?;
    let store = KeyStore::from_pkcs12(&data, password, Pkcs12ImportPolicy::Strict)
        .map_err(|error| Error::Signing(format!("parse {}: {error}", p12.display())))?;
    let (_, keychain) = store
        .private_key_chain()
        .ok_or_else(|| Error::Signing(format!("{} has no private key chain", p12.display())))?;
    let key = InMemoryPrivateKey::from_pkcs8_der(keychain.key().as_der())
        .map_err(|error| Error::Signing(format!("load private key: {error}")))?;
    let certs = keychain.certs();
    let Some(first) = certs.first() else {
        return Err(Error::Signing(format!(
            "{} has no certificate",
            p12.display()
        )));
    };
    let cert = CapturedX509Certificate::from_der(first.as_der())
        .map_err(|error| Error::Signing(format!("load certificate: {error}")))?;
    let mut chain = Vec::with_capacity(certs.len().saturating_sub(1));
    for cert in certs.iter().skip(1) {
        chain.push(
            CapturedX509Certificate::from_der(cert.as_der())
                .map_err(|error| Error::Signing(format!("load chain certificate: {error}")))?,
        );
    }
    Ok(SigningIdentity { key, cert, chain })
}

/// The optional nested UI-test bundle inside the runner app.
#[derive(Debug, Clone)]
pub struct NestedSigning {
    /// Relative path inside the outer app, e.g. `PlugIns/Foo.xctest`.
    pub relative_path: String,
    /// Provisioning profile for the nested bundle.
    pub profile: PathBuf,
    /// Entitlements for the nested bundle.
    pub entitlements: PathBuf,
}

/// One app to sign.
#[derive(Debug, Clone)]
pub struct SignRequest {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub profile: PathBuf,
    pub entitlements: PathBuf,
    pub nested: Option<NestedSigning>,
}

/// Sign `request` with `identity` and `team_id`.
pub fn sign_app(request: &SignRequest, identity: &SigningIdentity, team_id: &str) -> Result<()> {
    let source = &request.source;
    let destination = &request.destination;
    if !source.is_dir() {
        return Err(Error::Signing(format!(
            "{} is not an app bundle",
            source.display()
        )));
    }
    for path in [&request.profile, &request.entitlements] {
        if !path.is_file() {
            return Err(Error::Signing(format!(
                "signing material {} is missing",
                path.display()
            )));
        }
    }
    let parent = destination.parent().ok_or_else(|| {
        Error::Signing(format!("{} has no parent directory", destination.display()))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::io(format!("create {}", parent.display()), error))?;
    let work = parent.join(format!(".impresario-sign-{}", Uuid::new_v4()));
    let _guard = RemoveOnDrop(work.clone());
    let stage = work.join("stage").join(
        source
            .file_name()
            .ok_or_else(|| Error::Signing(format!("{} has no name", source.display())))?,
    );
    copy_dir_all(source, &stage)?;
    remove_code_signature_dirs(&stage)?;
    std::fs::copy(&request.profile, stage.join("embedded.mobileprovision"))
        .map_err(|error| Error::io("copy outer provisioning profile", error))?;
    if let Some(nested) = &request.nested {
        let nested_dir = stage.join(&nested.relative_path);
        if !nested_dir.is_dir() {
            return Err(Error::Signing(format!(
                "nested bundle {} is missing from {}",
                nested.relative_path,
                source.display()
            )));
        }
        for path in [&nested.profile, &nested.entitlements] {
            if !path.is_file() {
                return Err(Error::Signing(format!(
                    "nested signing material {} is missing",
                    path.display()
                )));
            }
        }
        std::fs::copy(&nested.profile, nested_dir.join("embedded.mobileprovision"))
            .map_err(|error| Error::io("copy nested provisioning profile", error))?;

        let pass1 = work.join("pass1");
        sign_bundle(
            &stage,
            &pass1,
            &request.entitlements,
            Some((&nested.relative_path, &nested.entitlements)),
            identity,
            team_id,
            false,
        )?;
        let pass2_source = work.join("pass2-source");
        copy_dir_all(&pass1, &pass2_source)?;
        sign_bundle(
            &pass2_source,
            destination,
            &request.entitlements,
            None,
            identity,
            team_id,
            true,
        )?;
    } else {
        sign_bundle(
            &stage,
            destination,
            &request.entitlements,
            None,
            identity,
            team_id,
            false,
        )?;
    }
    Ok(())
}

fn sign_bundle(
    source: &Path,
    destination: &Path,
    entitlements: &Path,
    nested: Option<(&str, &Path)>,
    identity: &SigningIdentity,
    team_id: &str,
    shallow: bool,
) -> Result<()> {
    if destination.exists() {
        std::fs::remove_dir_all(destination)
            .map_err(|error| Error::io(format!("remove {}", destination.display()), error))?;
    }
    let mut signer = BundleSigner::new_from_path(source)
        .map_err(|error| Error::Signing(format!("read {}: {error}", source.display())))?;
    signer
        .collect_nested_bundles()
        .map_err(|error| Error::Signing(format!("collect nested bundles: {error}")))?;
    let mut settings = base_settings(identity, team_id);
    settings.set_shallow(shallow);
    for pattern in ["**/*.dSYM", "**/*.dSYM/**", "*.dSYM", "*.dSYM/**"] {
        settings
            .add_path_exclusion(pattern)
            .map_err(|error| Error::Signing(format!("exclude {pattern}: {error}")))?;
    }
    settings
        .set_entitlements_xml(
            SettingsScope::Main,
            plist_xml(entitlements)?.to_writer_xml_string()?,
        )
        .map_err(|error| Error::Signing(format!("set entitlements: {error}")))?;
    if let Some((relative, nested_entitlements)) = nested {
        settings
            .set_entitlements_xml(
                SettingsScope::Path(relative.to_string()),
                plist_xml(nested_entitlements)?.to_writer_xml_string()?,
            )
            .map_err(|error| Error::Signing(format!("set nested entitlements: {error}")))?;
    }
    signer
        .write_signed_bundle(destination, &settings)
        .map(|_| ())
        .map_err(|error| {
            Error::Signing(format!(
                "sign {} -> {}: {error}",
                source.display(),
                destination.display()
            ))
        })
}

fn base_settings<'a>(identity: &'a SigningIdentity, team_id: &str) -> SigningSettings<'a> {
    let mut settings = SigningSettings::default();
    settings.set_signing_key(&identity.key, identity.cert.clone());
    for cert in &identity.chain {
        settings.chain_certificate(cert.clone());
    }
    if settings.chain_apple_certificates().is_none() {
        tracing::debug!("could not add Apple intermediate certificates");
    }
    settings.set_team_id(team_id);
    settings.set_shallow(false);
    settings
}

fn plist_xml(path: &Path) -> Result<plist::Value> {
    plist::Value::from_file(path)
        .map_err(|error| Error::Signing(format!("read {}: {error}", path.display())))
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)
        .map_err(|error| Error::io(format!("create {}", destination.display()), error))?;
    let entries = std::fs::read_dir(source)
        .map_err(|error| Error::io(format!("read {}", source.display()), error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| Error::io(format!("read {}", source.display()), error))?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| Error::io(format!("stat {}", path.display()), error))?;
        if file_type.is_dir() {
            if entry.file_name().to_string_lossy().ends_with(".dSYM") {
                continue;
            }
            copy_dir_all(&path, &target)?;
        } else if file_type.is_symlink() {
            #[cfg(unix)]
            {
                let link = std::fs::read_link(&path)
                    .map_err(|error| Error::io(format!("readlink {}", path.display()), error))?;
                std::os::unix::fs::symlink(&link, &target)
                    .map_err(|error| Error::io(format!("symlink {}", target.display()), error))?;
            }
            #[cfg(windows)]
            std::fs::copy(&path, &target)
                .map_err(|error| Error::io(format!("copy {}", target.display()), error))?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|error| Error::io(format!("copy {}", target.display()), error))?;
        }
    }
    Ok(())
}

fn remove_code_signature_dirs(root: &Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    let mut doomed = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| Error::io(format!("stat {}", entry.path().display()), error))?;
            if file_type.is_dir() {
                if entry.file_name() == "_CodeSignature" {
                    doomed.push(entry.path());
                } else {
                    stack.push(entry.path());
                }
            }
        }
    }
    for path in doomed {
        std::fs::remove_dir_all(&path)
            .map_err(|error| Error::io(format!("remove {}", path.display()), error))?;
    }
    Ok(())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

trait PlistXmlString {
    fn to_writer_xml_string(&self) -> Result<String>;
}

impl PlistXmlString for plist::Value {
    fn to_writer_xml_string(&self) -> Result<String> {
        let mut buffer = Vec::new();
        self.to_writer_xml(&mut buffer)
            .map_err(|error| Error::Signing(format!("serialize entitlements: {error}")))?;
        String::from_utf8(buffer)
            .map_err(|error| Error::Signing(format!("entitlements are not UTF-8: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_pkcs12_is_reported_without_a_password_in_the_message() {
        let error = load_identity(Path::new("missing-identity.p12"), "do-not-print-me")
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing-identity.p12"));
        assert!(!error.contains("do-not-print-me"));
    }
}
