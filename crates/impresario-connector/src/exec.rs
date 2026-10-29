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
use impresario_core::probe::{
    evaluate_assertions, evaluate_claim, Claim, ClaimVerdict, Confidence, Evidence, EvidenceClass,
    EvidenceSource, Observation, Operator, ProbeAssertion, ProbeResult, Verdict,
};
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
    pub probe: Option<pb::ProbeResult>,
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

#[derive(Debug, Clone, Deserialize, Default)]
struct ProbeSample {
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
    hit: bool,
    #[serde(default)]
    background: String,
    #[serde(default)]
    rect: Option<ProbeRect>,
    #[serde(default)]
    viewport: Option<ProbeViewport>,
    #[serde(default)]
    media: Option<MediaSample>,
    #[serde(default)]
    image: Option<ImageSample>,
    #[serde(default)]
    avplay: Option<AvPlaySample>,
    #[serde(default)]
    eme: Option<EmeSample>,
    /// Optional in-app hook snapshot (D-41), if the app installed the SDK.
    #[serde(default)]
    app_hook: Option<AppHookSample>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AppHookSample {
    #[serde(default)]
    media: Option<HookMedia>,
    #[serde(default)]
    drm: Option<HookDrm>,
    #[serde(default)]
    showing: Option<HookShowing>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct HookMedia {
    #[serde(default)]
    state: String,
    #[serde(default)]
    position_seconds: Option<f64>,
    #[serde(default)]
    duration_seconds: Option<f64>,
    #[serde(default)]
    rate: Option<f64>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    muted: Option<bool>,
    #[serde(default)]
    width_px: Option<f64>,
    #[serde(default)]
    height_px: Option<f64>,
    #[serde(default)]
    audio_track: String,
    #[serde(default)]
    subtitle_track: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    decoded: Option<bool>,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct HookDrm {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    key_status: String,
    #[serde(default)]
    security_level: String,
    #[serde(default)]
    hdcp: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct HookShowing {
    #[serde(default)]
    decoded: Option<bool>,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
struct ProbeRect {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    w: f64,
    #[serde(default)]
    h: f64,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
struct ProbeViewport {
    #[serde(default)]
    width: f64,
    #[serde(default)]
    height: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MediaSample {
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    ended: bool,
    #[serde(default)]
    ready_state: u32,
    #[serde(default)]
    current_time: f64,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    live: bool,
    #[serde(default)]
    buffered_end: f64,
    #[serde(default)]
    seeking: bool,
    #[serde(default)]
    rate: f64,
    #[serde(default)]
    volume: f64,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    width: f64,
    #[serde(default)]
    height: f64,
    #[serde(default)]
    dropped_frames: f64,
    #[serde(default)]
    total_frames: f64,
    #[serde(default)]
    audio_track: String,
    #[serde(default)]
    subtitle_track: String,
    #[serde(default)]
    src: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    media_keys: bool,
    #[serde(default)]
    looping: bool,
    #[serde(default)]
    fullscreen: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ImageSample {
    #[serde(default)]
    complete: bool,
    #[serde(default)]
    natural_width: f64,
    #[serde(default)]
    src: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AvPlaySample {
    #[serde(default)]
    present: bool,
    #[serde(default)]
    state: String,
    #[serde(default)]
    current_time: f64,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    rate: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct EmeSample {
    #[serde(default)]
    available: bool,
    #[serde(default)]
    key_system: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    key_status: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    security_level: String,
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
    ProbeSample,
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
var vertical = {vertical};
var amount = 0.8 * window.__impresario.visibleExtent(target, vertical);
var before = target
  ? (vertical ? target.scrollTop : target.scrollLeft)
  : (vertical ? window.pageYOffset : window.pageXOffset);
var fired = false;
function onScroll() {{ fired = true; }}
// Capture phase sees element scrolls too (they do not bubble), and by the
// time a later frame runs the page's own listeners have also completed.
window.addEventListener('scroll', onScroll, true);
window.__impresario.scrollBy(target, {direction}, amount);
return new Promise(function (resolve) {{
  var frames = 0;
  function position() {{
    return target
      ? (vertical ? target.scrollTop : target.scrollLeft)
      : (vertical ? window.pageYOffset : window.pageXOffset);
  }}
  function finish() {{
    window.removeEventListener('scroll', onScroll, true);
    resolve({{ok:true}});
  }}
  function settle() {{
    // A clamp at the end does not dispatch an event; give the renderer two
    // frames, then report success without waiting further.
    if (frames >= 2 && (fired || position() === before)) {{ finish(); return; }}
    // A real scroll event can be delayed behind a busy compositor; keep
    // polling frames instead of assuming two are always enough.
    if (frames >= 30) {{ finish(); return; }}
    frames += 1;
    requestAnimationFrame(settle);
  }}
  requestAnimationFrame(settle);
}});
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

    /// Execute one `Probe` command (TASK-3 sections 4-6). The connector
    /// collects evidence; core types decide each verdict. A completed probe is
    /// an execution success even when a claim fails: the master applies the
    /// `unverifiable_policy` and turns verdicts into step status.
    pub async fn run_probe(
        &self,
        probe: &pb::ProbeCommand,
        timeout_ms: u32,
        cancel: &CancellationToken,
    ) -> Result<CommandOutput, ConnectorError> {
        cancel_slot(cancel)?;
        let kind = pb::ProbeKind::try_from(probe.kind)
            .map_err(|_| ConnectorError::protocol(format!("unknown probe kind {}", probe.kind)))?;
        if kind == pb::ProbeKind::Unspecified {
            return Err(ConnectorError::protocol("probe kind is unspecified"));
        }
        let selector = probe
            .selector
            .as_ref()
            .ok_or_else(|| ConnectorError::protocol("probe requires a selector"))?;
        let sel = Sel::parse(selector)?;
        let min_confidence = confidence_from_proto(probe.min_confidence)?;
        let window_ms = if probe.window_ms == 0 {
            3000
        } else {
            probe.window_ms
        };
        let sample_window = window_ms.min(timeout_ms.max(250)).clamp(250, 1500);

        // The hook is idempotent and must be present before the sample. It can
        // only observe EME calls made after installation; an already-created
        // `media.mediaKeys` is still authoritative for "DRM is in use".
        // Re-inject the DOM helper as well: a navigation between the session
        // bootstrap and this command drops it, and a probe has no action path
        // that would otherwise repair it.
        self.evaluate_value(&script::inject_expression(), cancel)
            .await?;
        self.evaluate_value(script::PROBE_HOOK_SCRIPT, cancel)
            .await?;

        let body = script::guarded(&script::probe_body(&sel.find_call()));
        let mut first = self.probe_sample(&body, cancel).await?;
        // A probe may arrive immediately after navigation; locate the piece
        // with the same bounded patience as a check instead of treating a
        // transient miss as a failed claim.
        if !first.found {
            let find_ms = timeout_ms.clamp(250, 2_000);
            let deadline = std::time::Instant::now() + Duration::from_millis(u64::from(find_ms));
            while !first.found && std::time::Instant::now() < deadline {
                self.pause(Duration::from_millis(50), cancel).await?;
                first = self.probe_sample(&body, cancel).await?;
            }
        }
        if kind == pb::ProbeKind::Showing && first.found {
            // Showing is a property of the rendered frame: give the piece the
            // same bounded settle window that a visible check uses, and wait
            // for decode when the DSL asked for it. Otherwise a screenshot of
            // a just-navigated page would report the previous frame.
            let settle_ms = sample_window.min(timeout_ms.max(250));
            let deadline = std::time::Instant::now() + Duration::from_millis(u64::from(settle_ms));
            loop {
                let decoded = !probe.require_decoded || showing_decoded(&first) == Some(true);
                if decoded && first.visible && first.hit {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                self.pause(Duration::from_millis(50), cancel).await?;
                first = self.probe_sample(&body, cancel).await?;
            }
            // One paint turn after decode, so the captured frame includes it.
            self.pause(Duration::from_millis(100), cancel).await?;
            first = self.probe_sample(&body, cancel).await?;
        }
        let second = if kind == pb::ProbeKind::Showing || !first.found {
            None
        } else {
            self.pause(Duration::from_millis(u64::from(sample_window / 2)), cancel)
                .await?;
            let sample = self.probe_sample(&body, cancel).await?;
            Some(sample)
        };

        let claims: Vec<Claim> = probe
            .claims
            .iter()
            .map(|claim| Claim {
                id: claim.claim.clone(),
                negated: claim.negated,
            })
            .collect();
        let assertions: Vec<ProbeAssertion> = probe
            .assertions
            .iter()
            .map(|assertion| ProbeAssertion {
                property: assertion.property.clone(),
                operator: if assertion.operator.is_empty() {
                    None
                } else {
                    Operator::parse(&assertion.operator)
                },
                value: assertion.value.clone(),
                negated: assertion.negated,
            })
            .collect();

        let now = unix_ms_now();
        let mut evidence = Vec::new();
        let mut coverage = None;
        match kind {
            pb::ProbeKind::Showing => {
                if first.found {
                    if let Ok(png) = self.backend.screenshot().await {
                        coverage = first
                            .rect
                            .zip(first.viewport)
                            .and_then(|(rect, viewport)| analyze_png_region(&png, rect, viewport));
                    }
                }
                collect_showing_evidence(
                    &first,
                    coverage,
                    &claims,
                    now,
                    sample_window,
                    &mut evidence,
                );
            }
            pb::ProbeKind::Player | pb::ProbeKind::Drm => {
                let second = second.as_ref().unwrap_or(&first);
                collect_player_evidence(
                    &first,
                    second,
                    min_confidence,
                    &claims,
                    &assertions,
                    now,
                    sample_window,
                    &mut evidence,
                );
            }
            pb::ProbeKind::Unspecified => {
                return Err(ConnectorError::protocol("probe kind is unspecified"));
            }
        }

        let mut verdicts: Vec<ClaimVerdict> = claims
            .iter()
            .map(|claim| evaluate_claim(claim, &evidence, min_confidence))
            .collect();
        verdicts.extend(evaluate_assertions(&assertions, &evidence, min_confidence));
        let confidence = verdicts
            .iter()
            .map(|verdict| verdict.confidence)
            .max()
            .unwrap_or(Confidence::None);
        let unverifiable_reason = verdicts
            .iter()
            .find(|verdict| verdict.verdict == Verdict::Unverifiable)
            .and_then(|verdict| verdict.reason.clone());
        let mut observed = std::collections::BTreeMap::new();
        for item in &evidence {
            if let Some(claim) = item.supports.first().or_else(|| item.contradicts.first()) {
                observed.insert(claim.clone(), item.observation.render());
            }
        }
        let result = ProbeResult {
            verdicts,
            evidence,
            confidence,
            unverifiable_reason,
            observed,
            window_ms,
        };

        Ok(CommandOutput {
            probe: Some(probe_result_to_proto(&result)),
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
            pb::AppOp::Background => Err(ConnectorError::unsupported_action(
                "app background is not supported on web platforms (spec 6.1)",
            )),
            pb::AppOp::Unspecified => Err(ConnectorError::protocol(
                "app op is unspecified".to_string(),
            )),
        }
    }

    /// Best-effort `document.readyState == "complete"` wait (10 s cap; a page
    /// that never finishes loading does not fail the launch).
    async fn wait_ready(&self, cancel: &CancellationToken) {
        self.wait_ready_state(
            "return {ok:true, complete:((document.readyState || '') === 'complete')};",
            Duration::from_secs(10),
            cancel,
        )
        .await;
    }

    /// Best-effort wait until the page has left the `loading` state, i.e.
    /// parsing finished and its inline scripts (event listeners) have run.
    ///
    /// The connector may attach to a page target while it is still parsing;
    /// acting then can set a scroll position before the page's own `scroll`
    /// listener exists. Five seconds cap, like the launch wait.
    pub(crate) async fn wait_dom_ready(&self, cancel: &CancellationToken) {
        self.wait_ready_state(
            "return {ok:true, complete:((document.readyState || '') !== 'loading')};",
            Duration::from_secs(5),
            cancel,
        )
        .await;
    }

    async fn wait_ready_state(&self, body: &str, cap: Duration, cancel: &CancellationToken) {
        let deadline = tokio::time::Instant::now() + cap;
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

    /// Sample a probe, repairing both injected scripts once if navigation
    /// replaced the document between injection and evaluation. The probe body
    /// is already guarded (it returns `ok=false` rather than throwing).
    async fn probe_sample(
        &self,
        expression: &str,
        cancel: &CancellationToken,
    ) -> Result<ProbeSample, ConnectorError> {
        let first = self.probe_once::<ProbeSample>(expression, cancel).await?;
        if first.ok {
            return Ok(first);
        }
        self.evaluate_value(&script::inject_expression(), cancel)
            .await?;
        self.evaluate_value(script::PROBE_HOOK_SCRIPT, cancel)
            .await?;
        let retry = self.probe_once::<ProbeSample>(expression, cancel).await?;
        if retry.ok {
            Ok(retry)
        } else {
            Err(probe_error(&retry))
        }
    }

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

fn collect_showing_evidence(
    sample: &ProbeSample,
    coverage: Option<f64>,
    claims: &[Claim],
    now: u64,
    window_ms: u32,
    out: &mut Vec<Evidence>,
) {
    let present_claim = claims.iter().find(|claim| claim.id == "showing=present");
    let dom_present = sample.found && sample.visible && sample.hit;
    if let Some(claim) = present_claim {
        let source = EvidenceSource::Dom;
        let class = if sample.found {
            EvidenceClass::System
        } else {
            EvidenceClass::Authoritative
        };
        out.push(claim_evidence(
            source,
            "visibility",
            Observation::Bool(dom_present),
            class,
            &claim.id,
            dom_present,
            now,
            window_ms,
        ));
        if let Some(coverage) = coverage {
            let painted = coverage >= 0.5;
            out.push(claim_evidence(
                EvidenceSource::ScreenshotRegion,
                "painted_region",
                Observation::Number(coverage),
                EvidenceClass::System,
                &claim.id,
                painted,
                now,
                window_ms,
            ));
        }
    }

    for claim in claims {
        if let Some(percent) = claim
            .id
            .strip_prefix("showing.coverage>=")
            .and_then(|value| value.strip_suffix('%'))
            .and_then(|value| value.parse::<f64>().ok())
        {
            if let Some(coverage) = coverage {
                out.push(claim_evidence(
                    EvidenceSource::ScreenshotRegion,
                    "coverage",
                    Observation::Number(coverage),
                    // The percentage is measured directly from the captured
                    // frame; there is no second independent source for pixels.
                    EvidenceClass::Authoritative,
                    &claim.id,
                    coverage >= percent,
                    now,
                    window_ms,
                ));
            }
        } else if claim.id == "showing.decoded=true" {
            if let Some(decoded) = showing_decoded(sample) {
                let source = if sample.app_hook.is_some() {
                    EvidenceSource::AppHook
                } else if sample.image.is_some() {
                    EvidenceSource::Dom
                } else {
                    EvidenceSource::HtmlMedia
                };
                out.push(claim_evidence(
                    source,
                    "decoded",
                    Observation::Bool(decoded),
                    EvidenceClass::Authoritative,
                    &claim.id,
                    decoded,
                    now,
                    window_ms,
                ));
            }
        } else if let Some(substring) = claim.id.strip_prefix("showing.source~") {
            if let Some(source) = showing_source(sample) {
                out.push(claim_evidence(
                    if sample.app_hook.is_some() {
                        EvidenceSource::AppHook
                    } else {
                        EvidenceSource::Dom
                    },
                    "source",
                    Observation::Text(source.clone()),
                    EvidenceClass::Authoritative,
                    &claim.id,
                    source.contains(substring),
                    now,
                    window_ms,
                ));
            }
        }
    }
}

fn hook_showing(sample: &ProbeSample) -> Option<&HookShowing> {
    sample.app_hook.as_ref()?.showing.as_ref()
}

fn hook_media(sample: &ProbeSample) -> Option<&HookMedia> {
    sample.app_hook.as_ref()?.media.as_ref()
}

fn showing_decoded(sample: &ProbeSample) -> Option<bool> {
    if let Some(decoded) = hook_showing(sample)
        .and_then(|showing| showing.decoded)
        .or_else(|| hook_media(sample).and_then(|media| media.decoded))
    {
        return Some(decoded);
    }
    if let Some(image) = &sample.image {
        return Some(image.complete && image.natural_width > 0.0);
    }
    sample
        .media
        .as_ref()
        .map(|media| media.ready_state >= 2 && media.error.is_empty())
}

fn showing_source(sample: &ProbeSample) -> Option<String> {
    if let Some(source) = hook_showing(sample)
        .map(|showing| showing.source.trim())
        .filter(|source| !source.is_empty())
        .or_else(|| {
            hook_media(sample)
                .map(|media| media.source.trim())
                .filter(|source| !source.is_empty())
        })
    {
        return Some(source.to_string());
    }
    if let Some(image) = &sample.image {
        if !image.src.is_empty() {
            return Some(image.src.clone());
        }
    }
    if let Some(media) = &sample.media {
        if !media.src.is_empty() {
            return Some(media.src.clone());
        }
    }
    let background = sample.background.trim();
    if let Some(start) = background.find("url(") {
        let rest = &background[start + 4..];
        let rest = rest.trim_start_matches(['"', '\'']);
        let end = rest.find(['"', '\'']).unwrap_or(rest.len());
        let value = &rest[..end];
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn collect_player_evidence(
    first: &ProbeSample,
    second: &ProbeSample,
    _min_confidence: Confidence,
    claims: &[Claim],
    assertions: &[ProbeAssertion],
    now: u64,
    window_ms: u32,
    out: &mut Vec<Evidence>,
) {
    let statuses = player_statuses(first, second);
    let status_claims: Vec<&Claim> = claims
        .iter()
        .filter(|claim| claim.id.starts_with("state="))
        .collect();
    if statuses.is_empty() && !status_claims.is_empty() {
        // A missing piece is authoritative for "there is no media element
        // here"; a present piece with no observable media stays merely
        // system-level so native playback can still be reported elsewhere.
        let (source, class) = if first.found {
            (EvidenceSource::HtmlMedia, EvidenceClass::System)
        } else {
            (EvidenceSource::Dom, EvidenceClass::Authoritative)
        };
        for claim in status_claims {
            out.push(claim_evidence(
                source.clone(),
                "media_state",
                Observation::Bool(false),
                class,
                &claim.id,
                false,
                now,
                window_ms,
            ));
        }
    } else {
        for claim in status_claims {
            for (source, status) in &statuses {
                out.push(claim_evidence(
                    source.clone(),
                    "media_state",
                    Observation::Enum(status.clone()),
                    EvidenceClass::Authoritative,
                    &claim.id,
                    claim.id == format!("state={status}"),
                    now,
                    window_ms,
                ));
            }
        }
    }

    let eme = second.eme.as_ref().or(first.eme.as_ref());
    let app_hook = second.app_hook.as_ref().or(first.app_hook.as_ref());
    let media_keys = second
        .media
        .as_ref()
        .or(first.media.as_ref())
        .is_some_and(|media| media.media_keys);
    for claim in claims {
        if let Some(expected) = claim
            .id
            .strip_prefix("drm.enabled=")
            .and_then(|value| value.parse::<bool>().ok())
        {
            if let Some(drm) = app_hook.and_then(|hook| hook.drm.as_ref()) {
                if let Some(enabled) = drm.enabled {
                    out.push(claim_evidence(
                        EvidenceSource::AppHook,
                        "drm_enabled",
                        Observation::Bool(enabled),
                        EvidenceClass::Authoritative,
                        &claim.id,
                        enabled == expected,
                        now,
                        window_ms,
                    ));
                }
            }
            if let Some(eme) = eme {
                out.push(claim_evidence(
                    EvidenceSource::EmeHook,
                    "drm_enabled",
                    Observation::Bool(eme.enabled),
                    if eme.available {
                        EvidenceClass::Authoritative
                    } else {
                        EvidenceClass::System
                    },
                    &claim.id,
                    eme.enabled == expected,
                    now,
                    window_ms,
                ));
            }
            if second.media.is_some() || first.media.is_some() {
                out.push(claim_evidence(
                    EvidenceSource::HtmlMedia,
                    "media_keys",
                    Observation::Bool(media_keys),
                    EvidenceClass::System,
                    &claim.id,
                    media_keys == expected,
                    now,
                    window_ms,
                ));
            }
        } else if let Some(expected) = claim.id.strip_prefix("drm.provider=") {
            if let Some(provider) = app_hook
                .and_then(|hook| hook.drm.as_ref())
                .map(|drm| drm.provider.trim())
                .filter(|provider| !provider.is_empty())
            {
                out.push(claim_evidence(
                    EvidenceSource::AppHook,
                    "drm_provider",
                    Observation::Enum(provider.to_string()),
                    EvidenceClass::Authoritative,
                    &claim.id,
                    provider.eq_ignore_ascii_case(expected),
                    now,
                    window_ms,
                ));
            }
            if let Some(eme) = eme {
                if !eme.provider.is_empty() {
                    out.push(claim_evidence(
                        EvidenceSource::EmeHook,
                        "drm_provider",
                        Observation::Enum(eme.provider.clone()),
                        EvidenceClass::Authoritative,
                        &claim.id,
                        eme.provider.eq_ignore_ascii_case(expected),
                        now,
                        window_ms,
                    ));
                }
            }
        }
    }

    let dt = (second
        .media
        .as_ref()
        .map(|media| media.current_time)
        .unwrap_or(0.0)
        - first
            .media
            .as_ref()
            .map(|media| media.current_time)
            .unwrap_or(0.0))
    .max(0.0);
    for assertion in assertions {
        if let Some((observation, source, class)) =
            observe_property(&assertion.property, first, second, dt, eme, media_keys)
        {
            out.push(claim_evidence(
                source,
                format!("property:{}", assertion.property),
                observation,
                class,
                &assertion.claim_id(),
                true,
                now,
                window_ms,
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn claim_evidence(
    source: EvidenceSource,
    kind: impl Into<String>,
    observation: Observation,
    class: EvidenceClass,
    claim: &str,
    truth: bool,
    now: u64,
    window_ms: u32,
) -> Evidence {
    let mut evidence = if truth {
        Evidence::supporting(source, kind, observation, class, claim)
    } else {
        Evidence::contradicting(source, kind, observation, class, claim)
    };
    evidence.at_unix_ms = now;
    evidence.window_ms = window_ms;
    evidence
}

fn player_statuses(first: &ProbeSample, second: &ProbeSample) -> Vec<(EvidenceSource, String)> {
    let mut statuses = Vec::new();
    if let Some(status) = hook_media(second)
        .or_else(|| hook_media(first))
        .map(|media| media.state.trim())
        .filter(|state| !state.is_empty())
    {
        statuses.push((EvidenceSource::AppHook, status.to_string()));
    }
    if let Some(status) = avplay_status(second).or_else(|| avplay_status(first)) {
        statuses.push((EvidenceSource::AvPlayHook, status.to_string()));
    }
    if let Some(status) = media_status(first, second) {
        statuses.push((EvidenceSource::HtmlMedia, status.to_string()));
    }
    statuses
}

fn media_status(first: &ProbeSample, second: &ProbeSample) -> Option<&'static str> {
    let media = second.media.as_ref().or(first.media.as_ref())?;
    if !media.error.is_empty() {
        return Some("Error");
    }
    if media.seeking {
        return Some("Seeking");
    }
    if media.ended {
        return Some("Ended");
    }
    if media.paused {
        return Some(if media.current_time <= 0.05 && media.ready_state < 3 {
            if media.ready_state >= 2 {
                "Ready"
            } else {
                "Loading"
            }
        } else {
            "Paused"
        });
    }
    if media.ready_state < 2 {
        return Some("Loading");
    }
    let advanced =
        second
            .media
            .as_ref()
            .zip(first.media.as_ref())
            .is_some_and(|(later, earlier)| {
                later.current_time > earlier.current_time + 0.01
                    || later.total_frames > earlier.total_frames
            });
    if advanced || (media.ready_state >= 3 && media.buffered_end > media.current_time + 0.05) {
        Some("Playing")
    } else {
        Some("Buffering")
    }
}

/// AVPlay is exposed on every Tizen page, even when the app uses an HTML
/// `<video>`. `getState()` returns `NONE` until a session is opened, so an
/// unused API must not be reported as an idle player and contradict the HTML
/// media collector.
fn active_avplay(sample: &ProbeSample) -> Option<&AvPlaySample> {
    sample.avplay.as_ref().filter(|avplay| {
        avplay.present && !avplay.state.is_empty() && !avplay.state.eq_ignore_ascii_case("NONE")
    })
}

fn avplay_status(sample: &ProbeSample) -> Option<&'static str> {
    let avplay = active_avplay(sample)?;
    match avplay.state.as_str() {
        "IDLE" => Some("Idle"),
        "READY" => Some("Ready"),
        "PLAYING" => Some("Playing"),
        "PAUSED" => Some("Paused"),
        "BUFFERING" => Some("Buffering"),
        "SEEKING" => Some("Seeking"),
        "COMPLETED" | "STOPPED" => Some("Ended"),
        "ERROR" => Some("Error"),
        _ => None,
    }
}

/// Direct properties from the optional in-app hook. These are authoritative
/// because the app itself reports them; missing fields fall through to the
/// platform collectors instead of inventing a value.
fn hook_property(
    property: &str,
    hook: &AppHookSample,
) -> Option<(Observation, EvidenceSource, EvidenceClass)> {
    let media = hook.media.as_ref();
    let drm = hook.drm.as_ref();
    let number = |value: Option<f64>| {
        value.map(|value| {
            (
                Observation::Number(value),
                EvidenceSource::AppHook,
                EvidenceClass::Authoritative,
            )
        })
    };
    let text = |value: Option<&str>| {
        value.filter(|value| !value.is_empty()).map(|value| {
            (
                Observation::Text(value.to_string()),
                EvidenceSource::AppHook,
                EvidenceClass::Authoritative,
            )
        })
    };
    match property {
        "position" => number(media.and_then(|media| media.position_seconds)),
        "duration" => number(media.and_then(|media| media.duration_seconds)),
        "rate" => number(media.and_then(|media| media.rate)),
        "volume" => number(media.and_then(|media| media.volume)),
        "muted" => media.and_then(|media| media.muted).map(|value| {
            (
                Observation::Bool(value),
                EvidenceSource::AppHook,
                EvidenceClass::Authoritative,
            )
        }),
        "width" => number(media.and_then(|media| media.width_px)),
        "height" => number(media.and_then(|media| media.height_px)),
        "resolution" => number(media.and_then(|media| media.height_px)),
        "audioTrack" => text(media.map(|media| media.audio_track.as_str())),
        "subtitleTrack" => text(media.map(|media| media.subtitle_track.as_str())),
        "error" => text(media.map(|media| media.error.as_str())),
        "secure" => drm.and_then(|drm| drm.enabled).map(|value| {
            (
                Observation::Bool(value),
                EvidenceSource::AppHook,
                EvidenceClass::Authoritative,
            )
        }),
        "license" => text(drm.map(|drm| drm.license.as_str())),
        "keyStatus" => text(drm.map(|drm| drm.key_status.as_str())),
        "securityLevel" => text(drm.map(|drm| drm.security_level.as_str())),
        "keySystem" => text(drm.map(|drm| drm.provider.as_str())),
        "hdcp" => text(drm.map(|drm| drm.hdcp.as_str())),
        _ => None,
    }
}

fn observe_property(
    property: &str,
    first: &ProbeSample,
    second: &ProbeSample,
    dt: f64,
    eme: Option<&EmeSample>,
    media_keys: bool,
) -> Option<(Observation, EvidenceSource, EvidenceClass)> {
    if let Some(found) = second
        .app_hook
        .as_ref()
        .or(first.app_hook.as_ref())
        .and_then(|hook| hook_property(property, hook))
    {
        return Some(found);
    }
    let media = second.media.as_ref().or(first.media.as_ref());
    let avplay = active_avplay(second).or_else(|| active_avplay(first));
    let source = EvidenceSource::HtmlMedia;
    let authoritative = EvidenceClass::Authoritative;
    let value = match property {
        "position" => match (media, avplay) {
            (Some(media), _) => media.current_time,
            (None, Some(avplay)) => avplay.current_time,
            (None, None) => return None,
        },
        "duration" => match (media.and_then(|media| media.duration), avplay) {
            (Some(duration), _) => duration,
            (None, Some(avplay)) if avplay.duration > 0.0 => avplay.duration,
            (None, _) => return None,
        },
        "buffered" => {
            let media = media?;
            (media.buffered_end - media.current_time).max(0.0)
        }
        "rate" => match (media, avplay) {
            (Some(media), _) => media.rate,
            (None, Some(avplay)) => avplay.rate,
            (None, None) => return None,
        },
        "volume" => media.map(|media| media.volume)?,
        "muted" => return Some((Observation::Bool(media?.muted), source, authoritative)),
        "live" => return Some((Observation::Bool(media?.live), source, authoritative)),
        "looping" => return Some((Observation::Bool(media?.looping), source, authoritative)),
        "fullscreen" => return Some((Observation::Bool(media?.fullscreen), source, authoritative)),
        "width" => media.map(|media| media.width)?,
        "height" => media.map(|media| media.height)?,
        "resolution" => {
            let media = media?;
            if media.height <= 0.0 {
                return None;
            }
            return Some((Observation::Number(media.height), source, authoritative));
        }
        "framerate" => {
            if dt <= 0.0 {
                return None;
            }
            let later = second.media.as_ref()?;
            let earlier = first.media.as_ref()?;
            let frames = later.total_frames - earlier.total_frames;
            if frames <= 0.0 {
                return None;
            }
            return Some((
                Observation::Number(frames / dt),
                EvidenceSource::HtmlMedia,
                EvidenceClass::Heuristic,
            ));
        }
        "droppedFrames" => media.map(|media| media.dropped_frames)?,
        "audioTrack" => {
            return Some((
                Observation::Text(media?.audio_track.clone()),
                source,
                authoritative,
            ))
        }
        "subtitleTrack" => {
            return Some((
                Observation::Text(media?.subtitle_track.clone()),
                source,
                authoritative,
            ))
        }
        "error" => {
            return Some((
                Observation::Text(media?.error.clone()),
                source,
                authoritative,
            ))
        }
        "secure" => {
            return Some((
                Observation::Bool(media_keys || eme.is_some_and(|eme| eme.enabled)),
                EvidenceSource::EmeHook,
                EvidenceClass::Authoritative,
            ))
        }
        "license" => {
            let eme = eme?;
            if eme.license.is_empty() || eme.license == "none" {
                return None;
            }
            return Some((
                Observation::Text(eme.license.clone()),
                EvidenceSource::EmeHook,
                EvidenceClass::Authoritative,
            ));
        }
        "keyStatus" => {
            let eme = eme?;
            if eme.key_status.is_empty() {
                return None;
            }
            return Some((
                Observation::Text(eme.key_status.clone()),
                EvidenceSource::EmeHook,
                EvidenceClass::Authoritative,
            ));
        }
        "securityLevel" => {
            let eme = eme?;
            if eme.security_level.is_empty() {
                return None;
            }
            return Some((
                Observation::Text(eme.security_level.clone()),
                EvidenceSource::EmeHook,
                EvidenceClass::Authoritative,
            ));
        }
        "keySystem" => {
            let eme = eme?;
            if eme.key_system.is_empty() {
                return None;
            }
            return Some((
                Observation::Text(eme.key_system.clone()),
                EvidenceSource::EmeHook,
                EvidenceClass::Authoritative,
            ));
        }
        _ => return None,
    };
    let source = if media.is_none() && avplay.is_some() {
        EvidenceSource::AvPlayHook
    } else {
        source
    };
    Some((Observation::Number(value), source, authoritative))
}

fn confidence_from_proto(value: i32) -> Result<Confidence, ConnectorError> {
    match pb::ProbeConfidence::try_from(value) {
        Ok(pb::ProbeConfidence::None) => Ok(Confidence::Corroborated),
        Ok(pb::ProbeConfidence::Inferred) => Ok(Confidence::Inferred),
        Ok(pb::ProbeConfidence::Corroborated) => Ok(Confidence::Corroborated),
        Ok(pb::ProbeConfidence::Authoritative) => Ok(Confidence::Authoritative),
        Err(_) => Err(ConnectorError::protocol(format!(
            "unknown probe confidence value {value}"
        ))),
    }
}

fn observation_to_proto(observation: &Observation) -> (String, String) {
    match observation {
        Observation::Bool(value) => ("bool".to_string(), value.to_string()),
        Observation::Number(value) => ("number".to_string(), value.to_string()),
        Observation::Text(value) => ("text".to_string(), value.clone()),
        Observation::Enum(value) => ("enum".to_string(), value.clone()),
        Observation::Range { min, max } => ("range".to_string(), format!("{min}..{max}")),
    }
}

fn probe_result_to_proto(result: &ProbeResult) -> pb::ProbeResult {
    pb::ProbeResult {
        verdicts: result
            .verdicts
            .iter()
            .map(|verdict| pb::ClaimVerdict {
                claim: verdict.claim.clone(),
                verdict: match verdict.verdict {
                    Verdict::Pass => pb::VerdictState::Pass as i32,
                    Verdict::Fail => pb::VerdictState::Fail as i32,
                    Verdict::Unverifiable => pb::VerdictState::Unverifiable as i32,
                },
                confidence: confidence_to_proto(verdict.confidence) as i32,
                reason: verdict.reason.clone().unwrap_or_default(),
            })
            .collect(),
        evidence: result
            .evidence
            .iter()
            .map(|evidence| {
                let (observation_type, observation) = observation_to_proto(&evidence.observation);
                pb::ProbeEvidence {
                    source: evidence.source.as_str().to_string(),
                    kind: evidence.kind.clone(),
                    observation_type,
                    observation,
                    at_unix_ms: evidence.at_unix_ms,
                    window_ms: evidence.window_ms,
                    class: match evidence.class {
                        EvidenceClass::Heuristic => pb::EvidenceClass::Heuristic as i32,
                        EvidenceClass::System => pb::EvidenceClass::System as i32,
                        EvidenceClass::Authoritative => pb::EvidenceClass::Authoritative as i32,
                    },
                    supports: evidence.supports.clone(),
                    contradicts: evidence.contradicts.clone(),
                }
            })
            .collect(),
        confidence: confidence_to_proto(result.confidence) as i32,
        unverifiable_reason: result.unverifiable_reason.clone().unwrap_or_default(),
        observed: result.observed.clone().into_iter().collect(),
        window_ms: result.window_ms,
    }
}

fn confidence_to_proto(confidence: Confidence) -> pb::ProbeConfidence {
    match confidence {
        Confidence::None => pb::ProbeConfidence::None,
        Confidence::Inferred => pb::ProbeConfidence::Inferred,
        Confidence::Corroborated => pb::ProbeConfidence::Corroborated,
        Confidence::Authoritative => pb::ProbeConfidence::Authoritative,
    }
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Share of pixels in the selected region that differ from the region mean.
/// A uniform (black/blank) region scores 0; normal video/UI scores well above
/// the presence threshold without claiming semantic content.
fn analyze_png_region(png: &[u8], rect: ProbeRect, viewport: ProbeViewport) -> Option<f64> {
    if rect.w <= 0.0 || rect.h <= 0.0 || viewport.width <= 0.0 || viewport.height <= 0.0 {
        return Some(0.0);
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let width = info.width as f64;
    let height = info.height as f64;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let scale_x = width / viewport.width;
    let scale_y = height / viewport.height;
    let left = (rect.x * scale_x).floor().max(0.0) as usize;
    let top = (rect.y * scale_y).floor().max(0.0) as usize;
    let right = ((rect.x + rect.w) * scale_x).ceil().min(width) as usize;
    let bottom = ((rect.y + rect.h) * scale_y).ceil().min(height) as usize;
    if right <= left || bottom <= top {
        return Some(0.0);
    }
    let stride = info.width as usize * channels;
    let mut luma = Vec::with_capacity((right - left) * (bottom - top));
    for y in top..bottom {
        let row = buffer.get(y * stride..)?;
        for x in left..right {
            let index = x * channels;
            let pixel = row.get(index..index + channels)?;
            let alpha = match info.color_type {
                png::ColorType::Rgba => pixel.get(3).copied().unwrap_or(0),
                png::ColorType::GrayscaleAlpha => pixel.get(1).copied().unwrap_or(0),
                _ => 255,
            };
            if alpha < 8 {
                continue;
            }
            let first_channel = pixel.first().copied().unwrap_or(0);
            let value = match info.color_type {
                png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => {
                    f64::from(first_channel)
                }
                _ => {
                    0.2126 * f64::from(first_channel)
                        + 0.7152 * f64::from(pixel.get(1).copied().unwrap_or(0))
                        + 0.0722 * f64::from(pixel.get(2).copied().unwrap_or(0))
                }
            };
            luma.push(value);
        }
    }
    if luma.is_empty() {
        return Some(0.0);
    }
    let mean = luma.iter().sum::<f64>() / luma.len() as f64;
    let painted = luma
        .iter()
        .filter(|value| (*value - mean).abs() > 6.0)
        .count();
    Some(painted as f64 * 100.0 / luma.len() as f64)
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
    fn unused_avplay_is_not_an_idle_player() {
        let mut sample = ProbeSample {
            avplay: Some(AvPlaySample {
                present: true,
                state: "NONE".to_string(),
                ..AvPlaySample::default()
            }),
            ..ProbeSample::default()
        };
        assert_eq!(avplay_status(&sample), None);
        assert!(active_avplay(&sample).is_none());

        sample.avplay.as_mut().expect("avplay").state = "IDLE".to_string();
        assert_eq!(avplay_status(&sample), Some("Idle"));

        sample.avplay.as_mut().expect("avplay").state = "PLAYING".to_string();
        assert_eq!(avplay_status(&sample), Some("Playing"));
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
