//! Personalized Developer Disk Image mounting.
//!
//! XCUITest and other DVT services are unavailable until the matching
//! `/System/Developer` image is mounted. A tvOS DDI is build- and
//! device-specific; Impresario consumes a directory containing the three files
//! Apple's personalization produces. Nothing is downloaded or shelled out here.

use std::path::{Path, PathBuf};

use idevice::services::mobile_image_mounter::ImageMounter;
use idevice::RsdService as _;

use crate::tunnel::AppleTunnel;
use crate::{Error, Result};

/// The three files of a personalized DDI.
#[derive(Debug, Clone)]
pub struct DdiAssets {
    /// `Image.dmg`
    pub image: PathBuf,
    /// `Image.dmg.trustcache`
    pub trust_cache: PathBuf,
    /// `BuildManifest.plist`
    pub build_manifest: PathBuf,
}

impl DdiAssets {
    /// Resolve the conventional `Image.dmg`, `Image.dmg.trustcache` and
    /// `BuildManifest.plist` names inside `dir`.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let assets = Self {
            image: dir.join("Image.dmg"),
            trust_cache: dir.join("Image.dmg.trustcache"),
            build_manifest: dir.join("BuildManifest.plist"),
        };
        for path in [&assets.image, &assets.trust_cache, &assets.build_manifest] {
            if !path.is_file() {
                return Err(Error::Device(format!(
                    "DDI file {} is missing; place a personalized tvOS DDI there \
                     (`Image.dmg`, `Image.dmg.trustcache`, `BuildManifest.plist`)",
                    path.display()
                )));
            }
        }
        Ok(assets)
    }

    /// Read the three files for the mounter.
    fn read(&self) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let image = std::fs::read(&self.image)
            .map_err(|error| Error::io(format!("read {}", self.image.display()), error))?;
        let trust_cache = std::fs::read(&self.trust_cache)
            .map_err(|error| Error::io(format!("read {}", self.trust_cache.display()), error))?;
        let build_manifest = std::fs::read(&self.build_manifest)
            .map_err(|error| Error::io(format!("read {}", self.build_manifest.display()), error))?;
        Ok((image, trust_cache, build_manifest))
    }
}

/// Whether the developer image is already mounted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdiState {
    Mounted,
    NotMounted,
}

/// Query `/System/Developer` on the tunnel.
pub async fn query(tunnel: &mut AppleTunnel) -> Result<DdiState> {
    let mut mounter = ImageMounter::connect_rsd(&mut tunnel.handle, &mut tunnel.handshake)
        .await
        .map_err(|error| Error::Device(format!("developer image service: {error}")))?;
    let images = mounter
        .copy_devices()
        .await
        .map_err(|error| Error::Device(format!("list mounted images: {error}")))?;
    Ok(if mounted(&images) {
        DdiState::Mounted
    } else {
        DdiState::NotMounted
    })
}

fn mounted(images: &[plist::Value]) -> bool {
    images.iter().any(|image| {
        let Some(dict) = image.as_dictionary() else {
            return false;
        };
        let path = dict.get("MountPath").and_then(plist::Value::as_string);
        let is_mounted = dict
            .get("IsMounted")
            .and_then(plist::Value::as_boolean)
            .unwrap_or(false);
        path == Some("/System/Developer") && is_mounted
    })
}

/// Mount `assets` if `/System/Developer` is not already mounted.
///
/// The device's `UniqueChipID` and `BuildVersion` are read from RSD; the assets
/// must be personalized for that device/build. Returns the state before the
/// operation so callers can report whether they actually mounted.
pub async fn ensure_mounted(tunnel: &mut AppleTunnel, assets: &DdiAssets) -> Result<DdiState> {
    let before = query(tunnel).await?;
    if before == DdiState::Mounted {
        return Ok(before);
    }
    let ecid = tunnel.unique_chip_id().ok_or_else(|| {
        Error::Device("RSD did not report UniqueChipID; cannot personalize a DDI".into())
    })?;
    let build = tunnel
        .build_version()
        .ok_or_else(|| {
            Error::Device("RSD did not report BuildVersion; cannot select a DDI".into())
        })?
        .to_string();
    let (image, trust_cache, build_manifest) = assets.read()?;
    tracing::info!(
        device = %tunnel.device_name(),
        ecid,
        build,
        image_bytes = image.len(),
        "mounting personalized developer image"
    );
    let mut mounter = ImageMounter::connect_rsd(&mut tunnel.handle, &mut tunnel.handshake)
        .await
        .map_err(|error| Error::Device(format!("developer image service: {error}")))?;
    mounter
        .mount_personalized_rsd(
            &mut tunnel.handle,
            &mut tunnel.handshake,
            image,
            trust_cache,
            &build_manifest,
            None,
            ecid,
        )
        .await
        .map_err(|error| {
            Error::Device(format!(
                "mount personalized DDI for {build} (ECID {ecid}): {error}"
            ))
        })?;
    Ok(before)
}

/// Unmount `/System/Developer` (used by a clean reset or diagnostics).
pub async fn unmount(tunnel: &mut AppleTunnel) -> Result<()> {
    let mut mounter = ImageMounter::connect_rsd(&mut tunnel.handle, &mut tunnel.handshake)
        .await
        .map_err(|error| Error::Device(format!("developer image service: {error}")))?;
    mounter
        .unmount_image("/System/Developer")
        .await
        .map_err(|error| Error::Device(format!("unmount developer image: {error}")))
}

/// Test the pure /System/Developer decision without a device.
#[must_use]
pub fn mounted_from_values(path: Option<&str>, is_mounted: bool) -> bool {
    path == Some("/System/Developer") && is_mounted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_developer_mount_counts() {
        assert!(mounted_from_values(Some("/System/Developer"), true));
        assert!(!mounted_from_values(Some("/System/Developer"), false));
        assert!(!mounted_from_values(Some("/private/var/other"), true));
        assert!(!mounted_from_values(None, true));
    }
}
