//! Remote/hardware keys: the DSL names and the per-platform dispatch table
//! (spec section 6.2). This is the single source of truth for key codes;
//! the executor key tables and `docs/PARITY.md` are checked against it.

use serde::{Deserialize, Serialize};

use crate::platforms::Platform;

/// A key name from the DSL `key` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteKey {
    Ok,
    Enter,
    Back,
    Up,
    Down,
    Left,
    Right,
    Home,
    Menu,
    Play,
    Pause,
    PlayPause,
    Rewind,
    FastForward,
    Red,
    Green,
    Yellow,
    Blue,
    Backspace,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    VolumeUp,
    VolumeDown,
    Mute,
    ChannelUp,
    ChannelDown,
    Stop,
    Record,
    Info,
    Guide,
    Exit,
    Subtitle,
    Audio,
    Search,
    Settings,
}

impl RemoteKey {
    /// All keys in table order.
    pub const ALL: [RemoteKey; 43] = [
        RemoteKey::Ok,
        RemoteKey::Enter,
        RemoteKey::Back,
        RemoteKey::Up,
        RemoteKey::Down,
        RemoteKey::Left,
        RemoteKey::Right,
        RemoteKey::Home,
        RemoteKey::Menu,
        RemoteKey::Play,
        RemoteKey::Pause,
        RemoteKey::PlayPause,
        RemoteKey::Rewind,
        RemoteKey::FastForward,
        RemoteKey::Red,
        RemoteKey::Green,
        RemoteKey::Yellow,
        RemoteKey::Blue,
        RemoteKey::Backspace,
        RemoteKey::Num0,
        RemoteKey::Num1,
        RemoteKey::Num2,
        RemoteKey::Num3,
        RemoteKey::Num4,
        RemoteKey::Num5,
        RemoteKey::Num6,
        RemoteKey::Num7,
        RemoteKey::Num8,
        RemoteKey::Num9,
        RemoteKey::VolumeUp,
        RemoteKey::VolumeDown,
        RemoteKey::Mute,
        RemoteKey::ChannelUp,
        RemoteKey::ChannelDown,
        RemoteKey::Stop,
        RemoteKey::Record,
        RemoteKey::Info,
        RemoteKey::Guide,
        RemoteKey::Exit,
        RemoteKey::Subtitle,
        RemoteKey::Audio,
        RemoteKey::Search,
        RemoteKey::Settings,
    ];

    /// DSL spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RemoteKey::Ok => "ok",
            RemoteKey::Enter => "enter",
            RemoteKey::Back => "back",
            RemoteKey::Up => "up",
            RemoteKey::Down => "down",
            RemoteKey::Left => "left",
            RemoteKey::Right => "right",
            RemoteKey::Home => "home",
            RemoteKey::Menu => "menu",
            RemoteKey::Play => "play",
            RemoteKey::Pause => "pause",
            RemoteKey::PlayPause => "playpause",
            RemoteKey::Rewind => "rewind",
            RemoteKey::FastForward => "fastforward",
            RemoteKey::Red => "red",
            RemoteKey::Green => "green",
            RemoteKey::Yellow => "yellow",
            RemoteKey::Blue => "blue",
            RemoteKey::Backspace => "backspace",
            RemoteKey::Num0 => "num0",
            RemoteKey::Num1 => "num1",
            RemoteKey::Num2 => "num2",
            RemoteKey::Num3 => "num3",
            RemoteKey::Num4 => "num4",
            RemoteKey::Num5 => "num5",
            RemoteKey::Num6 => "num6",
            RemoteKey::Num7 => "num7",
            RemoteKey::Num8 => "num8",
            RemoteKey::Num9 => "num9",
            RemoteKey::VolumeUp => "volumeup",
            RemoteKey::VolumeDown => "volumedown",
            RemoteKey::Mute => "mute",
            RemoteKey::ChannelUp => "channelup",
            RemoteKey::ChannelDown => "channeldown",
            RemoteKey::Stop => "stop",
            RemoteKey::Record => "record",
            RemoteKey::Info => "info",
            RemoteKey::Guide => "guide",
            RemoteKey::Exit => "exit",
            RemoteKey::Subtitle => "subtitle",
            RemoteKey::Audio => "audio",
            RemoteKey::Search => "search",
            RemoteKey::Settings => "settings",
        }
    }

    /// Parse a DSL key name (case-sensitive).
    #[must_use]
    pub fn from_dsl(name: &str) -> Option<Self> {
        RemoteKey::ALL.iter().copied().find(|k| k.as_str() == name)
    }

    /// The web DOM `keyCode` for `web`, `webos` and `tizen` per table 6.2,
    /// with the DOM `key`/`code` names used for CDP dispatch.
    #[must_use]
    pub fn web(self, platform: Platform) -> Option<WebKey> {
        let is_plain_web = matches!(platform, Platform::Web);
        let named = |key_code: u32, key: &'static str, code: &'static str| WebKey {
            key_code,
            key,
            code,
            windows_vk: key_code,
            native_vk: key_code,
        };
        let digit =
            |value: u32, key: &'static str, code: &'static str| Some(named(48 + value, key, code));
        match self {
            RemoteKey::Ok => Some(named(13, "Enter", "Enter")),
            RemoteKey::Enter => Some(named(13, "Enter", "Enter")),
            RemoteKey::Up => Some(named(38, "ArrowUp", "ArrowUp")),
            RemoteKey::Down => Some(named(40, "ArrowDown", "ArrowDown")),
            RemoteKey::Left => Some(named(37, "ArrowLeft", "ArrowLeft")),
            RemoteKey::Right => Some(named(39, "ArrowRight", "ArrowRight")),
            RemoteKey::Backspace => Some(named(8, "Backspace", "Backspace")),
            RemoteKey::Back if !is_plain_web => Some(match platform {
                Platform::WebOs => named(461, "Back", ""),
                _ => named(10009, "Back", ""),
            }),
            RemoteKey::Home if is_plain_web => Some(named(36, "Home", "Home")),
            RemoteKey::Menu if is_plain_web => Some(named(93, "ContextMenu", "ContextMenu")),
            RemoteKey::Play => Some(named(415, "MediaPlay", "MediaPlay")),
            RemoteKey::Pause => Some(named(19, "MediaPause", "MediaPause")),
            RemoteKey::PlayPause => Some(named(179, "MediaPlayPause", "MediaPlayPause")),
            RemoteKey::Rewind => Some(named(412, "MediaRewind", "MediaRewind")),
            RemoteKey::FastForward => Some(named(417, "MediaFastForward", "MediaFastForward")),
            RemoteKey::Stop => Some(named(413, "MediaStop", "MediaStop")),
            RemoteKey::Record => Some(named(416, "MediaRecord", "MediaRecord")),
            RemoteKey::Red => Some(named(403, "ColorF0Red", "ColorF0Red")),
            RemoteKey::Green => Some(named(404, "ColorF1Green", "ColorF1Green")),
            RemoteKey::Yellow => Some(named(405, "ColorF2Yellow", "ColorF2Yellow")),
            RemoteKey::Blue => Some(named(406, "ColorF3Blue", "ColorF3Blue")),
            RemoteKey::Num0 => digit(0, "0", "Digit0"),
            RemoteKey::Num1 => digit(1, "1", "Digit1"),
            RemoteKey::Num2 => digit(2, "2", "Digit2"),
            RemoteKey::Num3 => digit(3, "3", "Digit3"),
            RemoteKey::Num4 => digit(4, "4", "Digit4"),
            RemoteKey::Num5 => digit(5, "5", "Digit5"),
            RemoteKey::Num6 => digit(6, "6", "Digit6"),
            RemoteKey::Num7 => digit(7, "7", "Digit7"),
            RemoteKey::Num8 => digit(8, "8", "Digit8"),
            RemoteKey::Num9 => digit(9, "9", "Digit9"),
            RemoteKey::VolumeUp => Some(named(447, "AudioVolumeUp", "AudioVolumeUp")),
            RemoteKey::VolumeDown => Some(named(448, "AudioVolumeDown", "AudioVolumeDown")),
            RemoteKey::Mute => Some(named(449, "AudioVolumeMute", "AudioVolumeMute")),
            RemoteKey::ChannelUp => Some(named(427, "ChannelUp", "")),
            RemoteKey::ChannelDown => Some(named(428, "ChannelDown", "")),
            RemoteKey::Info => Some(named(457, "Info", "")),
            RemoteKey::Guide => Some(named(458, "Guide", "")),
            RemoteKey::Exit if is_plain_web => Some(named(27, "Escape", "Escape")),
            RemoteKey::Exit => Some(named(461, "Exit", "")),
            RemoteKey::Subtitle => Some(named(460, "Subtitle", "")),
            RemoteKey::Audio => Some(named(459, "Audio", "")),
            RemoteKey::Search => Some(named(84, "Search", "Search")),
            RemoteKey::Settings => Some(named(176, "Settings", "Settings")),
            _ => None,
        }
    }

    /// The Android `KeyEvent` code (table 6.2; `androidtv` adds D-pad center
    /// and colour keys).
    #[must_use]
    pub fn android(self, platform: Platform) -> Option<i32> {
        let tv = matches!(platform, Platform::AndroidTv);
        match self {
            RemoteKey::Ok if tv => Some(23), // KEYCODE_DPAD_CENTER
            RemoteKey::Enter => Some(66),
            RemoteKey::Back => Some(4),
            RemoteKey::Up => Some(19),
            RemoteKey::Down => Some(20),
            RemoteKey::Left => Some(21),
            RemoteKey::Right => Some(22),
            RemoteKey::Home => Some(3),
            RemoteKey::Menu => Some(82),
            RemoteKey::Play => Some(126),
            RemoteKey::Pause => Some(127),
            RemoteKey::PlayPause => Some(85),
            RemoteKey::Rewind => Some(89),
            RemoteKey::FastForward => Some(90),
            RemoteKey::Stop => Some(86),
            RemoteKey::Record => Some(130),
            RemoteKey::Red if tv => Some(183),
            RemoteKey::Green if tv => Some(184),
            RemoteKey::Yellow if tv => Some(185),
            RemoteKey::Blue if tv => Some(186),
            RemoteKey::Backspace => Some(67),
            RemoteKey::Num0 => Some(7),
            RemoteKey::Num1 => Some(8),
            RemoteKey::Num2 => Some(9),
            RemoteKey::Num3 => Some(10),
            RemoteKey::Num4 => Some(11),
            RemoteKey::Num5 => Some(12),
            RemoteKey::Num6 => Some(13),
            RemoteKey::Num7 => Some(14),
            RemoteKey::Num8 => Some(15),
            RemoteKey::Num9 => Some(16),
            RemoteKey::VolumeUp => Some(24),
            RemoteKey::VolumeDown => Some(25),
            RemoteKey::Mute => Some(164),
            RemoteKey::ChannelUp => Some(166),
            RemoteKey::ChannelDown => Some(167),
            RemoteKey::Info => Some(165),
            RemoteKey::Guide => Some(172),
            RemoteKey::Exit => Some(111),
            RemoteKey::Subtitle => Some(175),
            RemoteKey::Audio => Some(222),
            RemoteKey::Search => Some(84),
            RemoteKey::Settings => Some(176),
            _ => None,
        }
    }

    /// iOS support: only `home` (`XCUIDevice.press(.home)`).
    #[must_use]
    pub fn ios(self) -> bool {
        matches!(self, RemoteKey::Home)
    }

    /// The tvOS `XCUIRemote.Button` case name (spec spelling, lowerCamel).
    #[must_use]
    pub fn tvos(self) -> Option<&'static str> {
        match self {
            RemoteKey::Ok => Some("select"),
            RemoteKey::Back => Some("menu"),
            RemoteKey::Up => Some("up"),
            RemoteKey::Down => Some("down"),
            RemoteKey::Left => Some("left"),
            RemoteKey::Right => Some("right"),
            RemoteKey::Home => Some("home"),
            RemoteKey::PlayPause => Some("playPause"),
            _ => None,
        }
    }

    /// Whether pressing this key is supported on `platform`.
    #[must_use]
    pub fn supports(self, platform: Platform) -> bool {
        if platform.is_web_runtime() {
            self.web(platform).is_some()
        } else if platform.is_android_family() {
            self.android(platform).is_some()
        } else if platform == Platform::Ios {
            self.ios()
        } else {
            self.tvos().is_some()
        }
    }

    /// Platforms on which this key works (for diagnostics).
    #[must_use]
    pub fn supported_platforms(self) -> Vec<Platform> {
        Platform::ALL
            .iter()
            .copied()
            .filter(|p| self.supports(*p))
            .collect()
    }
}

/// Web dispatch metadata for one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebKey {
    pub key_code: u32,
    pub key: &'static str,
    pub code: &'static str,
    pub windows_vk: u32,
    pub native_vk: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsl_names_round_trip() {
        for key in RemoteKey::ALL {
            assert_eq!(RemoteKey::from_dsl(key.as_str()), Some(key));
        }
        assert_eq!(RemoteKey::from_dsl("OK"), None);
    }

    #[test]
    fn table_62_web_codes() {
        let expect = |key: RemoteKey, web: Option<u32>, tv: Option<u32>| {
            assert_eq!(
                key.web(Platform::Web).map(|k| k.key_code),
                web,
                "{} web",
                key.as_str()
            );
            assert_eq!(
                key.web(Platform::WebOs).map(|k| k.key_code),
                tv,
                "{} webos",
                key.as_str()
            );
            assert_eq!(
                key.web(Platform::Tizen).map(|k| k.key_code),
                tv,
                "{} tizen",
                key.as_str()
            );
        };
        expect(RemoteKey::Ok, Some(13), Some(13));
        expect(RemoteKey::Enter, Some(13), Some(13));
        // `back` has different codes on webos (461) and tizen (10009).
        assert_eq!(RemoteKey::Back.web(Platform::Web).map(|k| k.key_code), None);
        assert_eq!(
            RemoteKey::Back.web(Platform::WebOs).map(|k| k.key_code),
            Some(461)
        );
        expect(RemoteKey::Up, Some(38), Some(38));
        expect(RemoteKey::Down, Some(40), Some(40));
        expect(RemoteKey::Left, Some(37), Some(37));
        expect(RemoteKey::Right, Some(39), Some(39));
        expect(RemoteKey::Home, Some(36), None);
        expect(RemoteKey::Menu, Some(93), None);
        expect(RemoteKey::Play, Some(415), Some(415));
        expect(RemoteKey::Pause, Some(19), Some(19));
        expect(RemoteKey::PlayPause, Some(179), Some(179));
        expect(RemoteKey::Rewind, Some(412), Some(412));
        expect(RemoteKey::FastForward, Some(417), Some(417));
        expect(RemoteKey::Red, Some(403), Some(403));
        expect(RemoteKey::Green, Some(404), Some(404));
        expect(RemoteKey::Yellow, Some(405), Some(405));
        expect(RemoteKey::Blue, Some(406), Some(406));
        expect(RemoteKey::Backspace, Some(8), Some(8));
        expect(RemoteKey::Num0, Some(48), Some(48));
        expect(RemoteKey::Num9, Some(57), Some(57));
        expect(RemoteKey::VolumeUp, Some(447), Some(447));
        expect(RemoteKey::Mute, Some(449), Some(449));
        expect(RemoteKey::ChannelUp, Some(427), Some(427));
        expect(RemoteKey::Stop, Some(413), Some(413));
        expect(RemoteKey::Record, Some(416), Some(416));
        expect(RemoteKey::Info, Some(457), Some(457));
        expect(RemoteKey::Guide, Some(458), Some(458));
        expect(RemoteKey::Exit, Some(27), Some(461));
        expect(RemoteKey::Subtitle, Some(460), Some(460));
        expect(RemoteKey::Audio, Some(459), Some(459));
        expect(RemoteKey::Search, Some(84), Some(84));
        expect(RemoteKey::Settings, Some(176), Some(176));
        // tizen back differs from webos
        assert_eq!(
            RemoteKey::Back.web(Platform::Tizen).map(|k| k.key_code),
            Some(10009)
        );
    }

    #[test]
    fn table_62_android_codes() {
        let expect = |key: RemoteKey, phone: Option<i32>, tv: Option<i32>| {
            assert_eq!(
                key.android(Platform::Android),
                phone,
                "{} android",
                key.as_str()
            );
            assert_eq!(
                key.android(Platform::AndroidTv),
                tv,
                "{} androidtv",
                key.as_str()
            );
        };
        expect(RemoteKey::Ok, None, Some(23));
        expect(RemoteKey::Enter, Some(66), Some(66));
        expect(RemoteKey::Back, Some(4), Some(4));
        expect(RemoteKey::Up, Some(19), Some(19));
        expect(RemoteKey::Down, Some(20), Some(20));
        expect(RemoteKey::Left, Some(21), Some(21));
        expect(RemoteKey::Right, Some(22), Some(22));
        expect(RemoteKey::Home, Some(3), Some(3));
        expect(RemoteKey::Menu, Some(82), Some(82));
        expect(RemoteKey::Play, Some(126), Some(126));
        expect(RemoteKey::Pause, Some(127), Some(127));
        expect(RemoteKey::PlayPause, Some(85), Some(85));
        expect(RemoteKey::Rewind, Some(89), Some(89));
        expect(RemoteKey::FastForward, Some(90), Some(90));
        expect(RemoteKey::Red, None, Some(183));
        expect(RemoteKey::Green, None, Some(184));
        expect(RemoteKey::Yellow, None, Some(185));
        expect(RemoteKey::Blue, None, Some(186));
        expect(RemoteKey::Backspace, Some(67), Some(67));
    }

    #[test]
    fn table_62_apple() {
        assert!(RemoteKey::Home.ios());
        assert!(!RemoteKey::Ok.ios());
        assert_eq!(RemoteKey::Ok.tvos(), Some("select"));
        assert_eq!(RemoteKey::Back.tvos(), Some("menu"));
        assert_eq!(RemoteKey::Up.tvos(), Some("up"));
        assert_eq!(RemoteKey::PlayPause.tvos(), Some("playPause"));
        assert_eq!(RemoteKey::Home.tvos(), Some("home"));
        assert_eq!(RemoteKey::Enter.tvos(), None);
        assert_eq!(RemoteKey::Menu.tvos(), None);
        assert_eq!(RemoteKey::Red.tvos(), None);
    }

    #[test]
    fn support_matrix_spot_checks() {
        assert!(RemoteKey::Ok.supports(Platform::WebOs));
        assert!(!RemoteKey::Ok.supports(Platform::Android));
        assert!(RemoteKey::Ok.supports(Platform::AndroidTv));
        assert!(RemoteKey::Ok.supports(Platform::Tvos));
        assert!(RemoteKey::Rewind.supports(Platform::Web));
        assert!(RemoteKey::Rewind.supports(Platform::Tizen));
        assert!(RemoteKey::Home.supports(Platform::Ios));
        assert!(!RemoteKey::Backspace.supports(Platform::Tvos));
        assert!(!RemoteKey::Back.supports(Platform::Web));
        assert!(RemoteKey::Num5.supports(Platform::AndroidTv));
        assert!(RemoteKey::VolumeUp.supports(Platform::WebOs));
        assert!(RemoteKey::Exit.supports(Platform::Web));
        assert!(!RemoteKey::Exit.supports(Platform::Ios));
    }
}
