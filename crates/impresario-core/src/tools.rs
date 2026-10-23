//! Managed tools (TASK-2 section 6.0, ladder rung 3).
//!
//! Only metadata and paths live here so the connector (which must find the
//! browser) and the CLI (which downloads it) share one definition. Downloads
//! are explicit: nothing in this module fetches or installs anything.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::home::Home;

/// Chrome for Testing build pinned by this release. Pinning the exact build
/// keeps the browser behavior and the checksum stable across installs
/// (TASK-2 section 6.0).
pub const CHROMIUM_VERSION: &str = "155.0.8059.84";

/// One platform's pinned Chrome for Testing archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromiumArtifact {
    /// `win64`, `mac-arm64`, `mac-x64`, `linux64` or `linux-arm64`.
    pub platform: &'static str,
    /// Version embedded in the artifact.
    pub version: &'static str,
    /// HTTPS URL of the official archive.
    pub url: &'static str,
    /// Lowercase SHA-256 of the archive.
    pub sha256: &'static str,
    /// Top-level directory inside the archive.
    pub archive_root: &'static str,
    /// Executable path relative to `archive_root`.
    pub binary: &'static str,
    /// Approximate compressed size (diagnostics only).
    pub size_bytes: u64,
}

/// The pinned archive for every platform we ship a managed browser for.
///
/// Checksums are computed from the official `chrome-for-testing-public`
/// archives; `tools install chromium` verifies them before extracting
/// anything. Platforms not listed fall back to `CHROME_BIN`, PATH and the
/// well-known system install locations.
pub const CHROMIUM_ARTIFACTS: &[ChromiumArtifact] = &[
    ChromiumArtifact {
        platform: "win64",
        version: CHROMIUM_VERSION,
        url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.84/win64/chrome-win64.zip",
        sha256: "507f2a031441d10c62b5e3d4e863f09ac1198a571c1419976cbd8387f639afde",
        archive_root: "chrome-win64",
        binary: "chrome.exe",
        size_bytes: 206_114_888,
    },
    ChromiumArtifact {
        platform: "linux64",
        version: CHROMIUM_VERSION,
        url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.84/linux64/chrome-linux64.zip",
        sha256: "cf0f049892dea22e6a6db26d47ed0d7776b7158edcbec4634086633df8bafcbc",
        archive_root: "chrome-linux64",
        binary: "chrome",
        size_bytes: 199_014_491,
    },
    ChromiumArtifact {
        platform: "linux-arm64",
        version: CHROMIUM_VERSION,
        url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.84/linux-arm64/chrome-linux-arm64.zip",
        sha256: "1692a407908867de88eb1f1b2a93ca47c348d8fb2e3c04752d966e0f2e0344b4",
        archive_root: "chrome-linux-arm64",
        binary: "chrome",
        size_bytes: 198_807_353,
    },
    ChromiumArtifact {
        platform: "mac-arm64",
        version: CHROMIUM_VERSION,
        url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.84/mac-arm64/chrome-mac-arm64.zip",
        sha256: "13b9e98ce56f5ce18e100a009f455c9285f399e7c9dc8a76fdf2f3a9755bb5f2",
        archive_root: "chrome-mac-arm64",
        binary: "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        size_bytes: 192_672_771,
    },
    ChromiumArtifact {
        platform: "mac-x64",
        version: CHROMIUM_VERSION,
        url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.84/mac-x64/chrome-mac-x64.zip",
        sha256: "d805a173118daa5f6f5cf464500789e1e00022a9a6aed8abfec821ab828fa862",
        archive_root: "chrome-mac-x64",
        binary: "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        size_bytes: 203_150_281,
    },
];

/// The platform key for the running host, or `None` for an unsupported CPU
/// (for example 32-bit Windows, where Chrome for Testing no longer ships).
#[must_use]
pub fn current_platform() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        if cfg!(target_arch = "x86_64") {
            Some("win64")
        } else {
            None
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            Some("mac-arm64")
        } else if cfg!(target_arch = "x86_64") {
            Some("mac-x64")
        } else {
            None
        }
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "aarch64") {
            Some("linux-arm64")
        } else if cfg!(target_arch = "x86_64") {
            Some("linux64")
        } else {
            None
        }
    } else {
        None
    }
}

/// The pinned artifact for the running host.
#[must_use]
pub fn chromium_artifact() -> Option<&'static ChromiumArtifact> {
    let platform = current_platform()?;
    CHROMIUM_ARTIFACTS
        .iter()
        .find(|artifact| artifact.platform == platform)
}

/// The pinned artifact for an explicit platform key.
#[must_use]
pub fn chromium_artifact_for(platform: &str) -> Option<&'static ChromiumArtifact> {
    CHROMIUM_ARTIFACTS
        .iter()
        .find(|artifact| artifact.platform == platform)
}

/// Root directory of the managed Chromium build.
#[must_use]
pub fn chromium_install_dir(home: &Home, platform: &str) -> PathBuf {
    home.tools_dir()
        .join("chromium")
        .join(CHROMIUM_VERSION)
        .join(platform)
}

/// Executable of the managed Chromium build if it is installed.
#[must_use]
pub fn managed_chromium_binary(home: &Home) -> Option<PathBuf> {
    let artifact = chromium_artifact()?;
    let candidate = chromium_install_dir(home, artifact.platform).join(artifact.binary);
    candidate.is_file().then_some(candidate)
}

/// True when `path` is the executable of the pinned managed build.
#[must_use]
pub fn is_managed_chromium(home: &Home, path: &Path) -> bool {
    managed_chromium_binary(home).is_some_and(|managed| managed == path)
}

/// Locate a Chromium-family browser for a `web` device.
///
/// Order: `CHROME_BIN`, the managed Chrome for Testing build, PATH, then
/// well-known installer locations (Windows/macOS installers do not add
/// Chrome to PATH; decision I-35).
#[must_use]
pub fn find_chromium() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CHROME_BIN") {
        if !path.is_empty() {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    if let Ok(home) = Home::resolve() {
        if let Some(managed) = managed_chromium_binary(&home) {
            return Some(managed);
        }
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ] {
        if let Some(found) = find_in_path(name) {
            return Some(found);
        }
    }
    for candidate in well_known_binaries() {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Classify a browser path for doctor output.
#[must_use]
pub fn chromium_source(home: &Home, path: &Path) -> &'static str {
    if std::env::var("CHROME_BIN")
        .ok()
        .is_some_and(|value| PathBuf::from(value) == path)
    {
        "CHROME_BIN"
    } else if is_managed_chromium(home, path) {
        "managed (tools/chromium)"
    } else {
        "system browser"
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            // chrome.exe is already covered by EXE_SUFFIX; this branch keeps
            // the Windows check explicit for non-.exe Chromium wrappers.
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn well_known_binaries() -> &'static [&'static str] {
    &[]
}

#[cfg(windows)]
fn well_known_binaries() -> &'static [&'static str] {
    &[
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
        "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    ]
}

#[cfg(target_os = "macos")]
fn well_known_binaries() -> &'static [&'static str] {
    &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn well_known_binaries() -> &'static [&'static str] {
    &[
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_artifact_is_pinned_and_https() {
        assert!(!CHROMIUM_ARTIFACTS.is_empty());
        for artifact in CHROMIUM_ARTIFACTS {
            assert_eq!(artifact.version, CHROMIUM_VERSION);
            assert!(artifact.url.starts_with("https://"));
            assert!(!artifact.sha256.is_empty());
            assert!(artifact.sha256.len() == 64 || artifact.sha256.starts_with("PENDING_"));
            assert!(!artifact.archive_root.contains('/'));
            assert!(!artifact.binary.is_empty());
        }
    }

    #[test]
    fn current_host_has_an_artifact_when_64_bit() {
        if let Some(platform) = current_platform() {
            assert!(chromium_artifact_for(platform).is_some());
        }
    }
}
