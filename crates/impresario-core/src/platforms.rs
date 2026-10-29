//! Device platforms and the selector fallback chain (D-07).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The exact `type:` spelling accepted in a devices file.
///
/// The DSL keeps its historical lowercase platform keys (selectors, scene
/// columns), but device configuration uses the product names below and only
/// these spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DeviceType {
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "webOS")]
    WebOs,
    #[serde(rename = "tizenOS")]
    Tizen,
    #[serde(rename = "android")]
    Android,
    #[serde(rename = "androidtv")]
    AndroidTv,
    #[serde(rename = "iOS")]
    Ios,
    #[serde(rename = "tvOS")]
    Tvos,
}

impl DeviceType {
    /// All device types in declaration order.
    pub const ALL: [DeviceType; 7] = [
        DeviceType::Web,
        DeviceType::WebOs,
        DeviceType::Tizen,
        DeviceType::Android,
        DeviceType::AndroidTv,
        DeviceType::Ios,
        DeviceType::Tvos,
    ];

    /// The exact config spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceType::Web => "web",
            DeviceType::WebOs => "webOS",
            DeviceType::Tizen => "tizenOS",
            DeviceType::Android => "android",
            DeviceType::AndroidTv => "androidtv",
            DeviceType::Ios => "iOS",
            DeviceType::Tvos => "tvOS",
        }
    }

    /// Map the config type to the internal platform family.
    #[must_use]
    pub fn platform(self) -> Platform {
        match self {
            DeviceType::Web => Platform::Web,
            DeviceType::WebOs => Platform::WebOs,
            DeviceType::Tizen => Platform::Tizen,
            DeviceType::Android => Platform::Android,
            DeviceType::AndroidTv => Platform::AndroidTv,
            DeviceType::Ios => Platform::Ios,
            DeviceType::Tvos => Platform::Tvos,
        }
    }

    /// The config type for an internal platform.
    #[must_use]
    pub fn from_platform(platform: Platform) -> Self {
        match platform {
            Platform::Web => DeviceType::Web,
            Platform::WebOs => DeviceType::WebOs,
            Platform::Tizen => DeviceType::Tizen,
            Platform::Android => DeviceType::Android,
            Platform::AndroidTv => DeviceType::AndroidTv,
            Platform::Ios => DeviceType::Ios,
            Platform::Tvos => DeviceType::Tvos,
        }
    }

    /// A short, lowercase slug used to derive a default device name.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            DeviceType::Web => "web",
            DeviceType::WebOs => "webos",
            DeviceType::Tizen => "tizen",
            DeviceType::Android => "android",
            DeviceType::AndroidTv => "androidtv",
            DeviceType::Ios => "ios",
            DeviceType::Tvos => "tvos",
        }
    }
}

impl fmt::Display for DeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DeviceType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "web" => Ok(DeviceType::Web),
            "webOS" => Ok(DeviceType::WebOs),
            "tizenOS" => Ok(DeviceType::Tizen),
            "android" => Ok(DeviceType::Android),
            "androidtv" => Ok(DeviceType::AndroidTv),
            "iOS" => Ok(DeviceType::Ios),
            "tvOS" => Ok(DeviceType::Tvos),
            _ => Err(()),
        }
    }
}

/// Every device platform the system knows (spec section 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Web,
    WebOs,
    Tizen,
    Android,
    AndroidTv,
    Ios,
    Tvos,
}

impl Platform {
    /// All platforms, in declaration order.
    pub const ALL: [Platform; 7] = [
        Platform::Web,
        Platform::WebOs,
        Platform::Tizen,
        Platform::Android,
        Platform::AndroidTv,
        Platform::Ios,
        Platform::Tvos,
    ];

    /// The DSL/config spelling (also the selector key, D-07).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Web => "web",
            Platform::WebOs => "webos",
            Platform::Tizen => "tizen",
            Platform::Android => "android",
            Platform::AndroidTv => "androidtv",
            Platform::Ios => "ios",
            Platform::Tvos => "tvos",
        }
    }

    /// Fallback chain for selector resolution (D-07):
    /// `webos→web`, `tizen→web`, `androidtv→android`, `tvos→ios`.
    ///
    /// Returns the keys to try in order (the platform itself first).
    #[must_use]
    pub fn selector_chain(self) -> &'static [Platform] {
        match self {
            Platform::WebOs => &[Platform::WebOs, Platform::Web],
            Platform::Tizen => &[Platform::Tizen, Platform::Web],
            Platform::AndroidTv => &[Platform::AndroidTv, Platform::Android],
            Platform::Tvos => &[Platform::Tvos, Platform::Ios],
            Platform::Web => &[Platform::Web],
            Platform::Android => &[Platform::Android],
            Platform::Ios => &[Platform::Ios],
        }
    }

    /// True for TV-style focus-driven platforms (section 6.3).
    #[must_use]
    pub fn is_tv(self) -> bool {
        matches!(
            self,
            Platform::WebOs | Platform::Tizen | Platform::AndroidTv | Platform::Tvos
        )
    }

    /// True for touch platforms with pointer gestures.
    #[must_use]
    pub fn is_touch(self) -> bool {
        matches!(self, Platform::Android | Platform::Ios | Platform::Web)
    }

    /// The web-runtime family driven via CDP by the connector.
    #[must_use]
    pub fn is_web_runtime(self) -> bool {
        matches!(self, Platform::Web | Platform::WebOs | Platform::Tizen)
    }

    /// The native Android family.
    #[must_use]
    pub fn is_android_family(self) -> bool {
        matches!(self, Platform::Android | Platform::AndroidTv)
    }

    /// The Apple family.
    #[must_use]
    pub fn is_apple_family(self) -> bool {
        matches!(self, Platform::Ios | Platform::Tvos)
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "web" => Ok(Platform::Web),
            "webos" => Ok(Platform::WebOs),
            "tizen" => Ok(Platform::Tizen),
            "android" => Ok(Platform::Android),
            "androidtv" => Ok(Platform::AndroidTv),
            "ios" => Ok(Platform::Ios),
            "tvos" => Ok(Platform::Tvos),
            _ => Err(()),
        }
    }
}

/// The set of platforms a scene applies to (`cross-platform` or a list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScenePlatforms {
    /// Applies to every device.
    CrossPlatform,
    /// Applies to the listed platforms only (exact match, no families).
    List(Vec<Platform>),
}

impl ScenePlatforms {
    #[must_use]
    pub fn applies_to(&self, platform: Platform) -> bool {
        match self {
            ScenePlatforms::CrossPlatform => true,
            ScenePlatforms::List(list) => list.contains(&platform),
        }
    }
}

/// Top-level device families used for transport selection (spec section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFamily {
    Connector,
    Adb,
    Apple,
}

impl Platform {
    #[must_use]
    pub fn family(self) -> TransportFamily {
        if self.is_web_runtime() {
            TransportFamily::Connector
        } else if self.is_android_family() {
            TransportFamily::Adb
        } else {
            TransportFamily::Apple
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_names() {
        for platform in Platform::ALL {
            assert_eq!(platform.as_str().parse::<Platform>(), Ok(platform));
        }
        assert_eq!("bogus".parse::<Platform>(), Err(()));
    }

    #[test]
    fn fallback_chain_matches_d07() {
        assert_eq!(
            Platform::WebOs.selector_chain(),
            &[Platform::WebOs, Platform::Web]
        );
        assert_eq!(
            Platform::Tvos.selector_chain(),
            &[Platform::Tvos, Platform::Ios]
        );
        assert_eq!(
            Platform::AndroidTv.selector_chain(),
            &[Platform::AndroidTv, Platform::Android]
        );
        assert_eq!(Platform::Web.selector_chain(), &[Platform::Web]);
        assert_eq!(Platform::Ios.selector_chain(), &[Platform::Ios]);
    }

    #[test]
    fn scene_applicability_is_exact() {
        let list = ScenePlatforms::List(vec![Platform::WebOs]);
        assert!(list.applies_to(Platform::WebOs));
        assert!(
            !list.applies_to(Platform::Web),
            "no implicit family matching"
        );
        assert!(ScenePlatforms::CrossPlatform.applies_to(Platform::Tvos));
    }

    #[test]
    fn families() {
        assert_eq!(Platform::WebOs.family(), TransportFamily::Connector);
        assert_eq!(Platform::AndroidTv.family(), TransportFamily::Adb);
        assert_eq!(Platform::Tvos.family(), TransportFamily::Apple);
        assert!(Platform::AndroidTv.is_tv());
        assert!(Platform::Tizen.is_web_runtime());
        assert!(Platform::Ios.is_touch());
        assert!(Platform::Web.is_touch());
    }
}
