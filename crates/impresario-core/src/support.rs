//! The action/check support matrix of spec section 6.1 (single source of
//! truth for planner detection of `IMP-DSL-013` and for `docs/PARITY.md`).

use std::fmt;

use crate::keys::RemoteKey;
use crate::platforms::Platform;

/// The verbs of the DSL, as used by the support matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    Click,
    Type,
    Append,
    Clear,
    LongPress,
    Focus,
    Select,
    Scroll,
    ScrollTo,
    Swipe,
    Back,
    Key,
    Wait,
    Screenshot,
    AppLaunch,
    AppTerminate,
    AppRestart,
    AppBackground,
}

impl Verb {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Verb::Click => "click",
            Verb::Type => "type",
            Verb::Append => "append",
            Verb::Clear => "clear",
            Verb::LongPress => "longpress",
            Verb::Focus => "focus",
            Verb::Select => "select",
            Verb::Scroll => "scroll",
            Verb::ScrollTo => "scrollto",
            Verb::Swipe => "swipe",
            Verb::Back => "back",
            Verb::Key => "key",
            Verb::Wait => "wait",
            Verb::Screenshot => "screenshot",
            Verb::AppLaunch => "app launch",
            Verb::AppTerminate => "app terminate",
            Verb::AppRestart => "app restart",
            Verb::AppBackground => "app background",
        }
    }

    /// True when the verb never crosses the wire (master-side only).
    #[must_use]
    pub fn is_master_side(self) -> bool {
        matches!(self, Verb::Wait)
    }

    /// Every verb, in matrix order (iteration source for tooling).
    pub const ALL: &'static [Verb] = &[
        Verb::Click,
        Verb::Type,
        Verb::Append,
        Verb::Clear,
        Verb::LongPress,
        Verb::Focus,
        Verb::Select,
        Verb::Scroll,
        Verb::ScrollTo,
        Verb::Swipe,
        Verb::Back,
        Verb::Key,
        Verb::Wait,
        Verb::Screenshot,
        Verb::AppLaunch,
        Verb::AppTerminate,
        Verb::AppRestart,
        Verb::AppBackground,
    ];
}

/// Checks of the DSL (spec 5.6 and the second table of 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckVerb {
    Visible,
    Hidden,
    Enabled,
    Disabled,
    Focused,
    TextEquals,
    TextContains,
    Count,
    Showing,
    Player,
}

impl CheckVerb {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CheckVerb::Visible => "visible",
            CheckVerb::Hidden => "hidden",
            CheckVerb::Enabled => "enabled",
            CheckVerb::Disabled => "disabled",
            CheckVerb::Focused => "focused",
            CheckVerb::TextEquals => "text",
            CheckVerb::TextContains => "textContains",
            CheckVerb::Count => "count",
            CheckVerb::Showing => "Showing",
            CheckVerb::Player => "Player",
        }
    }

    /// Every check, in matrix order (iteration source for tooling).
    pub const ALL: &'static [CheckVerb] = &[
        CheckVerb::Visible,
        CheckVerb::Hidden,
        CheckVerb::Enabled,
        CheckVerb::Disabled,
        CheckVerb::Focused,
        CheckVerb::TextEquals,
        CheckVerb::TextContains,
        CheckVerb::Count,
        CheckVerb::Showing,
        CheckVerb::Player,
    ];
}

/// Level of support for an action on a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// ✅ full support.
    Full,
    /// ◐ supported with a documented limitation.
    Limited(&'static str),
    /// ✖ unsupported; planner rejects with `IMP-DSL-013`.
    Unsupported,
}

impl Support {
    #[must_use]
    pub fn is_supported(self) -> bool {
        !matches!(self, Support::Unsupported)
    }

    /// The legend glyph used in PARITY.md.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Support::Full => "✅",
            Support::Limited(_) => "◐",
            Support::Unsupported => "✖",
        }
    }
}

impl fmt::Display for Support {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Support::Full => f.write_str("supported"),
            Support::Limited(note) => write!(f, "limited: {note}"),
            Support::Unsupported => f.write_str("unsupported"),
        }
    }
}

const MOUSE_HOLD: &str = "mouse hold";
const OK_HELD: &str = "OK held";
const INJECTED_OK_HOLD: &str = "injected OK hold";
const DPAD_ONLY: &str = "D-pad only";
const DPAD: &str = "D-pad";
const SAME_AS_FOCUS: &str = "same as focus";
const PROGRAMMATIC: &str = "programmatic";
const FOCUS_OK: &str = "focus+OK";
const FOCUS_SELECT: &str = "focus+select";
const TVOS_KEYBOARD: &str = "requires focus + keyboard";
const WEB_HISTORY: &str = "history";
const NAVBAR_BACK: &str = "nav-bar back button, else edge swipe";
const MENU: &str = "menu";
const OK_KEY: &str = "back key";
const NAVIGATE: &str = "navigate";
const RELOAD_NAV: &str = "reload/navigate";

/// Action support per spec table 6.1 (`key` handled separately per key).
#[must_use]
pub fn action_support(platform: Platform, verb: Verb) -> Support {
    use Platform::{Android, AndroidTv, Ios, Tizen, Tvos, Web, WebOs};
    use Support::{Full, Limited, Unsupported};
    match verb {
        Verb::Click => match platform {
            Web | Android | Ios => Full,
            WebOs | Tizen => Limited(FOCUS_OK),
            AndroidTv => Limited(FOCUS_OK),
            Tvos => Limited(FOCUS_SELECT),
        },
        Verb::Type | Verb::Append | Verb::Clear => match platform {
            Tvos => Limited(TVOS_KEYBOARD),
            WebOs | Tizen => Limited(PROGRAMMATIC),
            _ => Full,
        },
        Verb::LongPress => match platform {
            Web => Limited(MOUSE_HOLD),
            WebOs | Tizen => Limited(OK_HELD),
            Android | Ios | Tvos => Full,
            AndroidTv => Limited(INJECTED_OK_HOLD),
        },
        Verb::Focus => match platform {
            Web | WebOs | Tizen => Full,
            Android | Ios => Unsupported,
            AndroidTv | Tvos => Limited(DPAD_ONLY),
        },
        Verb::Select => match platform {
            Web | WebOs | Tizen => Full,
            Android | AndroidTv | Ios => Limited(DPAD_ONLY),
            Tvos => Unsupported,
        },
        Verb::Scroll => match platform {
            Web | WebOs | Tizen | Android | Ios => Full,
            AndroidTv | Tvos => Limited(DPAD),
        },
        Verb::ScrollTo => match platform {
            Web | WebOs | Tizen | Android | Ios => Full,
            AndroidTv | Tvos => Limited(SAME_AS_FOCUS),
        },
        Verb::Swipe => match platform {
            Android | Ios => Full,
            Web | WebOs | Tizen | AndroidTv | Tvos => Unsupported,
        },
        Verb::Back => match platform {
            Web => Limited(WEB_HISTORY),
            WebOs | Tizen => Limited(OK_KEY),
            Android | AndroidTv => Full,
            Ios => Limited(NAVBAR_BACK),
            Tvos => Limited(MENU),
        },
        // `key` support depends on the key itself (table 6.2).
        Verb::Key => Full,
        // `wait` never crosses the wire (D-10); support is platform-agnostic.
        Verb::Wait => Full,
        Verb::Screenshot => Full,
        Verb::AppLaunch => match platform {
            Web | WebOs | Tizen => Limited(NAVIGATE),
            _ => Full,
        },
        Verb::AppTerminate => match platform {
            Web | WebOs | Tizen => Unsupported,
            _ => Full,
        },
        Verb::AppRestart => match platform {
            Web | WebOs | Tizen => Limited(RELOAD_NAV),
            _ => Full,
        },
        Verb::AppBackground => match platform {
            Web | WebOs | Tizen => Unsupported,
            _ => Full,
        },
    }
}

/// Support for `key <k>` on a platform (table 6.2).
#[must_use]
pub fn key_support(platform: Platform, key: RemoteKey) -> Support {
    if key.supports(platform) {
        Support::Full
    } else {
        Support::Unsupported
    }
}

/// The support-matrix verb of an AST action (shared by validation and the
/// master planner; single source of truth for `IMP-DSL-013`).
#[must_use]
pub fn verb_of_action(verb: &crate::ast::ActionVerb) -> Verb {
    use crate::ast::ActionVerb as A;
    match verb {
        A::Type { .. } => Verb::Type,
        A::Append { .. } => Verb::Append,
        A::Clear { .. } => Verb::Clear,
        A::Click { .. } => Verb::Click,
        A::LongPress { .. } => Verb::LongPress,
        A::Focus { .. } => Verb::Focus,
        A::Select { .. } => Verb::Select,
        A::Scroll { .. } => Verb::Scroll,
        A::ScrollTo { .. } => Verb::ScrollTo,
        A::Swipe { .. } => Verb::Swipe,
        A::Key { .. } => Verb::Key,
        A::Back => Verb::Back,
        A::Wait { .. } => Verb::Wait,
        A::Screenshot { .. } => Verb::Screenshot,
        A::App { op } => match op {
            crate::ast::AppOp::Launch => Verb::AppLaunch,
            crate::ast::AppOp::Terminate => Verb::AppTerminate,
            crate::ast::AppOp::Restart => Verb::AppRestart,
            crate::ast::AppOp::Background => Verb::AppBackground,
        },
    }
}

/// The support-matrix verb of an AST check condition.
#[must_use]
pub fn verb_of_check(condition: &crate::ast::CheckCondition) -> CheckVerb {
    use crate::ast::CheckCondition as C;
    match condition {
        C::IsVisible => CheckVerb::Visible,
        C::IsHidden => CheckVerb::Hidden,
        C::IsEnabled => CheckVerb::Enabled,
        C::IsDisabled => CheckVerb::Disabled,
        C::IsFocused => CheckVerb::Focused,
        C::TextEquals { .. } => CheckVerb::TextEquals,
        C::TextContains { .. } => CheckVerb::TextContains,
        C::Count { .. } => CheckVerb::Count,
        C::Showing(_) => CheckVerb::Showing,
        C::Player(_) => CheckVerb::Player,
    }
}

/// Check support per the second table of 6.1.
#[must_use]
pub fn check_support(platform: Platform, check: CheckVerb) -> Support {
    match (platform, check) {
        (Platform::Ios, CheckVerb::Focused) => Support::Unsupported,
        _ => Support::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_spot_checks() {
        // swipe is ✖ on TV platforms
        for tv in [
            Platform::WebOs,
            Platform::Tizen,
            Platform::AndroidTv,
            Platform::Tvos,
        ] {
            assert_eq!(action_support(tv, Verb::Swipe), Support::Unsupported);
        }
        assert!(action_support(Platform::Android, Verb::Swipe).is_supported());

        // focus is ✖ on touch Android/iOS
        assert_eq!(
            action_support(Platform::Android, Verb::Focus),
            Support::Unsupported
        );
        assert_eq!(
            action_support(Platform::Ios, Verb::Focus),
            Support::Unsupported
        );
        assert_eq!(
            action_support(Platform::AndroidTv, Verb::Focus),
            Support::Limited(DPAD_ONLY)
        );

        // app terminate unsupported on web runtimes
        for web in [Platform::Web, Platform::WebOs, Platform::Tizen] {
            assert_eq!(
                action_support(web, Verb::AppTerminate),
                Support::Unsupported
            );
            assert_eq!(
                action_support(web, Verb::AppRestart),
                Support::Limited(RELOAD_NAV)
            );
            assert_eq!(
                action_support(web, Verb::AppBackground),
                Support::Unsupported
            );
        }
        assert!(action_support(Platform::Ios, Verb::AppTerminate).is_supported());
        assert!(action_support(Platform::AndroidTv, Verb::AppBackground).is_supported());

        // select unsupported on tvos
        assert_eq!(
            action_support(Platform::Tvos, Verb::Select),
            Support::Unsupported
        );
        assert!(action_support(Platform::Web, Verb::Select).is_supported());

        // focused check unsupported on ios only
        assert_eq!(
            check_support(Platform::Ios, CheckVerb::Focused),
            Support::Unsupported
        );
        assert!(check_support(Platform::Tvos, CheckVerb::Focused).is_supported());
        assert!(check_support(Platform::Android, CheckVerb::Visible).is_supported());

        // key support delegates to the key table
        assert_eq!(
            key_support(Platform::Tizen, RemoteKey::Rewind),
            Support::Full
        );
        assert_eq!(key_support(Platform::Web, RemoteKey::Rewind), Support::Full);
        assert_eq!(
            key_support(Platform::Web, RemoteKey::Back),
            Support::Unsupported
        );
        assert_eq!(
            key_support(Platform::Android, RemoteKey::Ok),
            Support::Unsupported
        );
    }

    #[test]
    fn every_platform_has_full_screenshot() {
        for platform in Platform::ALL {
            assert_eq!(action_support(platform, Verb::Screenshot), Support::Full);
            assert_eq!(action_support(platform, Verb::Wait), Support::Full);
        }
    }
}
