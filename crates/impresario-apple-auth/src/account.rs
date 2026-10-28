//! Global Apple ID session store and the host-side login/refresh flow.
//!
//! A user signs in once. The Apple session is written to
//! `~/.impresario/apple/session.bin`, encrypted with AES-256-GCM; the 32-byte
//! key lives in the OS credential store. No workspace config is involved and
//! no password is ever persisted.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use plist::Dictionary;
use rand::RngCore;
use rootcause::prelude::*;
use serde::{Deserialize, Serialize};

use crate::anisette::remote_v3::{DEFAULT_ANISETTE_V3_URL, RemoteV3AnisetteProvider};
use crate::auth::apple_account::{
    AppToken, AppleAccount, TrustedNumber, TwoFactorCallbackParams, TwoFactorCallbackResponse,
};
use crate::dev::developer_session::DeveloperSession;
use crate::dev::teams::{DeveloperTeam, TeamsApi};
use crate::util::storage::SideloadingStorage;

/// Default remote anisette provider. Override it when self-hosting anisette.
pub const DEFAULT_ANISETTE_URL: &str = DEFAULT_ANISETTE_V3_URL;
/// OS credential-store service for the global Apple session.
pub const KEYRING_SERVICE: &str = "impresario.apple";
/// OS credential-store user for the AES key that protects the session file.
pub const KEYRING_KEY_USER: &str = "session-key";
const MAGIC: &[u8; 8] = b"IMPAAPL1";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const SESSION_FILE: &str = "session.bin";
const ANISETTE_FILE: &str = "anisette.bin";

/// Errors surfaced by the account layer. Sensitive values never appear in the
/// messages: only the account email is masked and errors are contextualized.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no stored Apple account; run `impresario account login`")]
    NotLoggedIn,
    #[error("the stored Apple session key is missing; sign in again")]
    MissingKey,
    #[error("the stored Apple session file is missing or corrupt; sign in again")]
    CorruptStore,
    #[error("Apple request failed: {0}")]
    Apple(String),
    #[error("OS credential store error: {0}")]
    Keyring(String),
    #[error("Apple account storage error: {0}")]
    Storage(String),
    #[error(
        "Apple did not return a developer team; sign in to Apple Developer or accept the agreement"
    )]
    NoTeams,
    #[error("multiple Apple developer teams; select one with `--team`: {0}")]
    TeamChoiceRequired(String),
    #[error("Apple developer team `{0}` is not available on this account")]
    UnknownTeam(String),
    #[error("could not read 2FA input: {0}")]
    Prompt(String),
    #[error("2FA was cancelled")]
    TwoFactorCancelled,
    #[error("{0}")]
    TwoFactorUnavailable(String),
    #[error("the stored Apple session expired; run `impresario account login --force`")]
    Expired,
}

impl AuthError {
    fn apple(error: impl fmt::Display) -> Self {
        Self::Apple(error.to_string())
    }

    fn storage(error: impl fmt::Display) -> Self {
        Self::Storage(error.to_string())
    }

    fn keyring(error: impl fmt::Display) -> Self {
        Self::Keyring(error.to_string())
    }
}

/// An interactive 2FA source. The CLI implements this with a console; tests
/// can implement it with a scripted queue.
pub trait TwoFactorPrompt: Send {
    fn ask(
        &mut self,
        params: &TwoFactorCallbackParams,
    ) -> Result<TwoFactorCallbackResponse, AuthError>;
}

/// Anisette endpoint and stable machine serial used for Apple's headers.
#[derive(Clone, Debug)]
pub struct AnisetteConfig {
    pub url: String,
    pub serial_number: String,
}

impl Default for AnisetteConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_ANISETTE_URL.to_string(),
            serial_number: "0".to_string(),
        }
    }
}

/// A developer-services token plus the time it was obtained. Apple has
/// historically returned both absolute `expiry` values and short relative
/// ones; the accessor handles both.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenRecord {
    pub token: String,
    pub duration: u64,
    pub expiry: u64,
    pub saved_at_unix: u64,
}

impl fmt::Debug for TokenRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenRecord")
            .field("token", &"<redacted>")
            .field("duration", &self.duration)
            .field("expiry", &self.expiry)
            .field("saved_at_unix", &self.saved_at_unix)
            .finish()
    }
}

impl TokenRecord {
    fn from_app_token(token: AppToken) -> Self {
        Self {
            token: token.token,
            duration: token.duration,
            expiry: token.expiry,
            saved_at_unix: now_secs(),
        }
    }

    fn to_app_token(&self) -> AppToken {
        AppToken {
            token: self.token.clone(),
            duration: self.duration,
            expiry: self.expiry,
        }
    }

    /// Absolute expiry in Unix seconds.
    #[must_use]
    pub fn expires_at_unix(&self) -> u64 {
        if self.expiry > 1_000_000_000 {
            self.expiry
        } else {
            self.saved_at_unix.saturating_add(self.expiry)
        }
    }

    /// `expiry == 0` means Apple did not give us one; don't pre-emptively
    /// force a re-login, let the server decide.
    #[must_use]
    pub fn is_expired(&self, skew_secs: u64) -> bool {
        self.expiry != 0 && now_secs().saturating_add(skew_secs) >= self.expires_at_unix()
    }
}

/// The global Apple account. Serialized into the encrypted session file.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppleCredentials {
    pub email: String,
    pub display_name: Option<String>,
    pub team_id: String,
    pub team_name: Option<String>,
    pub adsid: String,
    pub app_token: TokenRecord,
    /// XML plist (`spd`) returned by GrandSlam, base64 encoded. It contains
    /// the Apple ID session, so it is only ever stored encrypted.
    pub spd_plist_base64: String,
    pub anisette_url: String,
    pub machine_serial: String,
    pub saved_at_unix: u64,
}

impl fmt::Debug for AppleCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppleCredentials")
            .field("email", &mask_email(&self.email))
            .field("team_id", &self.team_id)
            .field("app_token", &"<redacted>")
            .field("adsid", &"<redacted>")
            .field("spd", &"<redacted>")
            .field("expires_at_unix", &self.expires_at_unix())
            .finish()
    }
}

impl AppleCredentials {
    fn new(
        email: &str,
        display_name: Option<String>,
        team: &DeveloperTeam,
        token: AppToken,
        adsid: &str,
        spd: &Dictionary,
        anisette: &AnisetteConfig,
    ) -> Result<Self, AuthError> {
        Ok(Self {
            email: email.to_string(),
            display_name,
            team_id: team.team_id.clone(),
            team_name: team.name.clone(),
            adsid: adsid.to_string(),
            app_token: TokenRecord::from_app_token(token),
            spd_plist_base64: encode_plist(spd)?,
            anisette_url: anisette.url.clone(),
            machine_serial: anisette.serial_number.clone(),
            saved_at_unix: now_secs(),
        })
    }

    fn spd(&self) -> Result<Dictionary, AuthError> {
        let bytes = BASE64_STANDARD
            .decode(&self.spd_plist_base64)
            .map_err(|e| AuthError::storage(format!("invalid Apple session encoding: {e}")))?;
        plist::from_bytes(&bytes)
            .map_err(|e| AuthError::storage(format!("invalid Apple session plist: {e}")))
    }

    #[must_use]
    pub fn expires_at_unix(&self) -> u64 {
        self.app_token.expires_at_unix()
    }

    #[must_use]
    pub fn is_expired(&self, skew_secs: u64) -> bool {
        self.app_token.is_expired(skew_secs)
    }
}

/// Encrypted global credential storage. The implementation is deliberately
/// limited to three operations so the CLI can swap in a memory store for
/// tests without touching the OS keyring.
pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<AppleCredentials>, AuthError>;
    fn save(&self, credentials: &AppleCredentials) -> Result<(), AuthError>;
    fn delete(&self) -> Result<(), AuthError>;
}

/// The live session created by `authenticate`; the CLI picks a team and then
/// calls `save`.
pub struct LoginSession {
    email: String,
    display_name: Option<String>,
    account: AppleAccount,
    session: DeveloperSession,
    teams: Vec<DeveloperTeam>,
    anisette: AnisetteConfig,
}

impl LoginSession {
    #[must_use]
    pub fn email(&self) -> &str {
        &self.email
    }

    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    #[must_use]
    pub fn teams(&self) -> &[DeveloperTeam] {
        &self.teams
    }

    /// Pick `team_id`, or the only team when there is exactly one.
    pub fn select_team(&self, team_id: Option<&str>) -> Result<DeveloperTeam, AuthError> {
        choose_team(&self.teams, team_id)
    }

    /// Persist this session globally. The password is not part of the model.
    pub fn save(
        &self,
        team: &DeveloperTeam,
        store: &dyn CredentialStore,
    ) -> Result<AppleCredentials, AuthError> {
        let spd = self.account.spd.as_ref().ok_or(AuthError::CorruptStore)?;
        let credentials = AppleCredentials::new(
            &self.email,
            self.display_name.clone(),
            team,
            self.session.token().clone(),
            self.session.adsid(),
            spd,
            &self.anisette,
        )?;
        store.save(&credentials)?;
        Ok(credentials)
    }
}

/// Log in with an Apple ID. `password` is used only for this call by the
/// caller; this function never serializes it.
///
/// # Errors
/// Network, Apple, 2FA-cancel and storage errors are returned as `AuthError`.
pub async fn authenticate<P: TwoFactorPrompt + Send + 'static>(
    email: &str,
    password: &str,
    anisette: AnisetteConfig,
    anisette_storage: Box<dyn SideloadingStorage>,
    prompt: P,
) -> Result<LoginSession, AuthError> {
    crate::init();
    crate::init_tls();

    let provider = RemoteV3AnisetteProvider::new(
        &anisette.url,
        anisette_storage,
        anisette.serial_number.clone(),
    )
    .map_err(AuthError::apple)?;

    let prompt = Arc::new(Mutex::new(prompt));
    let callback_prompt = Arc::clone(&prompt);
    let callback = move |params: TwoFactorCallbackParams| {
        let prompt = Arc::clone(&callback_prompt);
        async move {
            let mut prompt = prompt.lock().map_err(|_| AuthError::TwoFactorCancelled)?;
            prompt.ask(&params).map_err(|error| report!("{error}"))
        }
    };

    let mut account = AppleAccount::builder(email)
        .anisette_provider(provider)
        .login(password, callback)
        .await
        .map_err(AuthError::apple)?;

    let display_name = account
        .get_name()
        .ok()
        .map(|(first, last)| format!("{first} {last}"));

    let mut session = DeveloperSession::from_account(&mut account)
        .await
        .map_err(AuthError::apple)?;
    let teams = session.list_teams().await.map_err(AuthError::apple)?;

    Ok(LoginSession {
        email: email.to_string(),
        display_name,
        account,
        session,
        teams,
        anisette,
    })
}

/// Rebuild a developer session from the encrypted global account.
///
/// The anisette state and the stored `spd` allow Apple's short-lived
/// developer token to be refreshed without asking for the password again.
/// If Apple has invalidated the underlying session, the caller should run
/// `impresario account login --force`.
pub async fn load_session(
    credentials: &mut AppleCredentials,
    store: &dyn CredentialStore,
    anisette_storage: Box<dyn SideloadingStorage>,
) -> Result<DeveloperSession, AuthError> {
    crate::init();
    crate::init_tls();

    let provider = RemoteV3AnisetteProvider::new(
        &credentials.anisette_url,
        anisette_storage,
        credentials.machine_serial.clone(),
    )
    .map_err(AuthError::apple)?;

    let mut account = AppleAccount::builder(&credentials.email)
        .anisette_provider(provider)
        .build()
        .await
        .map_err(AuthError::apple)?;
    account.spd = Some(credentials.spd()?);

    if credentials.is_expired(300) {
        let token = account
            .get_app_token("xcode.auth")
            .await
            .map_err(|error| {
                AuthError::apple(format!(
                    "could not refresh the Apple developer token; run `impresario account login --force` if Apple invalidated the session: {error}"
                ))
            })?;
        credentials.app_token = TokenRecord::from_app_token(token);
        credentials.saved_at_unix = now_secs();
        store.save(credentials)?;
    }

    Ok(DeveloperSession::new(
        credentials.app_token.to_app_token(),
        credentials.adsid.clone(),
        account.grandslam_client.clone(),
        account.anisette_generator.clone(),
    ))
}

/// Verify a stored account online by listing its developer teams.
pub async fn verify(
    credentials: &mut AppleCredentials,
    store: &dyn CredentialStore,
    anisette_storage: Box<dyn SideloadingStorage>,
) -> Result<Vec<DeveloperTeam>, AuthError> {
    let mut session = load_session(credentials, store, anisette_storage).await?;
    let teams = session.list_teams().await.map_err(AuthError::apple)?;
    if !teams.iter().any(|team| team.team_id == credentials.team_id) {
        return Err(AuthError::UnknownTeam(credentials.team_id.clone()));
    }
    Ok(teams)
}

/// Pick a team without duplicating the list logic in the CLI.
pub fn choose_team(
    teams: &[DeveloperTeam],
    team_id: Option<&str>,
) -> Result<DeveloperTeam, AuthError> {
    if teams.is_empty() {
        return Err(AuthError::NoTeams);
    }
    if let Some(team_id) = team_id {
        return teams
            .iter()
            .find(|team| team.team_id == team_id)
            .cloned()
            .ok_or_else(|| AuthError::UnknownTeam(team_id.to_string()));
    }
    if teams.len() == 1 {
        return teams.first().cloned().ok_or(AuthError::NoTeams);
    }
    let list = teams
        .iter()
        .map(|team| {
            format!(
                "{} ({})",
                team.name.as_deref().unwrap_or("<unnamed>"),
                team.team_id
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(AuthError::TeamChoiceRequired(list))
}

/// The machine-global store used by the CLI. The directory is
/// `~/.impresario/apple` in production, but tests can pass a temp directory.
pub struct AppleStore {
    dir: PathBuf,
    credentials: FileCredentialStore,
    anisette: FileAnisetteStorage,
    key: Arc<dyn SecretKeyProvider>,
}

impl AppleStore {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self::with_key_provider(dir, Arc::new(KeyringKeyProvider::default()))
    }

    #[must_use]
    pub fn with_key_provider(dir: impl Into<PathBuf>, key: Arc<dyn SecretKeyProvider>) -> Self {
        let dir = dir.into();
        let credentials = FileCredentialStore::new(dir.join(SESSION_FILE), Arc::clone(&key));
        let anisette = FileAnisetteStorage::new(dir.join(ANISETTE_FILE), Arc::clone(&key));
        Self {
            dir,
            credentials,
            anisette,
            key,
        }
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn credentials_path(&self) -> PathBuf {
        self.credentials.path.clone()
    }

    /// A fresh boxed anisette store sharing the same encryption key as the
    /// credential file.
    #[must_use]
    pub fn anisette_storage(&self) -> Box<dyn SideloadingStorage> {
        Box::new(self.anisette.clone())
    }

    /// Remove the encrypted session, the anisette state and the key. This is
    /// global and does not touch any workspace.
    pub fn logout(&self) -> Result<(), AuthError> {
        self.credentials.delete()?;
        self.anisette.delete_all()?;
        self.key.delete()
    }
}

impl CredentialStore for AppleStore {
    fn load(&self) -> Result<Option<AppleCredentials>, AuthError> {
        self.credentials.load()
    }

    fn save(&self, credentials: &AppleCredentials) -> Result<(), AuthError> {
        self.credentials.save(credentials)
    }

    fn delete(&self) -> Result<(), AuthError> {
        self.credentials.delete()
    }
}

/// A small in-memory credential store for tests and for non-persistent
/// embedding.
#[derive(Default)]
pub struct MemoryCredentialStore {
    credentials: Mutex<Option<AppleCredentials>>,
}

impl CredentialStore for MemoryCredentialStore {
    fn load(&self) -> Result<Option<AppleCredentials>, AuthError> {
        Ok(self
            .credentials
            .lock()
            .map_err(|_| AuthError::CorruptStore)?
            .clone())
    }

    fn save(&self, credentials: &AppleCredentials) -> Result<(), AuthError> {
        *self
            .credentials
            .lock()
            .map_err(|_| AuthError::CorruptStore)? = Some(credentials.clone());
        Ok(())
    }

    fn delete(&self) -> Result<(), AuthError> {
        *self
            .credentials
            .lock()
            .map_err(|_| AuthError::CorruptStore)? = None;
        Ok(())
    }
}

/// Supplies the AES key for the global files. In production the key is a
/// 32-byte binary secret in the OS credential store; tests use memory.
pub trait SecretKeyProvider: Send + Sync {
    fn load(&self) -> Result<Option<[u8; 32]>, AuthError>;
    fn save(&self, key: &[u8; 32]) -> Result<(), AuthError>;
    fn delete(&self) -> Result<(), AuthError>;
}

#[derive(Clone)]
pub struct KeyringKeyProvider {
    service: String,
    user: String,
}

impl Default for KeyringKeyProvider {
    fn default() -> Self {
        Self {
            service: KEYRING_SERVICE.to_string(),
            user: KEYRING_KEY_USER.to_string(),
        }
    }
}

impl KeyringKeyProvider {
    #[must_use]
    pub fn new(service: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            user: user.into(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, AuthError> {
        keyring::Entry::new(&self.service, &self.user).map_err(AuthError::keyring)
    }
}

impl SecretKeyProvider for KeyringKeyProvider {
    fn load(&self) -> Result<Option<[u8; 32]>, AuthError> {
        let entry = self.entry()?;
        match entry.get_secret() {
            Ok(secret) if secret.len() == 32 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&secret);
                Ok(Some(key))
            }
            Ok(_) => Err(AuthError::MissingKey),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(AuthError::keyring(error)),
        }
    }

    fn save(&self, key: &[u8; 32]) -> Result<(), AuthError> {
        self.entry()?.set_secret(key).map_err(AuthError::keyring)
    }

    fn delete(&self) -> Result<(), AuthError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(AuthError::keyring(error)),
        }
    }
}

#[derive(Default)]
pub struct MemoryKeyProvider {
    key: Mutex<Option<[u8; 32]>>,
}

impl SecretKeyProvider for MemoryKeyProvider {
    fn load(&self) -> Result<Option<[u8; 32]>, AuthError> {
        Ok(*self.key.lock().map_err(|_| AuthError::CorruptStore)?)
    }

    fn save(&self, key: &[u8; 32]) -> Result<(), AuthError> {
        *self.key.lock().map_err(|_| AuthError::CorruptStore)? = Some(*key);
        Ok(())
    }

    fn delete(&self) -> Result<(), AuthError> {
        *self.key.lock().map_err(|_| AuthError::CorruptStore)? = None;
        Ok(())
    }
}

#[derive(Clone)]
pub struct FileCredentialStore {
    path: PathBuf,
    key: Arc<dyn SecretKeyProvider>,
}

impl FileCredentialStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, key: Arc<dyn SecretKeyProvider>) -> Self {
        Self {
            path: path.into(),
            key,
        }
    }

    fn key_or_create(&self) -> Result<[u8; 32], AuthError> {
        if let Some(key) = self.key.load()? {
            return Ok(key);
        }
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        self.key.save(&key)?;
        Ok(key)
    }
}

impl CredentialStore for FileCredentialStore {
    fn load(&self) -> Result<Option<AppleCredentials>, AuthError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let key = self.key.load()?.ok_or(AuthError::MissingKey)?;
        let blob = fs::read(&self.path).map_err(AuthError::storage)?;
        let plaintext = decrypt_blob(&key, &blob)?;
        let credentials = serde_json::from_slice(&plaintext)
            .map_err(|error| AuthError::storage(format!("invalid Apple session JSON: {error}")))?;
        Ok(Some(credentials))
    }

    fn save(&self, credentials: &AppleCredentials) -> Result<(), AuthError> {
        let key = self.key_or_create()?;
        let plaintext = serde_json::to_vec(credentials).map_err(|error| {
            AuthError::storage(format!("could not encode Apple session: {error}"))
        })?;
        let blob = encrypt_blob(&key, &plaintext)?;
        write_private(&self.path, &blob)
    }

    fn delete(&self) -> Result<(), AuthError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AuthError::storage(error)),
        }
    }
}

/// Encrypted `SideloadingStorage` for the anisette state. The keyring only
/// holds the 32-byte key; the potentially larger state stays in the global
/// encrypted file.
#[derive(Clone)]
pub struct FileAnisetteStorage {
    path: PathBuf,
    key: Arc<dyn SecretKeyProvider>,
    cache: Arc<Mutex<Option<HashMap<String, String>>>>,
}

impl FileAnisetteStorage {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, key: Arc<dyn SecretKeyProvider>) -> Self {
        Self {
            path: path.into(),
            key,
            cache: Arc::new(Mutex::new(None)),
        }
    }

    fn load_map(&self) -> Result<HashMap<String, String>, AuthError> {
        if let Some(map) = self
            .cache
            .lock()
            .map_err(|_| AuthError::CorruptStore)?
            .as_ref()
        {
            return Ok(map.clone());
        }
        if !self.path.exists() {
            return Ok(HashMap::new());
        }
        let key = self.key.load()?.ok_or(AuthError::MissingKey)?;
        let blob = fs::read(&self.path).map_err(AuthError::storage)?;
        let plaintext = decrypt_blob(&key, &blob)?;
        let map: HashMap<String, String> = serde_json::from_slice(&plaintext)
            .map_err(|error| AuthError::storage(format!("invalid anisette state JSON: {error}")))?;
        *self.cache.lock().map_err(|_| AuthError::CorruptStore)? = Some(map.clone());
        Ok(map)
    }

    fn save_map(&self, map: &HashMap<String, String>) -> Result<(), AuthError> {
        let key = if let Some(key) = self.key.load()? {
            key
        } else {
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            self.key.save(&key)?;
            key
        };
        let plaintext = serde_json::to_vec(map).map_err(|error| {
            AuthError::storage(format!("could not encode anisette state: {error}"))
        })?;
        let blob = encrypt_blob(&key, &plaintext)?;
        write_private(&self.path, &blob)?;
        *self.cache.lock().map_err(|_| AuthError::CorruptStore)? = Some(map.clone());
        Ok(())
    }

    pub fn delete_all(&self) -> Result<(), AuthError> {
        self.cache
            .lock()
            .map_err(|_| AuthError::CorruptStore)?
            .take();
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AuthError::storage(error)),
        }
    }
}

impl SideloadingStorage for FileAnisetteStorage {
    fn store(&self, key: &str, value: &str) -> Result<(), Report> {
        let mut map = self.load_map().map_err(|error| report!("{error}"))?;
        map.insert(key.to_string(), value.to_string());
        self.save_map(&map).map_err(|error| report!("{error}"))
    }

    fn retrieve(&self, key: &str) -> Result<Option<String>, Report> {
        let map = self.load_map().map_err(|error| report!("{error}"))?;
        Ok(map.get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<(), Report> {
        let mut map = self.load_map().map_err(|error| report!("{error}"))?;
        map.remove(key);
        self.save_map(&map).map_err(|error| report!("{error}"))
    }
}

fn encode_plist(dictionary: &Dictionary) -> Result<String, AuthError> {
    let mut bytes = Vec::new();
    plist::to_writer_xml(&mut bytes, dictionary).map_err(|error| {
        AuthError::storage(format!("could not encode Apple session plist: {error}"))
    })?;
    Ok(BASE64_STANDARD.encode(bytes))
}

fn encrypt_blob(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, AuthError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|error| AuthError::storage(format!("invalid Apple session key: {error}")))?;
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let nonce = Nonce::try_from(&nonce[..])
        .map_err(|error| AuthError::storage(format!("invalid Apple session nonce: {error}")))?;
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|error| AuthError::storage(format!("could not encrypt Apple session: {error}")))?;
    let mut blob = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(MAGIC);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

fn decrypt_blob(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, AuthError> {
    if blob.len() < MAGIC.len() + NONCE_LEN + TAG_LEN || &blob[..MAGIC.len()] != MAGIC {
        return Err(AuthError::CorruptStore);
    }
    let nonce_start = MAGIC.len();
    let nonce_end = nonce_start + NONCE_LEN;
    let nonce = &blob[nonce_start..nonce_end];
    let ciphertext = &blob[nonce_end..];
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|error| AuthError::storage(format!("invalid Apple session key: {error}")))?;
    let nonce = Nonce::try_from(nonce)
        .map_err(|error| AuthError::storage(format!("invalid Apple session nonce: {error}")))?;
    cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| AuthError::CorruptStore)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), AuthError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(AuthError::storage)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(AuthError::storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(AuthError::storage)?;
    }
    #[cfg(windows)]
    {
        // The file lives under the user's profile and inherits its ACL. The
        // session is also encrypted, so losing the ACL alone does not expose
        // the token.
        if path.exists() {
            fs::remove_file(path).map_err(AuthError::storage)?;
        }
    }
    fs::rename(&tmp, path).map_err(AuthError::storage)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Mask an email for logs and diagnostics.
#[must_use]
pub fn mask_email(email: &str) -> String {
    if let Some(at) = email.find('@') {
        let (local, domain) = email.split_at(at);
        let first = local.chars().next().unwrap_or('*');
        format!("{first}***{domain}")
    } else {
        "***".to_string()
    }
}

/// Format the code the user should type from a trusted number, without
/// printing the full number.
#[must_use]
pub fn masked_number(number: &TrustedNumber) -> String {
    format!("••••{}", number.last_two_digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "impresario-apple-auth-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_credentials() -> AppleCredentials {
        AppleCredentials {
            email: "dev@example.com".into(),
            display_name: Some("Dev".into()),
            team_id: "TEAM123".into(),
            team_name: Some("Example Team".into()),
            adsid: "000123-00-abcdef".into(),
            app_token: TokenRecord {
                token: "secret-app-token".into(),
                duration: 3600,
                expiry: now_secs().saturating_add(3600),
                saved_at_unix: now_secs(),
            },
            spd_plist_base64: encode_plist(&Dictionary::new()).unwrap(),
            anisette_url: "https://anisette.example.invalid".into(),
            machine_serial: "0".into(),
            saved_at_unix: now_secs(),
        }
    }

    #[test]
    fn session_file_is_encrypted_and_round_trips() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("session.bin");
        let key = Arc::new(MemoryKeyProvider::default());
        let store = FileCredentialStore::new(&path, key);

        let credentials = sample_credentials();
        store.save(&credentials).unwrap();

        let bytes = fs::read(&path).unwrap();
        for secret in [
            credentials.email.as_str(),
            credentials.adsid.as_str(),
            credentials.app_token.token.as_str(),
        ] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()),
                "the encrypted session must not contain {secret:?} in plaintext"
            );
        }

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.email, credentials.email);
        assert_eq!(loaded.team_id, credentials.team_id);
        assert_eq!(loaded.adsid, credentials.adsid);
        assert_eq!(loaded.app_token.token, credentials.app_token.token);
        assert_eq!(loaded.spd_plist_base64, credentials.spd_plist_base64);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn session_file_rejects_a_different_key() {
        let dir = temp_dir("wrong-key");
        let path = dir.join("session.bin");
        let key = Arc::new(MemoryKeyProvider::default());
        FileCredentialStore::new(&path, key)
            .save(&sample_credentials())
            .unwrap();

        let other_key = Arc::new(MemoryKeyProvider::default());
        other_key.save(&[0x42; 32]).unwrap();
        let wrong = FileCredentialStore::new(&path, other_key);
        assert!(matches!(wrong.load(), Err(AuthError::CorruptStore)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn token_expiry_handles_absolute_relative_and_unknown() {
        let now = now_secs();
        let absolute = TokenRecord {
            token: "token".into(),
            duration: 3600,
            expiry: now.saturating_add(3600),
            saved_at_unix: now,
        };
        assert_eq!(absolute.expires_at_unix(), now.saturating_add(3600));
        assert!(!absolute.is_expired(60));

        let relative = TokenRecord {
            token: "token".into(),
            duration: 3600,
            expiry: 3600,
            saved_at_unix: now.saturating_sub(7200),
        };
        assert_eq!(relative.expires_at_unix(), now.saturating_sub(3600));
        assert!(relative.is_expired(0));

        let unknown = TokenRecord {
            token: "token".into(),
            duration: 0,
            expiry: 0,
            saved_at_unix: now,
        };
        assert!(!unknown.is_expired(0));
    }

    #[test]
    fn choose_team_cases() {
        let teams = vec![
            DeveloperTeam {
                name: Some("A".into()),
                team_id: "A".into(),
                r#type: None,
                status: None,
            },
            DeveloperTeam {
                name: Some("B".into()),
                team_id: "B".into(),
                r#type: None,
                status: None,
            },
        ];
        assert!(matches!(choose_team(&[], None), Err(AuthError::NoTeams)));
        assert_eq!(choose_team(&teams, Some("B")).unwrap().team_id, "B");
        assert!(matches!(
            choose_team(&teams, Some("Z")),
            Err(AuthError::UnknownTeam(_))
        ));
        assert_eq!(choose_team(&teams[..1], None).unwrap().team_id, "A");
        assert!(matches!(
            choose_team(&teams, None),
            Err(AuthError::TeamChoiceRequired(_))
        ));
    }

    #[test]
    fn debug_and_masks_never_expose_secrets() {
        let credentials = sample_credentials();
        let debug = format!("{credentials:?}");
        assert!(debug.contains("d***@example.com"));
        let token_debug = format!("{:?}", credentials.app_token);
        assert!(!token_debug.contains("secret-app-token"));
        for secret in [
            credentials.adsid.as_str(),
            credentials.app_token.token.as_str(),
            credentials.spd_plist_base64.as_str(),
        ] {
            assert!(!debug.contains(secret));
        }
        assert_eq!(mask_email("dev@example.com"), "d***@example.com");
        assert_eq!(
            masked_number(&TrustedNumber {
                number_with_dial_code: "+15551234567".into(),
                last_two_digits: "67".into(),
                push_mode: "sms".into(),
                id: 1,
            }),
            "••••67"
        );
    }
}
