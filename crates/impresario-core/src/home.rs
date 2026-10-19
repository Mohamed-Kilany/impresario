//! The global `~/.impresario` home (TASK-2 section 3.3).
//!
//! Everything the second-generation CLI keeps outside a workspace lives under
//! one root: global config, reusable device and app definitions, generated
//! keys, cached agent payloads and managed tools, job state and logs.
//!
//! Resolution order is `IMPRESARIO_HOME` first (tests and portable installs),
//! then the platform user profile. An unset or empty home is an error, never a
//! silent fallback to the current directory: writing credentials somewhere
//! unexpected would be worse than refusing to start.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Why the global home could not be determined or created.
#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    /// Neither `IMPRESARIO_HOME` nor a platform home directory is available.
    #[error(
        "cannot determine the user home directory; \
         set IMPRESARIO_HOME to the directory impresario should use"
    )]
    NoHome,
    /// The override exists but is empty.
    #[error("IMPRESARIO_HOME is set but empty")]
    EmptyOverride,
    /// A directory in the layout could not be created.
    #[error("cannot create {path}: {source}")]
    Create {
        /// The directory that failed.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// The resolved global home directory and the paths inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// Use `root` as the global home.
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve the global home from the process environment.
    pub fn resolve() -> Result<Self, HomeError> {
        Self::resolve_from(
            std::env::var_os("IMPRESARIO_HOME"),
            std::env::var_os("USERPROFILE"),
            std::env::var_os("HOME"),
        )
    }

    /// Pure resolution used by tests and by [`Home::resolve`].
    ///
    /// `user_profile` is the Windows convention, `home` the Unix one; either
    /// may be absent. `IMPRESARIO_HOME`, when present, wins and must not be
    /// empty.
    pub fn resolve_from(
        override_home: Option<OsString>,
        user_profile: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, HomeError> {
        if let Some(value) = override_home {
            if value.is_empty() {
                return Err(HomeError::EmptyOverride);
            }
            return Ok(Self::at(PathBuf::from(value)));
        }
        for value in [user_profile, home].into_iter().flatten() {
            if !value.is_empty() {
                return Ok(Self::at(PathBuf::from(value).join(".impresario")));
            }
        }
        Err(HomeError::NoHome)
    }

    /// The global home root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `config.yaml` - global defaults, the current workspace and UI prefs.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.yaml")
    }

    /// `devices.yaml` - devices reusable across workspaces.
    #[must_use]
    pub fn devices_path(&self) -> PathBuf {
        self.root.join("devices.yaml")
    }

    /// `apps.yaml` - apps reusable across workspaces.
    #[must_use]
    pub fn apps_path(&self) -> PathBuf {
        self.root.join("apps.yaml")
    }

    /// `keys/` - generated ADB/SSH keys and encrypted signing identities.
    #[must_use]
    pub fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    /// `agents/` - extracted/cached executor payloads.
    #[must_use]
    pub fn agents_dir(&self) -> PathBuf {
        self.root.join("agents")
    }

    /// `tools/` - managed downloads (check, never install silently).
    #[must_use]
    pub fn tools_dir(&self) -> PathBuf {
        self.root.join("tools")
    }

    /// `jobs/` - per-job state, logs and IPC metadata.
    #[must_use]
    pub fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    /// `cache/` - re-signing and packaging caches.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `history` - interactive prompt history.
    #[must_use]
    pub fn history_path(&self) -> PathBuf {
        self.root.join("history")
    }

    /// `state.json` - TUI state.
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// `logs/` - rotated diagnostic logs.
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Create the home and every directory the layout promises.
    ///
    /// Files are created lazily; missing directories are not an error for
    /// reading, but commands that write ask for this first.
    pub fn ensure_dirs(&self) -> Result<(), HomeError> {
        for path in [
            self.root.clone(),
            self.keys_dir(),
            self.agents_dir(),
            self.tools_dir(),
            self.jobs_dir(),
            self.cache_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(&path).map_err(|source| HomeError::Create {
                path: path.clone(),
                source,
            })?;
        }
        restrict_keys_dir(&self.keys_dir());
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_keys_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn restrict_keys_dir(_path: &Path) {}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn override_wins_over_platform_homes() {
        let home = Home::resolve_from(Some(os("D:/portable")), Some(os("C:/Users/x")), None)
            .expect("home");
        assert_eq!(home.root(), Path::new("D:/portable"));
        assert_eq!(home.config_path(), Path::new("D:/portable/config.yaml"));
    }

    #[test]
    fn windows_profile_is_used_with_the_dot_directory() {
        let home =
            Home::resolve_from(None, Some(os("C:/Users/x")), Some(os("/home/x"))).expect("home");
        assert_eq!(home.root(), Path::new("C:/Users/x/.impresario"));
    }

    #[test]
    fn unix_home_is_the_fallback() {
        let home = Home::resolve_from(None, None, Some(os("/home/x"))).expect("home");
        assert_eq!(home.root(), Path::new("/home/x/.impresario"));
    }

    #[test]
    fn missing_home_is_an_error() {
        assert!(matches!(
            Home::resolve_from(None, None, None),
            Err(HomeError::NoHome)
        ));
    }

    #[test]
    fn empty_override_is_an_error() {
        assert!(matches!(
            Home::resolve_from(Some(os("")), Some(os("C:/Users/x")), None),
            Err(HomeError::EmptyOverride)
        ));
    }

    #[test]
    fn layout_paths_live_under_the_root() {
        let home = Home::at("root");
        for path in [
            home.config_path(),
            home.devices_path(),
            home.apps_path(),
            home.keys_dir(),
            home.agents_dir(),
            home.tools_dir(),
            home.jobs_dir(),
            home.cache_dir(),
            home.history_path(),
            home.state_path(),
            home.logs_dir(),
        ] {
            assert!(path.starts_with("root"), "{}", path.display());
        }
        assert_eq!(home.devices_path(), Path::new("root/devices.yaml"));
        assert_eq!(home.keys_dir(), Path::new("root/keys"));
    }

    #[test]
    fn ensure_dirs_creates_the_layout() {
        let dir = std::env::temp_dir().join(format!(
            "impresario-home-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let home = Home::at(&dir);
        home.ensure_dirs().expect("layout");
        for path in [
            home.root().to_path_buf(),
            home.keys_dir(),
            home.agents_dir(),
            home.tools_dir(),
            home.jobs_dir(),
            home.cache_dir(),
            home.logs_dir(),
        ] {
            assert!(path.is_dir(), "{}", path.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
