#[derive(Debug, Clone)]
pub enum DeveloperDeviceType {
    Any,
    Ios,
    Tvos,
    Watchos,
}

impl DeveloperDeviceType {
    /// Apple's developer-services API exposes one `ios/` endpoint for every
    /// device family. Platform-specific behavior is selected in the request
    /// body (for example, a tvOS profile uses `subPlatform = tvOS`).
    pub fn url_segment(&self) -> &'static str {
        match self {
            DeveloperDeviceType::Any => "",
            DeveloperDeviceType::Ios | DeveloperDeviceType::Tvos | DeveloperDeviceType::Watchos => {
                "ios/"
            }
        }
    }

    /// The `subPlatform` value Apple needs when generating a profile for this
    /// device family. iOS profiles omit the field.
    pub fn profile_sub_platform(&self) -> Option<&'static str> {
        match self {
            DeveloperDeviceType::Any | DeveloperDeviceType::Ios => None,
            DeveloperDeviceType::Tvos => Some("tvOS"),
            DeveloperDeviceType::Watchos => Some("watchOS"),
        }
    }
}

pub fn dev_url(endpoint: &str, device_type: impl Into<Option<DeveloperDeviceType>>) -> String {
    format!(
        "https://developerservices2.apple.com/services/QH65B2/{}{}.action?clientId=XABBG36SBA",
        device_type
            .into()
            .unwrap_or(DeveloperDeviceType::Ios)
            .url_segment(),
        endpoint,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_device_families_use_the_ios_service_path() {
        for device_type in [
            DeveloperDeviceType::Ios,
            DeveloperDeviceType::Tvos,
            DeveloperDeviceType::Watchos,
        ] {
            assert_eq!(
                dev_url("listDevices", device_type),
                "https://developerservices2.apple.com/services/QH65B2/ios/listDevices.action?clientId=XABBG36SBA"
            );
        }
    }

    #[test]
    fn any_omits_the_service_path() {
        assert_eq!(
            dev_url("listTeams", DeveloperDeviceType::Any),
            "https://developerservices2.apple.com/services/QH65B2/listTeams.action?clientId=XABBG36SBA"
        );
    }

    #[test]
    fn tvos_profiles_request_the_tvos_sub_platform() {
        assert_eq!(
            DeveloperDeviceType::Tvos.profile_sub_platform(),
            Some("tvOS")
        );
        assert_eq!(DeveloperDeviceType::Ios.profile_sub_platform(), None);
    }
}
