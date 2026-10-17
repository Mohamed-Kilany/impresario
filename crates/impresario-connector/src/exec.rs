//! Platform semantics (spec 5.6, 6.1-6.4) executed against a
//! [`CdpBackend`]. Every wait is connector-side: one `Runtime.evaluate` per
//! poll, at `poll_interval_ms`, until the condition holds or `timeout_ms`
//! elapses. Elements are re-found on every poll (no stale handles).

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use impresario_core::ast::Direction;
use impresario_core::command::WireErrorCode;
use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::selectors::Strategy;
use impresario_proto::v1 as pb;

use crate::backends::{CdpBackend, KeyEvent, KeyEventType, MouseEvent, MouseEventType};
use crate::errors::ConnectorError;
use crate::focus::{self, FocusProbe, FocusState, Frame};
use crate::keys;
use crate::script;

/// `key_dispatch` device option (spec 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDispatchMode {
    Cdp,
    Js,
}

/// Executor knobs: device options plus the master's polling/loop bounds
/// (spec 7.1 handshake `options`, decision I-34).
#[derive(Debug, Clone)]
pub struct ExecOptions {
    pub poll_interval: Duration,
    pub scroll_max_attempts: u32,
    pub focus_max_steps: u32,
    pub key_dispatch: KeyDispatchMode,
    pub app_url: Option<String>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(100),
            scroll_max_attempts: 10,
            focus_max_steps: 50,
            key_dispatch: KeyDispatchMode::Cdp,
            app_url: None,
        }
    }
}

/// What a successful command produced (proto `CommandResult` fields).
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub observed_text: String,
    pub observed_count: u32,
    pub screenshot_png: Vec<u8>,
}

/// Text-entry verbs (spec 5.6: the three verbs are distinct and exact).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnterMode {
    Replace,
    Append,
    Clear,
}

impl EnterMode {
    fn as_script_mode(self) -> &'static str {
        match self {
            EnterMode::Replace => "replace",
            EnterMode::Append => "append",
            EnterMode::Clear => "clear",
        }
    }
}

/// A parsed selector.
#[derive(Debug, Clone)]
struct Sel {
    strategy: Strategy,
    value: String,
    index: u32,
}

impl Sel {
    fn parse(selector: &pb::Selector) -> Result<Self, ConnectorError> {
        let strategy = match strategy_from_proto(selector.strategy) {
            Some(strategy) => strategy,
            None => {
                return Err(ConnectorError::invalid_selector(format!(
                    "unknown selector strategy value {}",
                    selector.strategy
                )))
            }
        };
        if strategy_name(strategy).is_none() {
            return Err(ConnectorError::invalid_selector(format!(
                "strategy '{}' does not apply to web platforms (spec 5.3)",
                strategy.prefix()
            )));
        }
        Ok(Self {
            strategy,
            value: selector.value.clone(),
            index: selector.index,
        })
    }

    /// `window.__impresario.findSafe(...)` for this selector.
    fn find_call(&self) -> String {
        let name = strategy_name(self.strategy).unwrap_or("css");
        format!(
            "window.__impresario.findSafe({}, {}, {})",
            script::js_string(name),
            script::js_string(&self.value),
            self.index
        )
    }

    fn count_call(&self) -> String {
        let name = strategy_name(self.strategy).unwrap_or("css");
        format!(
            "window.__impresario.countSafe({}, {})",
            script::js_string(name),
            script::js_string(&self.value)
        )
    }

    fn describe(&self) -> String {
        format!("{}:{}", self.strategy.prefix(), self.value)
    }
}

fn strategy_from_proto(value: i32) -> Option<Strategy> {
    match pb::SelectorStrategy::try_from(value) {
        Ok(pb::SelectorStrategy::Css) => Some(Strategy::Css),
        Ok(pb::SelectorStrategy::Xpath) => Some(Strategy::XPath),
        Ok(pb::SelectorStrategy::Text) => Some(Strategy::Text),
        Ok(pb::SelectorStrategy::TextContains) => Some(Strategy::TextContains),
        Ok(pb::SelectorStrategy::AndroidId) => Some(Strategy::AndroidId),
        Ok(pb::SelectorStrategy::AndroidDesc) => Some(Strategy::AndroidDesc),
        Ok(pb::SelectorStrategy::AndroidClass) => Some(Strategy::AndroidClass),
        Ok(pb::SelectorStrategy::AccessibilityId) => Some(Strategy::AccessibilityId),
        Ok(pb::SelectorStrategy::Label) => Some(Strategy::Label),
        Ok(pb::SelectorStrategy::LabelContains) => Some(Strategy::LabelContains),
        Ok(pb::SelectorStrategy::Predicate) => Some(Strategy::Predicate),
        Ok(pb::SelectorStrategy::IosType) => Some(Strategy::IosType),
        Ok(pb::SelectorStrategy::Unspecified) | Err(_) => None,
    }
}

fn strategy_name(strategy: Strategy) -> Option<&'static str> {
    match strategy {
        Strategy::Css => Some("css"),
        Strategy::XPath => Some("xpath"),
        Strategy::Text => Some("text"),
        Strategy::TextContains => Some("text_contains"),
        _ => None,
    }
}

fn direction_from_proto(value: i32) -> Result<Direction, ConnectorError> {
    match pb::Direction::try_from(value) {
        Ok(pb::Direction::Up) => Ok(Direction::Up),
        Ok(pb::Direction::Down) => Ok(Direction::Down),
        Ok(pb::Direction::Left) => Ok(Direction::Left),
        Ok(pb::Direction::Right) => Ok(Direction::Right),
        Ok(pb::Direction::Unspecified) | Err(_) => Err(ConnectorError::protocol(format!(
            "action requires a direction (got {value})"
        ))),
    }
}

// ---------------------------------------------------------------------------
// probe shapes (helper-script replies)
// ---------------------------------------------------------------------------

/// Fields every helper reply carries.
trait ProbeEnvelope {
    fn ok(&self) -> bool;
    fn invalid_selector(&self) -> bool;
    fn error(&self) -> &str;
}

macro_rules! probe_envelope {
    ($($name:ident),* $(,)?) => {
        $(impl ProbeEnvelope for $name {
            fn ok(&self) -> bool { self.ok }
            fn invalid_selector(&self) -> bool { self.invalid_selector }
            fn error(&self) -> &str { &self.error }
        })*
    };
}

/// Map a failed helper reply onto a connector error (spec 7.1 codes).
fn probe_error<T: ProbeEnvelope>(probe: &T) -> ConnectorError {
    let message = probe.error();
    let code = if probe.invalid_selector() {
        WireErrorCode::InvalidSelector
    } else {
        WireErrorCode::Internal
    };
    ConnectorError::new(
        code,
        if message.is_empty() {
            "the helper script call failed".to_string()
        } else {
            message.to_string()
        },
    )
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NavProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ReadyProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    complete: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct InfoProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    ua: String,
    #[serde(default)]
    w: f64,
    #[serde(default)]
    h: f64,
    #[serde(default)]
    dpr: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ClickProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    visible: bool,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    hit: bool,
    #[serde(default)]
    obscured_by: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FocusAttemptProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    visible: bool,
    #[serde(default)]
    focused: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct TextProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    visible: bool,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    editable: String,
    #[serde(default)]
    value: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SetProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    applied: bool,
    #[serde(default)]
    value: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct OptionProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    value: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CheckProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    visible: bool,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    count: i64,
}

/// Predicate deciding one check kind from a `CheckProbe` reply.
type CheckPredicate = Box<dyn Fn(&CheckProbe) -> bool + Send + Sync>;

#[derive(Debug, Clone, Deserialize, Default)]
struct TargetProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    frame: Option<Frame>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FocusedProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    focused: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ActiveProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    present: bool,
    #[serde(default)]
    label: String,
    #[serde(default)]
    frame: Option<Frame>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FindProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    invalid_selector: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    found: bool,
}

probe_envelope!(
    NavProbe,
    ReadyProbe,
    InfoProbe,
    ClickProbe,
    FocusAttemptProbe,
    TextProbe,
    SetProbe,
    OptionProbe,
    CheckProbe,
    TargetProbe,
    FocusedProbe,
    ActiveProbe,
    FindProbe,
);

/// The outcome of one polling wait: satisfied, or elapsed with the last
/// observation (the caller maps that onto ELEMENT_NOT_FOUND /
/// NOT_INTERACTABLE / CHECK_FAILED).
enum Polled<T> {
    Satisfied(T),
    Elapsed(Box<T>),
}

/// One session's command executor.
pub struct Executor {
    platform: Platform,
    backend: Arc<dyn CdpBackend>,
    options: ExecOptions,
}

impl Executor {
    #[must_use]
    pub fn new(platform: Platform, backend: Arc<dyn CdpBackend>, options: ExecOptions) -> Self {
        Self {
            platform,
            backend,
            options,
        }
    }

    /// The CDP backend (session teardown, out-of-band screenshots).
    #[must_use]
    pub fn backend(&self) -> &Arc<dyn CdpBackend> {
        &self.backend
    }

    /// Inject the helper script idempotently (spec 10.3: before every command).
    pub async fn inject_helper(&self, cancel: &CancellationToken) -> Result<(), ConnectorError> {
        let expression = script::inject_expression();
        self.evaluate_value(&expression, cancel).await.map(|_| ())
    }

    /// `HandshakeResponse.device` (spec 7.1): user agent, viewport, DPR.
    pub async fn device_info(
        &self,
        cancel: &CancellationToken,
    ) -> Result<pb::DeviceInfo, ConnectorError> {
        let body = "var w = window.innerWidth || 0; var h = window.innerHeight || 0; \
                    var d = window.devicePixelRatio || 1; \
                    return {ok:true, ua:String(navigator.userAgent||''), w:w, h:h, dpr:d};";
        let probe: InfoProbe = self.probe(body, cancel).await?;
        let (os_name, os_version, runtime) = parse_user_agent(&probe.ua);
        Ok(pb::DeviceInfo {
            model: "Chromium".to_string(),
            os_name,
            os_version,
            screen_width_px: probe.w.max(0.0) as u32,
            screen_height_px: probe.h.max(0.0) as u32,
            pixel_ratio: probe.dpr as f32,
            runtime,
        })
    }

    // -----------------------------------------------------------------------
    // actions
    // -----------------------------------------------------------------------

    /// Execute one `RunAction` (spec 5.6).
    pub async fn run_action(
        &self,
        action: &pb::RunAction,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        let kind = pb::ActionType::try_from(action.r#type).map_err(|_| {
            ConnectorError::protocol(format!("unknown action type {}", action.r#type))
        })?;
        match kind {
            pb::ActionType::Click => {
                let sel = require_selector(action, "click")?;
                self.click(&sel, timeout_ms, cancel).await
            }
            pb::ActionType::Type => {
                let sel = require_selector(action, "type")?;
                self.enter_text(&sel, EnterMode::Replace, &action.text, timeout_ms, cancel)
                    .await
            }
            pb::ActionType::Append => {
                let sel = require_selector(action, "append")?;
                self.enter_text(&sel, EnterMode::Append, &action.text, timeout_ms, cancel)
                    .await
            }
            pb::ActionType::Clear => {
                let sel = require_selector(action, "clear")?;
                self.enter_text(&sel, EnterMode::Clear, "", timeout_ms, cancel)
                    .await
            }
            pb::ActionType::LongPress => {
                let sel = require_selector(action, "longpress")?;
                let duration = if action.duration_ms == 0 {
                    1000
                } else {
                    action.duration_ms
                };
                self.long_press(&sel, duration, timeout_ms, cancel).await
            }
            pb::ActionType::Focus => {
                let sel = require_selector(action, "focus")?;
                self.focus_action(&sel, timeout_ms, cancel).await
            }
            pb::ActionType::Select => {
                let sel = require_selector(action, "select")?;
                self.select_option(&sel, &action.text, timeout_ms, cancel)
                    .await
            }
            pb::ActionType::Scroll => {
                let direction = direction_from_proto(action.direction)?;
                let count = action.count.max(1);
                let sel = match &action.selector {
                    Some(selector) => Some(Sel::parse(selector)?),
                    None => None,
                };
                self.scroll(direction, count, sel.as_ref(), timeout_ms, cancel)
                    .await
            }
            pb::ActionType::ScrollTo => {
                let sel = require_selector(action, "scrollto")?;
                self.scroll_to(&sel, cancel).await
            }
            pb::ActionType::Swipe => Err(ConnectorError::unsupported_action(
                "swipe is not supported on web platforms (spec 6.1)",
            )),
            pb::ActionType::Back => self.back(cancel).await,
            pb::ActionType::Unspecified => Err(ConnectorError::protocol(
                "action type is unspecified".to_string(),
            )),
        }
    }

    /// `click` (spec 5.6 + 6.3): web taps the center; web TVs focus and press
    /// OK.
    async fn click(
        &self,
        sel: &Sel,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        if self.platform == Platform::Web {
            let body = click_body(sel);
            match self
                .poll(timeout_ms, cancel, &body, |probe: &ClickProbe| {
                    probe.found && probe.visible && probe.enabled && probe.hit
                })
                .await?
            {
                Polled::Satisfied(probe) => {
                    self.dispatch_click(probe.x, probe.y).await?;
                    Ok(CommandOutput::default())
                }
                Polled::Elapsed(probe) => Err(click_failure(sel, &probe)),
            }
        } else {
            self.ensure_focus_tv(sel, timeout_ms, cancel).await?;
            self.press_remote(RemoteKey::Ok, 1, 0, cancel).await?;
            Ok(CommandOutput::default())
        }
    }

    async fn dispatch_click(&self, x: f64, y: f64) -> Result<(), ConnectorError> {
        let moved = MouseEvent {
            kind: MouseEventType::Moved,
            x,
            y,
            button: "none",
            buttons: 0,
            click_count: 0,
        };
        let pressed = MouseEvent {
            kind: MouseEventType::Pressed,
            x,
            y,
            button: "left",
            buttons: 1,
            click_count: 1,
        };
        let released = MouseEvent {
            kind: MouseEventType::Released,
            x,
            y,
            button: "left",
            buttons: 0,
            click_count: 1,
        };
        self.backend
            .dispatch_mouse(&moved)
            .await
            .map_err(ConnectorError::from)?;
        self.backend
            .dispatch_mouse(&pressed)
            .await
            .map_err(ConnectorError::from)?;
        self.backend
            .dispatch_mouse(&released)
            .await
            .map_err(ConnectorError::from)?;
        Ok(())
    }

    /// `type` / `append` / `clear` (spec 5.6, 10.3 "Text").
    async fn enter_text(
        &self,
        sel: &Sel,
        mode: EnterMode,
        text: &str,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        let body = text_body(sel);
        let probe = match self
            .poll(timeout_ms, cancel, &body, |probe: &TextProbe| {
                probe.found && probe.visible && probe.enabled && !probe.editable.is_empty()
            })
            .await?
        {
            Polled::Satisfied(probe) => probe,
            Polled::Elapsed(probe) => return Err(editable_failure(sel, mode, &probe)),
        };

        let expected = match mode {
            EnterMode::Replace => text.to_string(),
            EnterMode::Append => format!("{}{}", probe.value, text),
            EnterMode::Clear => String::new(),
        };

        if probe.editable == "contenteditable" {
            return self
                .enter_contenteditable(sel, mode, text, &expected, cancel)
                .await;
        }

        let mutate = set_value_body(sel, mode, text);
        let applied: SetProbe = self.probe(&mutate, cancel).await?;
        if applied.applied && applied.value == expected {
            return Ok(CommandOutput {
                observed_text: applied.value,
                ..CommandOutput::default()
            });
        }

        // Fallback (spec 10.3): select-all + Input.insertText (or Backspace
        // for a clear), then re-verify; still wrong -> NOT_INTERACTABLE.
        self.evaluate_value(&script::guarded(&select_all_body(sel)), cancel)
            .await?;
        if expected.is_empty() {
            self.press_remote(RemoteKey::Backspace, 1, 0, cancel)
                .await?;
        } else {
            self.backend
                .insert_text(text)
                .await
                .map_err(ConnectorError::from)?;
        }
        let verify: TextProbe = self.probe(&text_body(sel), cancel).await?;
        if verify.value == expected {
            return Ok(CommandOutput {
                observed_text: verify.value,
                ..CommandOutput::default()
            });
        }
        Err(
            ConnectorError::not_interactable("the field content did not become the requested text")
                .with_detail("expected", expected)
                .with_detail("observed", verify.value)
                .with_detail("selector", sel.describe()),
        )
    }

    async fn enter_contenteditable(
        &self,
        sel: &Sel,
        mode: EnterMode,
        text: &str,
        expected: &str,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        let prepare = format!(
            r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, applied:false}}; }}
window.__impresario.focus(el);
if ({clear}) {{
  window.__impresario.clearContent(el);
}} else if ({append}) {{
  window.__impresario.collapseToEnd(el);
}} else {{
  window.__impresario.selectAll(el);
}}
return {{ok:true, applied:true}};
"#,
            find = sel.find_call(),
            clear = mode == EnterMode::Clear,
            append = mode == EnterMode::Append,
        );
        let prepared: SetProbe = self.probe(&prepare, cancel).await?;
        if !prepared.applied {
            return Err(ConnectorError::element_not_found(format!(
                "the element {}:{} disappeared before text entry",
                sel.strategy.prefix(),
                sel.value
            ))
            .with_detail("selector", sel.describe()));
        }
        if mode != EnterMode::Clear {
            self.backend
                .insert_text(text)
                .await
                .map_err(ConnectorError::from)?;
        }
        let verify: TextProbe = self.probe(&text_body(sel), cancel).await?;
        if verify.value == expected {
            return Ok(CommandOutput {
                observed_text: verify.value,
                ..CommandOutput::default()
            });
        }
        Err(ConnectorError::not_interactable(
            "the contenteditable content did not become the requested text",
        )
        .with_detail("expected", expected)
        .with_detail("observed", verify.value)
        .with_detail("selector", sel.describe()))
    }

    /// `focus` (spec 6.1, 6.3): programmatic on web; on web TVs programmatic
    /// first, then the D-pad traversal.
    async fn focus_action(
        &self,
        sel: &Sel,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        if self.platform == Platform::Web {
            let body = focus_body(sel);
            match self
                .poll(timeout_ms, cancel, &body, |probe: &FocusAttemptProbe| {
                    probe.found && probe.focused
                })
                .await?
            {
                Polled::Satisfied(_) => Ok(CommandOutput::default()),
                Polled::Elapsed(probe) => {
                    if !probe.found {
                        Err(element_not_found(sel, "focus"))
                    } else if !probe.visible {
                        Err(ConnectorError::not_interactable(format!(
                            "focus: the element {}:{} is not visible",
                            sel.strategy.prefix(),
                            sel.value
                        ))
                        .with_detail("selector", sel.describe())
                        .with_detail("reason", "not visible"))
                    } else {
                        Err(ConnectorError::focus_unreachable(format!(
                            "focus() did not take effect on {}:{}",
                            sel.strategy.prefix(),
                            sel.value
                        ))
                        .with_detail("reason", "programmatic")
                        .with_detail("selector", sel.describe()))
                    }
                }
            }
        } else {
            self.ensure_focus_tv(sel, timeout_ms, cancel).await?;
            Ok(CommandOutput::default())
        }
    }

    /// `select` (spec 5.6): set the `<select>` whose option text equals the
    /// option, then dispatch events.
    async fn select_option(
        &self,
        sel: &Sel,
        option: &str,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        let body = select_probe_body(sel);
        match self
            .poll(timeout_ms, cancel, &body, |probe: &TextProbe| {
                probe.found && probe.visible && probe.enabled && probe.editable == "select"
            })
            .await?
        {
            Polled::Satisfied(_) => {}
            Polled::Elapsed(probe) => {
                if !probe.found {
                    return Err(element_not_found(sel, "select"));
                }
                if probe.editable != "select" {
                    return Err(ConnectorError::not_interactable(format!(
                        "the element {}:{} is not a <select>",
                        sel.strategy.prefix(),
                        sel.value
                    ))
                    .with_detail("selector", sel.describe()));
                }
                return Err(
                    ConnectorError::not_interactable("the <select> is not interactable")
                        .with_detail("selector", sel.describe()),
                );
            }
        }
        let mutate = format!(
            r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
var picked = window.__impresario.selectOption(el, {option});
if (!picked.found) {{ return {{ok:true, found:false}}; }}
return {{ok:true, found:true, value:(picked.value == null ? '' : String(picked.value))}};
"#,
            find = sel.find_call(),
            option = script::js_string(option)
        );
        let picked: OptionProbe = self.probe(&mutate, cancel).await?;
        if !picked.found {
            return Err(ConnectorError::element_not_found(format!(
                "no option '{option}' in the <select> {}:{}",
                sel.strategy.prefix(),
                sel.value
            ))
            .with_detail("option", option)
            .with_detail("selector", sel.describe()));
        }
        Ok(CommandOutput {
            observed_text: picked.value,
            ..CommandOutput::default()
        })
    }

    /// `scroll` (spec 5.6): DOM scrolling on `web`, D-pad presses on web TVs.
    async fn scroll(
        &self,
        direction: Direction,
        count: u32,
        container: Option<&Sel>,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        if self.platform == Platform::Web {
            if let Some(sel) = container {
                let probe: FindProbe = self.probe(&find_body(sel), cancel).await?;
                if !probe.found {
                    return Err(element_not_found(sel, "scroll"));
                }
            }
            let mut steps = 0;
            while steps < count {
                cancel_slot(cancel)?;
                self.dom_scroll_step(direction, container, cancel).await?;
                steps += 1;
            }
            Ok(CommandOutput::default())
        } else {
            if let Some(sel) = container {
                self.ensure_focus_tv(sel, timeout_ms, cancel).await?;
            }
            let key = direction_key(direction);
            self.press_remote(key, count, 100, cancel).await?;
            Ok(CommandOutput::default())
        }
    }

    /// One DOM scroll step: 80% of the container's visible extent (spec 5.6).
    async fn dom_scroll_step(
        &self,
        direction: Direction,
        container: Option<&Sel>,
        cancel: &CancellationToken,
    ) -> Result<(), ConnectorError> {
        let vertical = matches!(direction, Direction::Up | Direction::Down);
        let target = match container {
            Some(sel) => format!(
                r#"var c = {find}; if (c.error) {{ return {{ok:false, invalid_selector:true, error:c.error}}; }} var target = c.el;"#,
                find = sel.find_call()
            ),
            None => "var target = window.__impresario.primaryScrollable();".to_string(),
        };
        let body = format!(
            r#"
{target}
var amount = 0.8 * window.__impresario.visibleExtent(target, {vertical});
window.__impresario.scrollBy(target, {direction}, amount);
return {{ok:true}};
"#,
            target = target,
            vertical = vertical,
            direction = script::js_string(direction_name(direction)),
        );
        self.probe::<NavProbe>(&body, cancel).await.map(|_| ())
    }

    /// `scrollto` (spec 5.6): down first, then up once the container reached
    /// its end, up to `scroll_max_attempts` steps.
    async fn scroll_to(
        &self,
        sel: &Sel,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        let max_attempts = self.options.scroll_max_attempts.max(1);
        let mut attempts: u32 = 0;
        let mut phase_down = true;
        let tv_half = max_attempts.div_ceil(2);
        loop {
            cancel_slot(cancel)?;
            let body = visible_body(sel);
            let probe: CheckProbe = self.probe(&body, cancel).await?;
            if probe.found && probe.visible {
                return Ok(CommandOutput::default());
            }
            if attempts >= max_attempts {
                return Err(ConnectorError::element_not_found(format!(
                    "{}:{} was not visible after {attempts} scroll steps",
                    sel.strategy.prefix(),
                    sel.value
                ))
                .with_detail("scroll_attempts", attempts.to_string())
                .with_detail("selector", sel.describe()));
            }
            if self.platform == Platform::Web {
                let direction = if phase_down {
                    Direction::Down
                } else {
                    Direction::Up
                };
                self.dom_scroll_step(direction, None, cancel).await?;
                if phase_down && self.scroll_at_end(cancel).await? {
                    phase_down = false;
                }
            } else {
                if phase_down && attempts >= tv_half {
                    phase_down = false;
                }
                let direction = if phase_down {
                    Direction::Down
                } else {
                    Direction::Up
                };
                self.press_remote(direction_key(direction), 1, 100, cancel)
                    .await?;
            }
            attempts += 1;
        }
    }

    async fn scroll_at_end(&self, cancel: &CancellationToken) -> Result<bool, ConnectorError> {
        let body = r#"
var target = window.__impresario.primaryScrollable();
var info = window.__impresario.scrollInfo(target);
return {ok:true, complete:(info.max - info.top) <= 1};
"#;
        let probe: ReadyProbe = self.probe(body, cancel).await?;
        Ok(probe.complete)
    }

    /// `longpress` (spec 6.1): mouse hold on web, OK held on web TVs.
    async fn long_press(
        &self,
        sel: &Sel,
        duration_ms: u32,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        if self.platform == Platform::Web {
            let body = click_body(sel);
            let probe = match self
                .poll(timeout_ms, cancel, &body, |probe: &ClickProbe| {
                    probe.found && probe.visible && probe.enabled && probe.hit
                })
                .await?
            {
                Polled::Satisfied(probe) => probe,
                Polled::Elapsed(probe) => return Err(click_failure(sel, &probe)),
            };
            let pressed = MouseEvent {
                kind: MouseEventType::Pressed,
                x: probe.x,
                y: probe.y,
                button: "left",
                buttons: 1,
                click_count: 1,
            };
            let released = MouseEvent {
                kind: MouseEventType::Released,
                x: probe.x,
                y: probe.y,
                button: "left",
                buttons: 0,
                click_count: 1,
            };
            self.backend
                .dispatch_mouse(&pressed)
                .await
                .map_err(ConnectorError::from)?;
            let slept = tokio::select! {
                biased;
                _ = cancel.cancelled() => false,
                _ = tokio::time::sleep(Duration::from_millis(u64::from(duration_ms))) => true,
            };
            let _ = self.backend.dispatch_mouse(&released).await;
            if !slept {
                return Err(ConnectorError::cancelled());
            }
            Ok(CommandOutput::default())
        } else {
            self.ensure_focus_tv(sel, timeout_ms, cancel).await?;
            let key = keys::require_web_key(self.platform, RemoteKey::Ok)?;
            let down = key_event(&key, KeyEventType::KeyDown);
            let up = key_event(&key, KeyEventType::KeyUp);
            if self.options.key_dispatch == KeyDispatchMode::Js {
                // The JS fallback cannot hold a key; it dispatches one
                // down/up pair (documented in docs/DEVICES.md).
                self.probe::<NavProbe>(&keys::js_dispatch_expression(&key), cancel)
                    .await?;
                self.pause(Duration::from_millis(u64::from(duration_ms)), cancel)
                    .await?;
                return Ok(CommandOutput::default());
            }
            self.backend
                .dispatch_key(&down)
                .await
                .map_err(ConnectorError::from)?;
            let slept = tokio::select! {
                biased;
                _ = cancel.cancelled() => false,
                _ = tokio::time::sleep(Duration::from_millis(u64::from(duration_ms))) => true,
            };
            let _ = self.backend.dispatch_key(&up).await;
            if !slept {
                return Err(ConnectorError::cancelled());
            }
            Ok(CommandOutput::default())
        }
    }

    /// `back` (spec 6.1): browser history on web; the back key on web TVs.
    async fn back(&self, cancel: &CancellationToken) -> Result<CommandOutput, ConnectorError> {
        if self.platform == Platform::Web {
            self.probe::<NavProbe>("history.back(); return {ok:true};", cancel)
                .await?;
        } else {
            self.press_remote(RemoteKey::Back, 1, 0, cancel).await?;
        }
        Ok(CommandOutput::default())
    }

    /// Focus a web-TV element (spec 6.3): programmatic first, D-pad traversal
    /// as the fallback.
    async fn ensure_focus_tv(
        &self,
        sel: &Sel,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<(), ConnectorError> {
        let body = focus_body(sel);
        let probe = match self
            .poll(timeout_ms, cancel, &body, |probe: &FocusAttemptProbe| {
                probe.found && probe.visible
            })
            .await?
        {
            Polled::Satisfied(probe) => probe,
            Polled::Elapsed(probe) => {
                if !probe.found {
                    return Err(element_not_found(sel, "focus"));
                }
                return Err(ConnectorError::not_interactable(format!(
                    "focus: the element {}:{} is not visible",
                    sel.strategy.prefix(),
                    sel.value
                ))
                .with_detail("selector", sel.describe())
                .with_detail("reason", "not visible"));
            }
        };
        if probe.focused {
            return Ok(());
        }
        // A couple of programmatic retries before the D-pad fallback.
        for _ in 0..2 {
            self.pause(self.options.poll_interval, cancel).await?;
            let retry: FocusAttemptProbe = self.probe(&body, cancel).await?;
            if retry.focused {
                return Ok(());
            }
        }
        let adapter = ExecFocusProbe {
            executor: self,
            sel,
            cancel,
        };
        focus::navigate(&adapter, self.options.focus_max_steps).await
    }

    // -----------------------------------------------------------------------
    // checks
    // -----------------------------------------------------------------------

    /// One `ValidateCheck` (spec 5.6 "Checks", 6.4).
    pub async fn validate_check(
        &self,
        check: &pb::ValidateCheck,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        let kind = pb::CheckKind::try_from(check.kind)
            .map_err(|_| ConnectorError::protocol(format!("unknown check kind {}", check.kind)))?;
        let selector = check.selector.as_ref().ok_or_else(|| {
            ConnectorError::protocol("a check always carries a selector".to_string())
        })?;
        let sel = Sel::parse(selector)?;

        let (body, satisfied): (String, CheckPredicate) = match kind {
            pb::CheckKind::Visible => (
                visible_body(&sel),
                Box::new(|probe: &CheckProbe| probe.found && probe.visible),
            ),
            pb::CheckKind::Hidden => (
                visible_body(&sel),
                Box::new(|probe: &CheckProbe| !probe.found || !probe.visible),
            ),
            pb::CheckKind::Enabled => (
                state_body(&sel, "enabled"),
                Box::new(|probe: &CheckProbe| probe.found && probe.enabled),
            ),
            pb::CheckKind::Disabled => (
                state_body(&sel, "enabled"),
                Box::new(|probe: &CheckProbe| probe.found && !probe.enabled),
            ),
            pb::CheckKind::Focused => (
                state_body(&sel, "focused"),
                Box::new(|probe: &CheckProbe| probe.found && probe.focused),
            ),
            pb::CheckKind::TextEquals => {
                let expected = check.expected_text.clone();
                (
                    state_body(&sel, "text"),
                    Box::new(move |probe: &CheckProbe| probe.found && probe.text == expected),
                )
            }
            pb::CheckKind::TextContainsCheck => {
                let expected = check.expected_text.clone();
                (
                    state_body(&sel, "text"),
                    Box::new(move |probe: &CheckProbe| {
                        probe.found && probe.text.contains(&expected)
                    }),
                )
            }
            pb::CheckKind::Count => {
                let expected = i64::from(check.expected_count);
                (
                    count_body(&sel),
                    Box::new(move |probe: &CheckProbe| probe.count == expected),
                )
            }
            pb::CheckKind::Unspecified => {
                return Err(ConnectorError::protocol(
                    "check kind is unspecified".to_string(),
                ))
            }
        };

        let probe = self.poll(timeout_ms, cancel, &body, satisfied).await?;
        match probe {
            Polled::Satisfied(probe) => Ok(check_output(kind, &probe)),
            Polled::Elapsed(probe) => Err(check_failure(kind, check, &sel, &probe)),
        }
    }

    // -----------------------------------------------------------------------
    // keys, screenshots, app control
    // -----------------------------------------------------------------------

    /// One `KeyPress` command (spec 6.2).
    pub async fn key_press(
        &self,
        press: &pb::KeyPress,
        cancel: &CancellationToken,
    ) -> Result<(), ConnectorError> {
        let key = remote_key_from_proto(press.key)?;
        let repeat = press.repeat.max(1);
        let interval = if press.interval_ms == 0 {
            100
        } else {
            press.interval_ms
        };
        self.press_remote(key, repeat, interval, cancel).await
    }

    async fn press_remote(
        &self,
        key: RemoteKey,
        repeat: u32,
        interval_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<(), ConnectorError> {
        let dispatch = keys::require_web_key(self.platform, key)?;
        let mut pressed = 0;
        while pressed < repeat.max(1) {
            cancel_slot(cancel)?;
            if self.options.key_dispatch == KeyDispatchMode::Js {
                let expression = keys::js_dispatch_expression(&dispatch);
                self.evaluate_value(&expression, cancel).await?;
            } else {
                let down = key_event(&dispatch, KeyEventType::KeyDown);
                let up = key_event(&dispatch, KeyEventType::KeyUp);
                self.backend
                    .dispatch_key(&down)
                    .await
                    .map_err(ConnectorError::from)?;
                self.backend
                    .dispatch_key(&up)
                    .await
                    .map_err(ConnectorError::from)?;
            }
            pressed += 1;
            if pressed < repeat {
                self.pause(Duration::from_millis(u64::from(interval_ms)), cancel)
                    .await?;
            }
        }
        Ok(())
    }

    /// `screenshot` (the master names the file; the label is ignored here).
    pub async fn screenshot(
        &self,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        cancel_slot(cancel)?;
        let png = self
            .backend
            .screenshot()
            .await
            .map_err(ConnectorError::from)?;
        Ok(CommandOutput {
            screenshot_png: png,
            ..CommandOutput::default()
        })
    }

    /// `app launch|restart` (spec 10.3): navigate to `app_url`, else reload.
    /// `terminate` is `UNSUPPORTED_ACTION` (spec 6.1).
    pub async fn app_control(
        &self,
        op: pb::AppOp,
        cancel: &CancellationToken,
    ) -> Result<(), ConnectorError> {
        match op {
            pb::AppOp::Launch | pb::AppOp::Restart => {
                match self.options.app_url.clone() {
                    Some(url) => self
                        .backend
                        .navigate(&url)
                        .await
                        .map_err(ConnectorError::from)?,
                    None => self.backend.reload().await.map_err(ConnectorError::from)?,
                }
                self.wait_ready(cancel).await;
                Ok(())
            }
            pb::AppOp::Terminate => Err(ConnectorError::unsupported_action(
                "app terminate is not supported on web platforms (spec 6.1)",
            )),
            pb::AppOp::Unspecified => Err(ConnectorError::protocol(
                "app op is unspecified".to_string(),
            )),
        }
    }

    /// Best-effort `document.readyState == "complete"` wait (10 s cap; a page
    /// that never finishes loading does not fail the launch).
    async fn wait_ready(&self, cancel: &CancellationToken) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let body = "return {ok:true, complete:((document.readyState || '') === 'complete')};";
        loop {
            if cancel.is_cancelled() {
                return;
            }
            match self.probe::<ReadyProbe>(body, cancel).await {
                Ok(probe) if probe.complete => return,
                Ok(_) => {}
                Err(_) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            if self
                .pause(self.options.poll_interval, cancel)
                .await
                .is_err()
            {
                return;
            }
        }
    }

    // -----------------------------------------------------------------------
    // plumbing
    // -----------------------------------------------------------------------

    async fn probe<T: DeserializeOwned + ProbeEnvelope>(
        &self,
        body: &str,
        cancel: &CancellationToken,
    ) -> Result<T, ConnectorError> {
        let expression = script::guarded(body);
        let probe = self.probe_once::<T>(&expression, cancel).await?;
        if probe.ok() {
            return Ok(probe);
        }
        // A navigation can drop the helper between the per-command injection
        // and this evaluation (page discovery can name a page before its first
        // document commits): inject again and retry once.
        self.evaluate_value(&script::inject_expression(), cancel)
            .await?;
        let retry = self.probe_once::<T>(&expression, cancel).await?;
        if retry.ok() {
            return Ok(retry);
        }
        Err(probe_error(&retry))
    }

    async fn probe_once<T: DeserializeOwned + ProbeEnvelope>(
        &self,
        expression: &str,
        cancel: &CancellationToken,
    ) -> Result<T, ConnectorError> {
        let value = self.evaluate_value(expression, cancel).await?;
        serde_json::from_value(value)
            .map_err(|error| ConnectorError::internal(format!("malformed helper reply: {error}")))
    }

    async fn evaluate_value(
        &self,
        expression: &str,
        cancel: &CancellationToken,
    ) -> Result<Value, ConnectorError> {
        let evaluation = self.backend.evaluate(expression);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ConnectorError::cancelled()),
            result = evaluation => result.map_err(ConnectorError::from),
        }
    }

    /// Poll `body` until `satisfied` or the timeout elapses (spec 10.3
    /// "Polling": one evaluate per poll).
    async fn poll<T, F>(
        &self,
        timeout_ms: u32,
        cancel: &CancellationToken,
        body: &str,
        satisfied: F,
    ) -> Result<Polled<T>, ConnectorError>
    where
        T: DeserializeOwned + ProbeEnvelope,
        F: Fn(&T) -> bool,
    {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(u64::from(timeout_ms.max(1)));
        loop {
            let probe: T = self.probe(body, cancel).await?;
            if satisfied(&probe) {
                return Ok(Polled::Satisfied(probe));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(Polled::Elapsed(Box::new(probe)));
            }
            self.pause(self.options.poll_interval, cancel).await?;
        }
    }

    async fn pause(
        &self,
        duration: Duration,
        cancel: &CancellationToken,
    ) -> Result<(), ConnectorError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ConnectorError::cancelled()),
            _ = tokio::time::sleep(duration) => Ok(()),
        }
    }
}

fn cancel_slot(cancel: &CancellationToken) -> Result<(), ConnectorError> {
    if cancel.is_cancelled() {
        Err(ConnectorError::cancelled())
    } else {
        Ok(())
    }
}

fn key_event(key: &keys::DispatchKey, kind: KeyEventType) -> KeyEvent {
    KeyEvent {
        kind,
        key: key.key.clone(),
        code: key.code.clone(),
        key_code: key.key_code,
        windows_vk: key.windows_vk,
        native_vk: key.native_vk,
        text: match kind {
            KeyEventType::KeyDown => key.text.clone(),
            _ => None,
        },
    }
}

fn require_selector(action: &pb::RunAction, verb: &str) -> Result<Sel, ConnectorError> {
    let selector = action
        .selector
        .as_ref()
        .ok_or_else(|| ConnectorError::protocol(format!("{verb} requires a selector")))?;
    Sel::parse(selector)
}

fn remote_key_from_proto(value: i32) -> Result<RemoteKey, ConnectorError> {
    match pb::RemoteKey::try_from(value) {
        Ok(pb::RemoteKey::KeyOk) => Ok(RemoteKey::Ok),
        Ok(pb::RemoteKey::KeyEnter) => Ok(RemoteKey::Enter),
        Ok(pb::RemoteKey::KeyBack) => Ok(RemoteKey::Back),
        Ok(pb::RemoteKey::KeyUp) => Ok(RemoteKey::Up),
        Ok(pb::RemoteKey::KeyDown) => Ok(RemoteKey::Down),
        Ok(pb::RemoteKey::KeyLeft) => Ok(RemoteKey::Left),
        Ok(pb::RemoteKey::KeyRight) => Ok(RemoteKey::Right),
        Ok(pb::RemoteKey::KeyHome) => Ok(RemoteKey::Home),
        Ok(pb::RemoteKey::KeyMenu) => Ok(RemoteKey::Menu),
        Ok(pb::RemoteKey::KeyPlay) => Ok(RemoteKey::Play),
        Ok(pb::RemoteKey::KeyPause) => Ok(RemoteKey::Pause),
        Ok(pb::RemoteKey::KeyPlaypause) => Ok(RemoteKey::PlayPause),
        Ok(pb::RemoteKey::KeyRewind) => Ok(RemoteKey::Rewind),
        Ok(pb::RemoteKey::KeyFastforward) => Ok(RemoteKey::FastForward),
        Ok(pb::RemoteKey::KeyRed) => Ok(RemoteKey::Red),
        Ok(pb::RemoteKey::KeyGreen) => Ok(RemoteKey::Green),
        Ok(pb::RemoteKey::KeyYellow) => Ok(RemoteKey::Yellow),
        Ok(pb::RemoteKey::KeyBlue) => Ok(RemoteKey::Blue),
        Ok(pb::RemoteKey::KeyBackspace) => Ok(RemoteKey::Backspace),
        Ok(pb::RemoteKey::Unspecified) | Err(_) => Err(ConnectorError::protocol(format!(
            "unknown remote key value {value}"
        ))),
    }
}

fn direction_key(direction: Direction) -> RemoteKey {
    match direction {
        Direction::Up => RemoteKey::Up,
        Direction::Down => RemoteKey::Down,
        Direction::Left => RemoteKey::Left,
        Direction::Right => RemoteKey::Right,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Up => "up",
        Direction::Down => "down",
        Direction::Left => "left",
        Direction::Right => "right",
    }
}

// ---------------------------------------------------------------------------
// helper-script bodies
// ---------------------------------------------------------------------------

fn find_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
if (!r.el) {{ return {{ok:true, found:false}}; }}
return {{ok:true, found:true}};
"#,
        find = sel.find_call()
    )
}

fn click_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
window.__impresario.scrollIntoView(el);
var visible = window.__impresario.isVisible(el);
var enabled = window.__impresario.isEnabled(el);
if (!visible || !enabled) {{ return {{ok:true, found:true, visible:visible, enabled:enabled}}; }}
var rect = window.__impresario.rectOf(el);
if (!rect) {{ return {{ok:true, found:true, visible:visible, enabled:enabled}}; }}
var hit = window.__impresario.hitTest(el);
return {{ok:true, found:true, visible:visible, enabled:enabled, x:(rect.x + rect.w / 2), y:(rect.y + rect.h / 2), hit:hit.ok, obscured_by:hit.obscured_by}};
"#,
        find = sel.find_call()
    )
}

fn focus_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
window.__impresario.scrollIntoView(el);
var visible = window.__impresario.isVisible(el);
if (!visible) {{ return {{ok:true, found:true, visible:false}}; }}
var focused = window.__impresario.focus(el);
return {{ok:true, found:true, visible:true, focused:focused}};
"#,
        find = sel.find_call()
    )
}

fn text_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
window.__impresario.scrollIntoView(el);
return {{
  ok:true,
  found:true,
  visible:window.__impresario.isVisible(el),
  enabled:window.__impresario.isEnabled(el),
  editable:window.__impresario.editableKind(el),
  value:window.__impresario.textOf(el)
}};
"#,
        find = sel.find_call()
    )
}

fn select_probe_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
window.__impresario.scrollIntoView(el);
var tag = String(el.tagName || '').toLowerCase();
return {{ok:true, found:true, visible:window.__impresario.isVisible(el), enabled:window.__impresario.isEnabled(el), editable:(tag === 'select' ? 'select' : tag)}};
"#,
        find = sel.find_call()
    )
}

fn set_value_body(sel: &Sel, mode: EnterMode, text: &str) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, applied:false}}; }}
window.__impresario.focus(el);
var value = window.__impresario.setValue(el, {text}, {mode});
return {{ok:true, applied:true, value:(value == null ? '' : String(value))}};
"#,
        find = sel.find_call(),
        text = script::js_string(text),
        mode = script::js_string(mode.as_script_mode()),
    )
}

fn select_all_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true}}; }}
window.__impresario.focus(el);
window.__impresario.selectAll(el);
return {{ok:true}};
"#,
        find = sel.find_call()
    )
}

fn visible_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
return {{ok:true, found:true, visible:window.__impresario.isVisible(el)}};
"#,
        find = sel.find_call()
    )
}

fn state_body(sel: &Sel, field: &str) -> String {
    let field_js = match field {
        "enabled" => "enabled:window.__impresario.isEnabled(el)",
        "focused" => "focused:window.__impresario.isFocused(el)",
        _ => "text:window.__impresario.textOf(el)",
    };
    format!(
        r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el) {{ return {{ok:true, found:false}}; }}
return {{ok:true, found:true, {field}}};
"#,
        find = sel.find_call(),
        field = field_js
    )
}

fn count_body(sel: &Sel) -> String {
    format!(
        r#"
var r = {count};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
return {{ok:true, count:r.count}};
"#,
        count = sel.count_call()
    )
}

// ---------------------------------------------------------------------------
// failure construction (spec 5.6 details)
// ---------------------------------------------------------------------------

fn element_not_found(sel: &Sel, verb: &str) -> ConnectorError {
    ConnectorError::element_not_found(format!(
        "{verb}: the element {}:{} did not appear within the timeout",
        sel.strategy.prefix(),
        sel.value
    ))
    .with_detail("selector", sel.describe())
}

fn click_failure(sel: &Sel, probe: &ClickProbe) -> ConnectorError {
    if !probe.found {
        return element_not_found(sel, "click");
    }
    let (reason, message) = if !probe.visible {
        ("not visible", "the element is not visible")
    } else if !probe.enabled {
        ("disabled", "the element is disabled")
    } else {
        ("obscured", "the element is obscured at its center")
    };
    let mut error = ConnectorError::not_interactable(format!(
        "click: {message} ({}:{})",
        sel.strategy.prefix(),
        sel.value
    ))
    .with_detail("selector", sel.describe())
    .with_detail("reason", reason);
    if !probe.obscured_by.is_empty() {
        error = error.with_detail("obscured_by", probe.obscured_by.clone());
    }
    error
}

fn editable_failure(sel: &Sel, mode: EnterMode, probe: &TextProbe) -> ConnectorError {
    if !probe.found {
        return element_not_found(sel, mode.as_script_mode());
    }
    let reason = if !probe.visible {
        "not visible"
    } else if !probe.enabled {
        "disabled"
    } else {
        "not editable"
    };
    ConnectorError::not_interactable(format!(
        "{}: the element is {reason} ({}:{})",
        mode.as_script_mode(),
        sel.strategy.prefix(),
        sel.value
    ))
    .with_detail("selector", sel.describe())
    .with_detail("reason", reason)
}

fn check_output(kind: pb::CheckKind, probe: &CheckProbe) -> CommandOutput {
    let (observed_text, observed_count) = observed_of(kind, probe);
    CommandOutput {
        observed_text,
        observed_count,
        ..CommandOutput::default()
    }
}

fn check_failure(
    kind: pb::CheckKind,
    check: &pb::ValidateCheck,
    sel: &Sel,
    probe: &CheckProbe,
) -> ConnectorError {
    let (observed, _) = observed_of(kind, probe);
    let expected = match kind {
        pb::CheckKind::Visible => "visible".to_string(),
        pb::CheckKind::Hidden => "hidden".to_string(),
        pb::CheckKind::Enabled => "enabled".to_string(),
        pb::CheckKind::Disabled => "disabled".to_string(),
        pb::CheckKind::Focused => "focused".to_string(),
        pb::CheckKind::TextEquals | pb::CheckKind::TextContainsCheck => check.expected_text.clone(),
        pb::CheckKind::Count => check.expected_count.to_string(),
        pb::CheckKind::Unspecified => String::new(),
    };
    let matched_count = if kind == pb::CheckKind::Count {
        probe.count.max(0)
    } else if probe.found {
        1
    } else {
        0
    };
    ConnectorError::check_failed(format!(
        "check failed for {}:{} (expected {expected}, observed {observed})",
        sel.strategy.prefix(),
        sel.value
    ))
    .with_detail("selector", sel.describe())
    .with_detail("expected", expected)
    .with_detail("observed", observed)
    .with_detail("matched_count", matched_count.to_string())
}

fn observed_of(kind: pb::CheckKind, probe: &CheckProbe) -> (String, u32) {
    match kind {
        pb::CheckKind::Visible | pb::CheckKind::Hidden => (
            if !probe.found {
                "missing".to_string()
            } else if probe.visible {
                "visible".to_string()
            } else {
                "hidden".to_string()
            },
            u32::from(probe.found),
        ),
        pb::CheckKind::Enabled | pb::CheckKind::Disabled => (
            if !probe.found {
                "missing".to_string()
            } else if probe.enabled {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
            u32::from(probe.found),
        ),
        pb::CheckKind::Focused => (
            if !probe.found {
                "missing".to_string()
            } else if probe.focused {
                "focused".to_string()
            } else {
                "not focused".to_string()
            },
            u32::from(probe.found),
        ),
        pb::CheckKind::TextEquals | pb::CheckKind::TextContainsCheck => {
            (probe.text.clone(), u32::from(probe.found))
        }
        pb::CheckKind::Count => {
            let count = probe.count.max(0);
            (count.to_string(), count as u32)
        }
        pb::CheckKind::Unspecified => (String::new(), 0),
    }
}

/// Parse a Chromium user agent into `(os_name, os_version, runtime)`.
#[must_use]
pub fn parse_user_agent(ua: &str) -> (String, String, String) {
    let runtime = extract_version(ua, "HeadlessChrome/")
        .or_else(|| extract_version(ua, "Chrome/"))
        .map_or_else(|| "Chromium".to_string(), |v| format!("Chromium/{v}"));
    let (os_name, os_version) = if let Some(version) = extract_version(ua, "Windows NT ") {
        ("Windows".to_string(), version)
    } else if let Some(version) = extract_version(ua, "Mac OS X ") {
        ("macOS".to_string(), version.replace('_', "."))
    } else if let Some(version) = extract_version(ua, "CrOS ") {
        ("ChromeOS".to_string(), version)
    } else if ua.contains("Android") {
        let version = extract_version(ua, "Android ").unwrap_or_default();
        ("Android".to_string(), version)
    } else if ua.contains("Linux") {
        ("Linux".to_string(), String::new())
    } else {
        ("Unknown".to_string(), String::new())
    };
    (os_name, os_version, runtime)
}

fn extract_version(ua: &str, marker: &str) -> Option<String> {
    let start = ua.find(marker)? + marker.len();
    let rest = ua.get(start..)?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    let value = rest.get(..end)?;
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

// ---------------------------------------------------------------------------
// the executor-side FocusProbe (spec 6.3)
// ---------------------------------------------------------------------------

struct ExecFocusProbe<'a> {
    executor: &'a Executor,
    sel: &'a Sel,
    cancel: &'a CancellationToken,
}

#[async_trait::async_trait]
impl FocusProbe for ExecFocusProbe<'_> {
    async fn current_focus(&self) -> Result<FocusState, ConnectorError> {
        let body = r#"
var active = window.__impresario.describeActive();
return {ok:true, present:active.present, label:active.label, frame:active.frame};
"#;
        let probe: ActiveProbe = self.executor.probe(body, self.cancel).await?;
        Ok(FocusState {
            present: probe.present,
            label: probe.label,
            frame: probe.frame,
        })
    }

    async fn target_frame(&self) -> Result<Option<Frame>, ConnectorError> {
        let body = format!(
            r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
var el = r.el;
if (!el || !window.__impresario.isVisible(el)) {{ return {{ok:true, frame:null}}; }}
return {{ok:true, frame:window.__impresario.rectOf(el)}};
"#,
            find = self.sel.find_call()
        );
        let probe: TargetProbe = self.executor.probe(&body, self.cancel).await?;
        Ok(probe.frame)
    }

    async fn target_focused(&self) -> Result<bool, ConnectorError> {
        let body = format!(
            r#"
var r = {find};
if (r.error) {{ return {{ok:false, invalid_selector:true, error:r.error}}; }}
return {{ok:true, focused:window.__impresario.isFocused(r.el)}};
"#,
            find = self.sel.find_call()
        );
        let probe: FocusedProbe = self.executor.probe(&body, self.cancel).await?;
        Ok(probe.focused)
    }

    async fn press(&self, direction: Direction) -> Result<(), ConnectorError> {
        self.executor
            .press_remote(direction_key(direction), 1, 0, self.cancel)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_sel(strategy: Strategy, value: &str) -> Sel {
        Sel {
            strategy,
            value: value.to_string(),
            index: 0,
        }
    }

    #[test]
    fn selector_parsing_rejects_foreign_strategies() {
        let pb_selector = pb::Selector {
            strategy: pb::SelectorStrategy::AndroidId as i32,
            value: "button".to_string(),
            index: 0,
        };
        let error = Sel::parse(&pb_selector).expect_err("android strategy on web");
        assert_eq!(error.code, WireErrorCode::InvalidSelector);

        let ok = pb::Selector {
            strategy: pb::SelectorStrategy::Css as i32,
            value: "#go".to_string(),
            index: 2,
        };
        let sel = Sel::parse(&ok).expect("css");
        assert_eq!(sel.index, 2);
        assert!(sel.find_call().contains("\"#go\""));
        assert!(sel.count_call().contains("countSafe"));
    }

    #[test]
    fn user_agent_parsing() {
        let (os, version, runtime) = parse_user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/131.0.6778.86 Safari/537.36",
        );
        assert_eq!(os, "Windows");
        assert_eq!(version, "10.0");
        assert_eq!(runtime, "Chromium/131.0.6778.86");

        let (os, version, _) = parse_user_agent(
            "Mozilla/5.0 (Linux; Android 13; Chromecast) AppleWebKit/537.36 Chrome/79.0.3945.79",
        );
        assert_eq!(os, "Android");
        assert_eq!(version, "13");

        let (os, _, runtime) = parse_user_agent("something else");
        assert_eq!(os, "Unknown");
        assert_eq!(runtime, "Chromium");
    }

    #[test]
    fn direction_and_key_mapping() {
        assert_eq!(direction_key(Direction::Down), RemoteKey::Down);
        assert_eq!(direction_name(Direction::Left), "left");
        let key = keys::web_key(Platform::Web, RemoteKey::Enter).expect("enter");
        let down = key_event(&key, KeyEventType::KeyDown);
        assert_eq!(down.text.as_deref(), Some("\r"));
        let up = key_event(&key, KeyEventType::KeyUp);
        assert!(up.text.is_none());
    }

    #[test]
    fn check_failure_carries_expected_observed_and_count() {
        let check = pb::ValidateCheck {
            selector: None,
            kind: pb::CheckKind::Count as i32,
            expected_text: String::new(),
            expected_count: 3,
        };
        let probe = CheckProbe {
            ok: true,
            count: 1,
            ..CheckProbe::default()
        };
        let sel = test_sel(Strategy::Css, ".item");
        let error = check_failure(pb::CheckKind::Count, &check, &sel, &probe);
        assert_eq!(error.code, WireErrorCode::CheckFailed);
        assert_eq!(error.details.get("expected").map(String::as_str), Some("3"));
        assert_eq!(error.details.get("observed").map(String::as_str), Some("1"));
        assert_eq!(
            error.details.get("matched_count").map(String::as_str),
            Some("1")
        );

        let check = pb::ValidateCheck {
            selector: None,
            kind: pb::CheckKind::TextEquals as i32,
            expected_text: "Hello".to_string(),
            expected_count: 0,
        };
        let probe = CheckProbe {
            ok: true,
            found: true,
            text: "Hi".to_string(),
            ..CheckProbe::default()
        };
        let error = check_failure(pb::CheckKind::TextEquals, &check, &sel, &probe);
        assert_eq!(
            error.details.get("observed").map(String::as_str),
            Some("Hi")
        );
        assert_eq!(
            error.details.get("matched_count").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn click_failure_distinguishes_missing_and_obscured() {
        let sel = test_sel(Strategy::Css, "#go");
        let missing = ClickProbe {
            ok: true,
            ..ClickProbe::default()
        };
        let error = click_failure(&sel, &missing);
        assert_eq!(error.code, WireErrorCode::ElementNotFound);

        let obscured = ClickProbe {
            ok: true,
            found: true,
            visible: true,
            enabled: true,
            obscured_by: "div#overlay".to_string(),
            ..ClickProbe::default()
        };
        let error = click_failure(&sel, &obscured);
        assert_eq!(error.code, WireErrorCode::NotInteractable);
        assert_eq!(
            error.details.get("obscured_by").map(String::as_str),
            Some("div#overlay")
        );

        let disabled = ClickProbe {
            ok: true,
            found: true,
            visible: true,
            enabled: false,
            ..ClickProbe::default()
        };
        let error = click_failure(&sel, &disabled);
        assert_eq!(
            error.details.get("reason").map(String::as_str),
            Some("disabled")
        );
    }

    #[test]
    fn editable_failure_maps_reasons() {
        let sel = test_sel(Strategy::Css, "#field");
        let readonly = TextProbe {
            ok: true,
            found: true,
            visible: true,
            enabled: true,
            editable: String::new(),
            ..TextProbe::default()
        };
        let error = editable_failure(&sel, EnterMode::Replace, &readonly);
        assert_eq!(error.code, WireErrorCode::NotInteractable);
        assert_eq!(
            error.details.get("reason").map(String::as_str),
            Some("not editable")
        );

        let missing = TextProbe::default();
        let error = editable_failure(&sel, EnterMode::Clear, &missing);
        assert_eq!(error.code, WireErrorCode::ElementNotFound);
    }
}
