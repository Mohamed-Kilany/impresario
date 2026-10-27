//! Machine-global signing profiles for the Impresario Apple agent.
//!
//! A signing set owns one PKCS#12 identity and one provisioning profile plus
//! entitlements per bundle id. The directory layout is deliberately boring so a
//! profile can be inspected, backed up and regenerated without a database:
//!
//! ```text
//! ~/.impresario/apple/signing/default/
//!   team-id
//!   identity.p12
//!   bundles/
//!     com.impresario.agent/
//!       profile.mobileprovision
//!       entitlements.plist
//!     com.impresario.agent.tests/
//!       ...
//! ```
//!
//! The PKCS#12 password is never written to disk in plaintext. It comes from
//! `IMPRESARIO_APPLE_P12_PASSWORD` for headless runs, otherwise from the OS
//! credential store under `impresario.apple` / `signing:<name>`.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use cryptographic_message_syntax::SignedData;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::sign::{load_identity, SigningIdentity};
use crate::{Error, Result};

/// OS credential-store service shared with the Apple account session.
pub const KEYRING_SERVICE: &str = "impresario.apple";
/// Environment override for the PKCS#12 password (never logged).
pub const PASSWORD_ENV: &str = "IMPRESARIO_APPLE_P12_PASSWORD";
/// Default signing set name.
pub const DEFAULT_SIGNING_SET: &str = "default";

/// Provisioning-profile facts used by `doctor` and import validation.
#[derive(Debug, Clone, Default)]
pub struct ProfileInfo {
    pub name: Option<String>,
    pub uuid: Option<String>,
    pub team_id: Option<String>,
    /// Bundle id or wildcard pattern from `application-identifier`.
    pub bundle_id: Option<String>,
    /// Full `TEAM.bundle` value from the profile.
    pub application_identifier: Option<String>,
    pub expiration: Option<String>,
    pub platforms: Vec<String>,
    pub provisioned_devices: Vec<String>,
    pub get_task_allow: Option<bool>,
}

impl ProfileInfo {
    /// Parse a `.mobileprovision` (CMS-wrapped plist) or a plain plist profile.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|error| Error::io(format!("read {}", path.display()), error))?;
        let plist_bytes = profile_plist_bytes(&bytes)
            .map_err(|error| Error::Signing(format!("{}: {error}", path.display())))?;
        let value = plist::Value::from_reader(Cursor::new(plist_bytes))
            .map_err(|error| Error::Signing(format!("{}: {error}", path.display())))?;
        let dict = value.as_dictionary().ok_or_else(|| {
            Error::Signing(format!("{} is not a plist dictionary", path.display()))
        })?;
        let entitlements = dict
            .get("Entitlements")
            .and_then(plist::Value::as_dictionary);
        let team_id = dict
            .get("TeamIdentifier")
            .and_then(plist::Value::as_array)
            .and_then(|array| array.first())
            .and_then(plist::Value::as_string)
            .map(str::to_string)
            .or_else(|| {
                entitlements
                    .and_then(|dict| dict.get("com.apple.developer.team-identifier"))
                    .and_then(plist::Value::as_string)
                    .map(str::to_string)
            });
        let application_identifier = entitlements
            .and_then(|dict| dict.get("application-identifier"))
            .and_then(plist::Value::as_string)
            .map(str::to_string);
        let bundle_id = application_identifier.as_deref().and_then(|identifier| {
            identifier
                .split_once('.')
                .map(|(_, bundle)| bundle.to_string())
        });
        let get_task_allow = entitlements
            .and_then(|dict| dict.get("get-task-allow"))
            .and_then(plist::Value::as_boolean);
        Ok(Self {
            name: string_key(dict, "Name"),
            uuid: string_key(dict, "UUID"),
            team_id,
            bundle_id,
            application_identifier,
            expiration: dict
                .get("ExpirationDate")
                .and_then(plist::Value::as_date)
                .map(|date| date.to_xml_format()),
            platforms: string_array(dict, "Platform"),
            provisioned_devices: string_array(dict, "ProvisionedDevices"),
            get_task_allow,
        })
    }

    /// Whether the profile's app id covers `bundle_id` (including a trailing `*`).
    #[must_use]
    pub fn allows_bundle(&self, bundle_id: &str) -> bool {
        self.bundle_id
            .as_deref()
            .is_some_and(|pattern| bundle_pattern_matches(pattern, bundle_id))
    }

    /// Whether `udid` is listed. Profiles without a device list (managed
    /// distribution) allow every device and are not rejected here.
    #[must_use]
    pub fn allows_device(&self, udid: &str) -> bool {
        self.provisioned_devices.is_empty()
            || self
                .provisioned_devices
                .iter()
                .any(|value| value.eq_ignore_ascii_case(udid))
    }
}

fn string_key(dict: &plist::Dictionary, key: &str) -> Option<String> {
    dict.get(key)
        .and_then(plist::Value::as_string)
        .map(str::to_string)
}

fn string_array(dict: &plist::Dictionary, key: &str) -> Vec<String> {
    dict.get(key)
        .and_then(plist::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(plist::Value::as_string)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn profile_plist_bytes(bytes: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if bytes.starts_with(b"<?xml") || bytes.starts_with(b"bplist") {
        return Ok(bytes.to_vec());
    }
    let signed = SignedData::parse_ber(bytes).map_err(|error| error.to_string())?;
    signed
        .signed_content()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "CMS signature has no signed content".to_string())
}

/// One profile and its entitlements.
#[derive(Debug, Clone)]
pub struct BundleMaterials {
    pub profile: PathBuf,
    pub entitlements: PathBuf,
    pub info: ProfileInfo,
}

/// The on-disk directory for a named signing set.
///
/// Names are normalized exactly like [`SigningSet::load`], so callers can
/// check for an existing set without loading it.
#[must_use]
pub fn set_dir(signing_root: &Path, name: &str) -> PathBuf {
    signing_root.join(safe_key(name))
}

/// One PKCS#12 identity plus per-bundle signing materials.
#[derive(Debug, Clone)]
pub struct SigningSet {
    root: PathBuf,
    name: String,
    team_id: String,
    identity_p12: PathBuf,
    bundles: BTreeMap<String, BundleMaterials>,
}

impl SigningSet {
    /// Load `signing_root/<safe name>`.
    pub fn load(signing_root: &Path, name: &str) -> Result<Self> {
        Self::from_dir(&set_dir(signing_root, name), name)
    }

    /// Load an explicit signing-set directory.
    pub fn from_dir(dir: &Path, name: &str) -> Result<Self> {
        if !dir.is_dir() {
            return Err(Error::Signing(format!(
                "signing set '{}' is missing; run `impresario apple signing import <dir>`",
                dir.display()
            )));
        }
        let team_id = read_trimmed(&dir.join("team-id"))?;
        if team_id.is_empty() {
            return Err(Error::Signing(format!(
                "{} is empty",
                dir.join("team-id").display()
            )));
        }
        let identity_p12 = dir.join("identity.p12");
        if !identity_p12.is_file() {
            return Err(Error::Signing(format!(
                "{} is missing",
                identity_p12.display()
            )));
        }
        let bundles_dir = dir.join("bundles");
        if !bundles_dir.is_dir() {
            return Err(Error::Signing(format!(
                "{} has no bundles/ directory",
                dir.display()
            )));
        }
        let mut bundles = BTreeMap::new();
        for entry in std::fs::read_dir(&bundles_dir)
            .map_err(|error| Error::io(format!("read {}", bundles_dir.display()), error))?
        {
            let entry = entry
                .map_err(|error| Error::io(format!("read {}", bundles_dir.display()), error))?;
            if !entry
                .file_type()
                .map_err(|error| Error::io("stat signing bundle", error))?
                .is_dir()
            {
                continue;
            }
            let bundle_dir = entry.path();
            let profile = bundle_dir.join("profile.mobileprovision");
            let entitlements = bundle_dir.join("entitlements.plist");
            for path in [&profile, &entitlements] {
                if !path.is_file() {
                    return Err(Error::Signing(format!(
                        "{} is missing in signing bundle {}",
                        path.file_name().map_or_else(
                            || path.display().to_string(),
                            |name| name.to_string_lossy().to_string()
                        ),
                        bundle_dir.display()
                    )));
                }
            }
            let info = ProfileInfo::read(&profile)?;
            let Some(bundle_id) = info.bundle_id.clone() else {
                return Err(Error::Signing(format!(
                    "{} has no Entitlements.application-identifier",
                    profile.display()
                )));
            };
            if let Some(profile_team) = info.team_id.as_deref() {
                if profile_team != team_id {
                    return Err(Error::Signing(format!(
                        "{} belongs to team {profile_team}, but signing set '{}' uses {team_id}",
                        profile.display(),
                        self_name_hint(dir)
                    )));
                }
            }
            if bundles
                .insert(
                    bundle_id.clone(),
                    BundleMaterials {
                        profile,
                        entitlements,
                        info,
                    },
                )
                .is_some()
            {
                return Err(Error::Signing(format!(
                    "signing set '{}' has two profiles for {bundle_id}",
                    dir.display()
                )));
            }
        }
        if bundles.is_empty() {
            return Err(Error::Signing(format!(
                "{} has no bundle profiles under bundles/",
                dir.display()
            )));
        }
        Ok(Self {
            root: dir.to_path_buf(),
            name: name.to_string(),
            team_id,
            identity_p12,
            bundles,
        })
    }

    /// The signing-set directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The Apple developer team id.
    #[must_use]
    pub fn team_id(&self) -> &str {
        &self.team_id
    }

    /// The PKCS#12 identity path.
    #[must_use]
    pub fn identity_p12(&self) -> &Path {
        &self.identity_p12
    }

    /// All bundle ids with materials.
    pub fn bundle_ids(&self) -> impl Iterator<Item = &str> {
        self.bundles.keys().map(String::as_str)
    }

    /// Materials for one bundle id, with a useful list when absent.
    ///
    /// An exact profile key wins; otherwise the longest matching wildcard
    /// (`com.example.*`) is used.
    pub fn require(&self, bundle_id: &str) -> Result<&BundleMaterials> {
        if let Some(materials) = self.bundles.get(bundle_id) {
            return Ok(materials);
        }
        let wildcard = self
            .bundles
            .iter()
            .filter(|(pattern, _)| bundle_pattern_matches(pattern, bundle_id))
            .max_by_key(|(pattern, _)| pattern.len())
            .map(|(_, materials)| materials);
        wildcard.ok_or_else(|| {
            let available = self.bundles.keys().cloned().collect::<Vec<_>>().join(", ");
            Error::Signing(format!(
                "signing set '{}' has no profile for {bundle_id}; available: {available}",
                self.name
            ))
        })
    }

    /// Load and verify the PKCS#12 identity.
    pub fn identity(&self) -> Result<SigningIdentity> {
        let password = self.password()?;
        load_identity_checked(&self.identity_p12, &password)
    }

    /// Store the PKCS#12 password in the OS credential store.
    pub fn store_password(&self, password: &str) -> Result<()> {
        entry(&self.name)?
            .set_secret(password.as_bytes())
            .map_err(|error| {
                Error::Signing(format!(
                    "could not store the PKCS#12 password in the OS credential store: {error}"
                ))
            })
    }

    /// Remove the stored PKCS#12 password, if any.
    pub fn clear_password(&self) -> Result<()> {
        match entry(&self.name)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(Error::Signing(format!(
                "could not remove the PKCS#12 password: {error}"
            ))),
        }
    }

    /// Resolve the PKCS#12 password without printing it.
    pub fn password(&self) -> Result<Zeroizing<String>> {
        if let Ok(value) = std::env::var(PASSWORD_ENV) {
            return Ok(Zeroizing::new(value));
        }
        let secret = entry(&self.name)?
            .get_secret()
            .map_err(|error| match error {
                keyring::Error::NoEntry => Error::Signing(format!(
                    "the PKCS#12 password for signing set '{}' is not stored; rerun \
                     `impresario apple signing import` or set {PASSWORD_ENV}",
                    self.name
                )),
                error => Error::Signing(format!(
                    "could not read the PKCS#12 password from the OS credential store: {error}"
                )),
            })?;
        let text = String::from_utf8(secret).map_err(|_| {
            Error::Signing("the stored PKCS#12 password is not valid UTF-8".to_string())
        })?;
        Ok(Zeroizing::new(text))
    }

    /// Create a validated signing set from a source tree.
    ///
    /// Each directory containing `profile.mobileprovision` and
    /// `entitlements.plist` becomes one bundle. The PKCS#12 identity is taken
    /// from the source root or any bundle directory. Exports of one Apple
    /// development certificate can differ byte-for-byte, so identities are
    /// compared by leaf certificate, not container bytes. The profile plist
    /// supplies the bundle id, so callers do not have to pass bundle ids by
    /// hand.
    pub fn import(
        source: &Path,
        signing_root: &Path,
        name: &str,
        password: &str,
    ) -> Result<SigningImportReport> {
        if !source.is_dir() {
            return Err(Error::Signing(format!(
                "signing import source {} is not a directory",
                source.display()
            )));
        }
        let candidates = profile_dirs(source)?;
        if candidates.is_empty() {
            return Err(Error::Signing(format!(
                "no profile.mobileprovision + entitlements.plist pair found under {}",
                source.display()
            )));
        }

        let mut identity: Option<(Vec<u8>, SigningIdentity)> = None;
        let mut team_id: Option<String> = None;
        let mut planned: Vec<(String, PathBuf, PathBuf)> = Vec::new();
        for dir in candidates {
            let profile = dir.join("profile.mobileprovision");
            let entitlements = dir.join("entitlements.plist");
            let info = ProfileInfo::read(&profile)?;
            let Some(bundle_id) = info.bundle_id.clone() else {
                return Err(Error::Signing(format!(
                    "{} has no Entitlements.application-identifier",
                    profile.display()
                )));
            };
            if let Some(profile_team) = info.team_id {
                match &team_id {
                    Some(existing) if *existing != profile_team => {
                        return Err(Error::Signing(format!(
                            "profiles use different teams: {existing} and {profile_team}"
                        )));
                    }
                    None => team_id = Some(profile_team),
                    _ => {}
                }
            }
            if planned
                .iter()
                .any(|(existing, _, _)| *existing == bundle_id)
            {
                return Err(Error::Signing(format!(
                    "two profiles both target bundle id {bundle_id}"
                )));
            }
            let candidate_identity = dir.join("identity.p12");
            if candidate_identity.is_file() {
                load_matching_identity(&mut identity, &candidate_identity, password)?;
            }
            planned.push((bundle_id, profile, entitlements));
        }

        let root_identity = source.join("identity.p12");
        if root_identity.is_file() {
            load_matching_identity(&mut identity, &root_identity, password)?;
        }
        let (identity, _) = identity.ok_or_else(|| {
            Error::Signing(format!(
                "no identity.p12 found under {}; point the import at a directory containing the \
                 signing identity",
                source.display()
            ))
        })?;
        let team_id = team_id.ok_or_else(|| {
            Error::Signing("the source profiles do not name a TeamIdentifier".to_string())
        })?;

        std::fs::create_dir_all(signing_root)
            .map_err(|error| Error::io(format!("create {}", signing_root.display()), error))?;
        let destination = signing_root.join(safe_key(name));
        let temporary = signing_root.join(format!(".import-{}", Uuid::new_v4()));
        let guard = RemoveOnDrop(temporary.clone());
        std::fs::create_dir_all(temporary.join("bundles"))
            .map_err(|error| Error::io("create import directory", error))?;
        std::fs::write(temporary.join("team-id"), format!("{team_id}\n"))
            .map_err(|error| Error::io("write team-id", error))?;
        std::fs::write(temporary.join("identity.p12"), &identity)
            .map_err(|error| Error::io("write identity.p12", error))?;
        for (bundle_id, profile, entitlements) in &planned {
            let bundle_dir = temporary.join("bundles").join(safe_key(bundle_id));
            std::fs::create_dir_all(&bundle_dir)
                .map_err(|error| Error::io("create bundle directory", error))?;
            std::fs::copy(profile, bundle_dir.join("profile.mobileprovision"))
                .map_err(|error| Error::io("copy provisioning profile", error))?;
            std::fs::copy(entitlements, bundle_dir.join("entitlements.plist"))
                .map_err(|error| Error::io("copy entitlements", error))?;
        }

        // Refuse a wrong password or an unreadable PKCS#12 before replacing an
        // existing working set.
        load_identity_checked(&temporary.join("identity.p12"), password)?;
        let set = Self::from_dir(&temporary, name)?;
        if destination.exists() {
            std::fs::remove_dir_all(&destination)
                .map_err(|error| Error::io(format!("replace {}", destination.display()), error))?;
        }
        std::fs::rename(&temporary, &destination).map_err(|error| {
            Error::io(
                format!("move {} to {}", temporary.display(), destination.display()),
                error,
            )
        })?;
        drop(guard);

        let password_stored = match set.store_password(password) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "PKCS#12 password was not stored in the credential store");
                false
            }
        };
        Ok(SigningImportReport {
            name: name.to_string(),
            team_id,
            bundles: planned
                .into_iter()
                .map(|(bundle_id, _, _)| bundle_id)
                .collect(),
            password_stored,
            root: destination,
            warning: (!password_stored).then(|| {
                format!("the credential store is unavailable; set {PASSWORD_ENV} for signing runs")
            }),
        })
    }
}

fn load_identity_checked(p12: &Path, password: &str) -> Result<SigningIdentity> {
    load_identity(p12, password).map_err(|error| match error {
        Error::Signing(message) => Error::Signing(format!(
            "could not open {} with the supplied PKCS#12 password: {message}",
            p12.display()
        )),
        other => other,
    })
}

fn load_matching_identity(
    identity: &mut Option<(Vec<u8>, SigningIdentity)>,
    p12: &Path,
    password: &str,
) -> Result<()> {
    let candidate = load_identity_checked(p12, password)?;
    if let Some((_, baseline)) = identity {
        if !baseline.same_certificate(&candidate) {
            return Err(Error::Signing(format!(
                "{} uses a different signing certificate than the first identity in this tree; \
                 one signing set must use one Apple development certificate",
                p12.display()
            )));
        }
        return Ok(());
    }
    let bytes =
        std::fs::read(p12).map_err(|error| Error::io(format!("read {}", p12.display()), error))?;
    *identity = Some((bytes, candidate));
    Ok(())
}

fn entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("signing:{name}"))
        .map_err(|error| Error::Signing(format!("OS credential store unavailable: {error}")))
}

/// A successful signing-set import.
#[derive(Debug, Clone)]
pub struct SigningImportReport {
    pub name: String,
    pub team_id: String,
    pub bundles: Vec<String>,
    pub password_stored: bool,
    pub root: PathBuf,
    pub warning: Option<String>,
}

fn profile_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if dir.join("profile.mobileprovision").is_file() && dir.join("entitlements.plist").is_file()
        {
            out.push(dir);
            continue;
        }
        if depth >= 4 {
            continue;
        }
        let entries = std::fs::read_dir(&dir)
            .map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
            if entry
                .file_type()
                .map_err(|error| Error::io("stat signing source", error))?
                .is_dir()
            {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn read_trimmed(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::io(format!("read {}", path.display()), error))?;
    Ok(text.trim().to_string())
}

fn self_name_hint(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |name| name.to_string_lossy().to_string(),
    )
}

/// Sanitize a user-supplied profile or set name for a directory component.
#[must_use]
pub fn safe_key(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        "profile".to_string()
    } else {
        out
    }
}

/// Match a provisioning-profile app id against a concrete bundle id.
#[must_use]
pub fn bundle_pattern_matches(pattern: &str, bundle_id: &str) -> bool {
    if pattern == bundle_id {
        return true;
    }
    pattern
        .strip_suffix('*')
        .is_some_and(|prefix| bundle_id.starts_with(prefix))
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn profile(dir: &Path, bundle_id: &str, team: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let mut entitlements = plist::Dictionary::new();
        entitlements.insert(
            "application-identifier".into(),
            plist::Value::String(format!("{team}.{bundle_id}")),
        );
        entitlements.insert(
            "com.apple.developer.team-identifier".into(),
            plist::Value::String(team.into()),
        );
        entitlements.insert("get-task-allow".into(), plist::Value::Boolean(true));
        let mut profile = plist::Dictionary::new();
        profile.insert(
            "Name".into(),
            plist::Value::String(format!("Profile for {bundle_id}")),
        );
        profile.insert(
            "UUID".into(),
            plist::Value::String(Uuid::new_v4().to_string()),
        );
        profile.insert(
            "TeamIdentifier".into(),
            plist::Value::Array(vec![plist::Value::String(team.into())]),
        );
        profile.insert(
            "Platform".into(),
            plist::Value::Array(vec![plist::Value::String("tvOS".into())]),
        );
        profile.insert(
            "ProvisionedDevices".into(),
            plist::Value::Array(vec![plist::Value::String("device-1".into())]),
        );
        profile.insert(
            "ExpirationDate".into(),
            plist::Value::Date(plist::Date::from_xml_format("2030-01-02T03:04:05Z").unwrap()),
        );
        profile.insert(
            "Entitlements".into(),
            plist::Value::Dictionary(entitlements),
        );
        plist::Value::Dictionary(profile)
            .to_file_xml(dir.join("profile.mobileprovision"))
            .unwrap();
        std::fs::write(dir.join("entitlements.plist"), b"<plist/>").unwrap();
    }

    #[test]
    fn profile_info_extracts_bundle_team_and_devices() {
        let root = std::env::temp_dir().join(format!(
            "impresario-profile-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        profile(&root, "com.example.app", "TEAM123456");
        let info = ProfileInfo::read(&root.join("profile.mobileprovision")).unwrap();
        assert_eq!(info.team_id.as_deref(), Some("TEAM123456"));
        assert_eq!(info.bundle_id.as_deref(), Some("com.example.app"));
        assert!(info.allows_bundle("com.example.app"));
        assert!(info.allows_device("DEVICE-1"));
        assert!(!info.allows_device("other"));
        assert_eq!(info.get_task_allow, Some(true));
        assert_eq!(info.expiration.as_deref(), Some("2030-01-02T03:04:05Z"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wildcard_profiles_cover_a_family() {
        assert!(bundle_pattern_matches("com.example.*", "com.example.app"));
        assert!(bundle_pattern_matches(
            "com.example.agent.*",
            "com.example.agent.tests"
        ));
        assert!(!bundle_pattern_matches("com.example.*", "com.other.app"));
        assert!(bundle_pattern_matches("com.example.app", "com.example.app"));
    }

    #[test]
    fn signing_sets_are_keyed_by_profile_bundle_id() {
        let root = std::env::temp_dir().join(format!(
            "impresario-signing-set-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        profile(
            &root.join("bundles").join("not-the-bundle-id"),
            "com.example.app",
            "TEAM123456",
        );
        std::fs::write(root.join("team-id"), "TEAM123456\n").unwrap();
        std::fs::write(root.join("identity.p12"), b"not a real p12").unwrap();
        let set = SigningSet::from_dir(&root, "test").unwrap();
        assert_eq!(set.team_id(), "TEAM123456");
        assert!(set.require("com.example.app").is_ok());
        assert!(set.require("com.example.other").is_err());
        let _lock = ENV_LOCK.lock().unwrap();
        // CI runners have no OS credential store. Supply the password through
        // the documented environment override so this test reaches the PKCS#12
        // parser instead of failing while looking up a stored secret.
        std::env::set_var(PASSWORD_ENV, "test-password");
        let error = set.identity().unwrap_err().to_string();
        std::env::remove_var(PASSWORD_ENV);
        assert!(
            error.contains("PKCS#12") || error.contains("could not open"),
            "{error}"
        );
        assert!(!error.contains("not a real p12"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn safe_keys_never_escape_the_signing_root() {
        let root = Path::new("signing");
        let path = root.join(safe_key("../../etc/passwd"));
        assert_eq!(path.parent(), Some(root));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".._.._etc_passwd")
        );
        assert_eq!(safe_key(".."), "profile");
        assert_eq!(safe_key(""), "profile");
        assert_eq!(safe_key("com.example.app"), "com.example.app");
    }

    #[test]
    fn password_can_come_from_the_environment_without_being_printed() {
        let root = std::env::temp_dir().join(format!(
            "impresario-password-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        profile(
            &root.join("bundles").join("com.example.app"),
            "com.example.app",
            "TEAM123456",
        );
        std::fs::write(root.join("team-id"), "TEAM123456\n").unwrap();
        std::fs::write(root.join("identity.p12"), b"fake").unwrap();
        let set = SigningSet::from_dir(&root, "test").unwrap();
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(PASSWORD_ENV, "not-a-real-secret");
        assert_eq!(&*set.password().unwrap(), "not-a-real-secret");
        std::env::remove_var(PASSWORD_ENV);
        std::fs::remove_dir_all(root).unwrap();
    }
}
