//! Embedded inspector assets. Everything is compiled into the binary; the UI
//! never loads a CDN, font or script from the network.

pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_CSS: &str = include_str!("assets/app.css");
pub const APP_JS: &str = include_str!("assets/app.js");
pub const FAVICON_SVG: &str = include_str!("assets/favicon.svg");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpad_covers_navigation_and_remote_actions() {
        assert!(INDEX_HTML.contains("class=\"dpad\""));
        for key in ["up", "down", "left", "right", "ok", "back", "home"] {
            assert!(
                INDEX_HTML.contains(&format!("data-key=\"{key}\"")),
                "the D-pad is missing the {key} key"
            );
        }
    }

    #[test]
    fn dpad_and_keyboard_bindings_send_remote_keys() {
        assert!(APP_JS.contains(".dpad button[data-key]"));
        assert!(APP_JS.contains("sendKey(button.dataset.key)"));
        assert!(APP_JS.contains("sendKey('back')"));
        assert!(APP_JS.contains("sendKey('ok')"));
        assert!(APP_JS.contains("sendKey(event.key.slice(5).toLowerCase())"));
        assert!(APP_JS.contains("FALLBACK_KEYS"));
    }

    #[test]
    fn flutter_banner_starts_hidden() {
        assert!(INDEX_HTML.contains("id=\"flutter-banner\""));
        assert!(INDEX_HTML.contains("class=\"banner warn\" role=\"status\" hidden"));
    }

    #[test]
    fn flutter_banner_is_driven_by_snapshot_degradations() {
        assert!(APP_JS.contains("state.snapshot.degraded"));
        assert!(APP_JS.contains("'FLUTTER_SEMANTICS_OFF'"));
        assert!(APP_JS.contains("byId('flutter-banner')"));
        assert!(APP_JS.contains("Impresario.ensureSemantics()"));
    }
}
