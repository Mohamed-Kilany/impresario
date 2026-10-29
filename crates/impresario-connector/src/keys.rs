//! The web key table (spec 6.2) and the two dispatch modes of the
//! `key_dispatch` device option: CDP `Input.dispatchKeyEvent` and the JS
//! `KeyboardEvent` fallback for TVs that ignore synthetic input events.
//!
//! The codes come from `impresario_core::keys` (the single source of truth,
//! spec 6.2) so the connector table cannot drift from `docs/PARITY.md`.

use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;

use crate::errors::ConnectorError;
use crate::script;

/// One resolved key press: CDP fields plus the optional inserted text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchKey {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    pub windows_vk: u32,
    pub native_vk: u32,
    /// Text a `keyDown` inserts (Enter/`ok`); `None` uses `rawKeyDown`.
    pub text: Option<String>,
}

/// Resolve `key` for a web-platform device; `None` means the key is not part
/// of the platform's column of table 6.2.
#[must_use]
pub fn web_key(platform: Platform, key: RemoteKey) -> Option<DispatchKey> {
    let web = key.web(platform)?;
    let text = match key {
        RemoteKey::Ok | RemoteKey::Enter => Some("\r".to_string()),
        _ => None,
    };
    Some(DispatchKey {
        key: web.key.to_string(),
        code: web.code.to_string(),
        key_code: web.key_code,
        windows_vk: web.windows_vk,
        native_vk: web.native_vk,
        text,
    })
}

/// Resolve and surface the unsupported case as a wire error (defence in depth:
/// the planner already rejects these with `IMP-DSL-013`).
pub fn require_web_key(platform: Platform, key: RemoteKey) -> Result<DispatchKey, ConnectorError> {
    web_key(platform, key).ok_or_else(|| {
        ConnectorError::unsupported_action(format!(
            "key '{}' is not supported on {platform} (spec 6.2)",
            key.as_str()
        ))
        .with_detail("key", key.as_str())
        .with_detail("platform", platform.as_str())
    })
}

/// A JS expression that dispatches synthetic `keyDown`/`keyUp` events for the
/// key on `document.activeElement` (or `document.body`) with overridden
/// `keyCode`/`which` (spec 10.3). ES5 only.
#[must_use]
pub fn js_dispatch_expression(key: &DispatchKey) -> String {
    let body = format!(
        r#"
var override = function (event, name, value) {{
  try {{ Object.defineProperty(event, name, {{ get: function () {{ return value; }} }}); }}
  catch (e) {{ try {{ event[name] = value; }} catch (e2) {{ }} }}
}};
var target = document.activeElement || document.body;
var dispatch = function (type) {{
  var event = document.createEvent('Event');
  event.initEvent(type, true, true);
  override(event, 'key', {key});
  override(event, 'code', {code});
  override(event, 'keyCode', {key_code});
  override(event, 'which', {key_code});
  override(event, 'charCode', 0);
  target.dispatchEvent(event);
}};
dispatch('keydown');
dispatch('keyup');
return {{ ok: true }};
"#,
        key = script::js_string(&key.key),
        code = script::js_string(&key.code),
        key_code = key.key_code,
    );
    script::guarded(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_keys_follow_table_62() {
        let ok = web_key(Platform::WebOs, RemoteKey::Ok).expect("ok");
        assert_eq!(ok.key_code, 13);
        assert_eq!(ok.key, "Enter");
        assert_eq!(ok.text.as_deref(), Some("\r"));

        let back = web_key(Platform::WebOs, RemoteKey::Back).expect("back");
        assert_eq!(back.key_code, 461);
        let back = web_key(Platform::Tizen, RemoteKey::Back).expect("back");
        assert_eq!(back.key_code, 10009);

        assert!(web_key(Platform::Web, RemoteKey::Back).is_none());
        assert!(web_key(Platform::Web, RemoteKey::Play).is_some());
        assert!(web_key(Platform::WebOs, RemoteKey::Play).is_some());
        assert!(web_key(Platform::Web, RemoteKey::Up).is_some());
        assert!(web_key(Platform::Web, RemoteKey::Backspace).is_some());
        assert_eq!(
            web_key(Platform::Web, RemoteKey::Menu)
                .expect("menu")
                .key_code,
            93
        );
        assert_eq!(
            web_key(Platform::Web, RemoteKey::Num5)
                .expect("num5")
                .key_code,
            53
        );
        assert_eq!(
            web_key(Platform::Web, RemoteKey::Settings)
                .expect("settings")
                .key_code,
            176
        );
    }

    #[test]
    fn unsupported_keys_become_unsupported_action() {
        let error = require_web_key(Platform::Web, RemoteKey::Back).expect_err("back");
        assert_eq!(
            error.code,
            impresario_core::command::WireErrorCode::UnsupportedAction
        );
        assert_eq!(error.details.get("key").map(String::as_str), Some("back"));
    }

    #[test]
    fn js_dispatch_expression_carries_overrides() {
        let key = web_key(Platform::Tizen, RemoteKey::Ok).expect("ok");
        let expression = js_dispatch_expression(&key);
        assert!(expression.contains("keyCode"));
        assert!(expression.contains("13"));
        assert!(expression.contains("keydown"));
        assert!(expression.contains("keyup"));
        assert!(!expression.contains("=>"), "ES5 required");
    }
}
