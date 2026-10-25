//! Probe evidence model and verdict logic (TASK-3 section 3, D-26/D-27).
//!
//! Executors only *produce* [`Evidence`]. Every PASS/FAIL/UNVERIFIABLE
//! decision is made here so platform implementations cannot diverge. The
//! vocabularies and the per-platform support matrix in this module are the
//! single source for the DSL planner, PARITY, the LSP and the docs.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::platforms::Platform;

/// A probe family (wire `ProbeKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    Showing,
    Player,
    Drm,
}

impl ProbeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeKind::Showing => "showing",
            ProbeKind::Player => "player",
            ProbeKind::Drm => "drm",
        }
    }

    pub const ALL: [ProbeKind; 3] = [ProbeKind::Showing, ProbeKind::Player, ProbeKind::Drm];
}

/// Where an evidence item came from. New sources require a decision.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    A11yTree,
    SurfaceFlinger,
    MediaSession,
    AudioPlayback,
    MediaCodec,
    MediaDrm,
    LogcatTag,
    NetFlow,
    ScreenshotRegion,
    Dom,
    HtmlMedia,
    EmeHook,
    AvPlayHook,
    LunaMedia,
    DvtSysmon,
    DvtGraphics,
    DvtNetwork,
    Syslog,
    Pcapd,
    NowPlayingUi,
    AudioSession,
    AppHook,
    PlatformLog,
    DecoderLog,
    /// A source added by a future agent. Kept explicit in reports.
    Other(String),
}

impl EvidenceSource {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            EvidenceSource::A11yTree => "a11y_tree",
            EvidenceSource::SurfaceFlinger => "surfaceflinger",
            EvidenceSource::MediaSession => "media_session",
            EvidenceSource::AudioPlayback => "audio_playback",
            EvidenceSource::MediaCodec => "media_codec",
            EvidenceSource::MediaDrm => "media_drm",
            EvidenceSource::LogcatTag => "logcat_tag",
            EvidenceSource::NetFlow => "net_flow",
            EvidenceSource::ScreenshotRegion => "screenshot_region",
            EvidenceSource::Dom => "dom",
            EvidenceSource::HtmlMedia => "html_media",
            EvidenceSource::EmeHook => "eme_hook",
            EvidenceSource::AvPlayHook => "avplay_hook",
            EvidenceSource::LunaMedia => "luna_media",
            EvidenceSource::DvtSysmon => "dvt_sysmon",
            EvidenceSource::DvtGraphics => "dvt_graphics",
            EvidenceSource::DvtNetwork => "dvt_network",
            EvidenceSource::Syslog => "syslog",
            EvidenceSource::Pcapd => "pcapd",
            EvidenceSource::NowPlayingUi => "now_playing_ui",
            EvidenceSource::AudioSession => "audio_session",
            EvidenceSource::AppHook => "app_hook",
            EvidenceSource::PlatformLog => "platform_log",
            EvidenceSource::DecoderLog => "decoder_log",
            EvidenceSource::Other(name) => name,
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "a11y_tree" => EvidenceSource::A11yTree,
            "surfaceflinger" => EvidenceSource::SurfaceFlinger,
            "media_session" => EvidenceSource::MediaSession,
            "audio_playback" => EvidenceSource::AudioPlayback,
            "media_codec" => EvidenceSource::MediaCodec,
            "media_drm" => EvidenceSource::MediaDrm,
            "logcat_tag" => EvidenceSource::LogcatTag,
            "net_flow" => EvidenceSource::NetFlow,
            "screenshot_region" => EvidenceSource::ScreenshotRegion,
            "dom" => EvidenceSource::Dom,
            "html_media" => EvidenceSource::HtmlMedia,
            "eme_hook" => EvidenceSource::EmeHook,
            "avplay_hook" => EvidenceSource::AvPlayHook,
            "luna_media" => EvidenceSource::LunaMedia,
            "dvt_sysmon" => EvidenceSource::DvtSysmon,
            "dvt_graphics" => EvidenceSource::DvtGraphics,
            "dvt_network" => EvidenceSource::DvtNetwork,
            "syslog" => EvidenceSource::Syslog,
            "pcapd" => EvidenceSource::Pcapd,
            "now_playing_ui" => EvidenceSource::NowPlayingUi,
            "audio_session" => EvidenceSource::AudioSession,
            "app_hook" => EvidenceSource::AppHook,
            "platform_log" => EvidenceSource::PlatformLog,
            "decoder_log" => EvidenceSource::DecoderLog,
            other => EvidenceSource::Other(other.to_string()),
        }
    }
}

/// Evidence confidence class (TASK-3 section 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    Heuristic,
    System,
    Authoritative,
}

/// The confidence actually reached for a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// No agreeing evidence.
    None,
    /// Any agreeing evidence.
    Inferred,
    /// One AUTHORITATIVE item or at least two independent SYSTEM/HEURISTIC
    /// items from distinct sources with at least one SYSTEM item.
    Corroborated,
    /// At least one AUTHORITATIVE item.
    Authoritative,
}

impl Confidence {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::None => "none",
            Confidence::Inferred => "inferred",
            Confidence::Corroborated => "corroborated",
            Confidence::Authoritative => "authoritative",
        }
    }

    #[must_use]
    pub fn meets(self, required: Confidence) -> bool {
        self >= required
    }

    pub const ALL: [Confidence; 4] = [
        Confidence::None,
        Confidence::Inferred,
        Confidence::Corroborated,
        Confidence::Authoritative,
    ];
}

/// A typed, payload-free observation. Raw license keys, tokens and media
/// payloads never reach this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Observation {
    Bool(bool),
    Number(f64),
    Text(String),
    Enum(String),
    Range { min: f64, max: f64 },
}

impl Observation {
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Observation::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Observation::Number(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Observation::Text(value) | Observation::Enum(value) => Some(value),
            _ => None,
        }
    }

    /// Stable one-line rendering for reports. Floats use up to three
    /// decimals so evidence tables do not churn on tiny sampling noise.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Observation::Bool(value) => value.to_string(),
            Observation::Number(value) => {
                if value.fract().abs() < f64::EPSILON {
                    format!("{value:.0}")
                } else {
                    format!("{value:.3}")
                }
            }
            Observation::Text(value) | Observation::Enum(value) => value.clone(),
            Observation::Range { min, max } => format!("{min:.3}..{max:.3}"),
        }
    }
}

/// One observed fact, tied to the claim ids it supports or contradicts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub source: EvidenceSource,
    pub kind: String,
    pub observation: Observation,
    #[serde(default)]
    pub at_unix_ms: u64,
    #[serde(default)]
    pub window_ms: u32,
    pub class: EvidenceClass,
    #[serde(default)]
    pub supports: Vec<String>,
    #[serde(default)]
    pub contradicts: Vec<String>,
}

impl Evidence {
    #[must_use]
    pub fn supporting(
        source: EvidenceSource,
        kind: impl Into<String>,
        observation: Observation,
        class: EvidenceClass,
        claim: impl Into<String>,
    ) -> Self {
        Self {
            source,
            kind: kind.into(),
            observation,
            at_unix_ms: 0,
            window_ms: 0,
            class,
            supports: vec![claim.into()],
            contradicts: Vec::new(),
        }
    }

    #[must_use]
    pub fn contradicting(
        source: EvidenceSource,
        kind: impl Into<String>,
        observation: Observation,
        class: EvidenceClass,
        claim: impl Into<String>,
    ) -> Self {
        Self {
            source,
            kind: kind.into(),
            observation,
            at_unix_ms: 0,
            window_ms: 0,
            class,
            supports: Vec::new(),
            contradicts: vec![claim.into()],
        }
    }
}

/// Tri-state verdict (D-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    Unverifiable,
}

impl Verdict {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Unverifiable => "unverifiable",
        }
    }
}

/// A claim assertion supplied by the DSL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// Stable machine id, e.g. `state=Playing`, `drm.provider=Widevine`.
    pub id: String,
    /// Invert the expected truth (DSL `not`).
    #[serde(default)]
    pub negated: bool,
}

impl Claim {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            negated: false,
        }
    }

    #[must_use]
    pub fn negated(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            negated: true,
        }
    }
}

/// Comparison operators of `with` assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
}

impl Operator {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Operator::Eq => "=",
            Operator::Ne => "!=",
            Operator::Gt => ">",
            Operator::Ge => ">=",
            Operator::Lt => "<",
            Operator::Le => "<=",
            Operator::Contains => "~",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "=" | "==" => Some(Operator::Eq),
            "!=" => Some(Operator::Ne),
            ">" => Some(Operator::Gt),
            ">=" => Some(Operator::Ge),
            "<" => Some(Operator::Lt),
            "<=" => Some(Operator::Le),
            "~" => Some(Operator::Contains),
            _ => None,
        }
    }
}

/// One `with` assertion. A bare property (`muted`, `secure`) has no operator
/// and expects truthiness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeAssertion {
    pub property: String,
    pub operator: Option<Operator>,
    pub value: String,
    #[serde(default)]
    pub negated: bool,
}

impl ProbeAssertion {
    #[must_use]
    pub fn claim_id(&self) -> String {
        format!("property.{}", self.property)
    }

    /// Evaluate this assertion against an observed value.
    #[must_use]
    pub fn matches(&self, observation: &Observation) -> Option<bool> {
        let base = match self.operator {
            None => match observation {
                Observation::Bool(value) => *value,
                Observation::Number(value) => *value != 0.0,
                Observation::Text(value) | Observation::Enum(value) => !value.is_empty(),
                Observation::Range { .. } => true,
            },
            Some(Operator::Eq) => values_equal(observation, &self.value)?,
            Some(Operator::Ne) => !values_equal(observation, &self.value)?,
            Some(Operator::Contains) => observation
                .as_text()
                .map(|observed| observed.contains(&self.value))?,
            Some(operator) => compare_order(observation, &self.value, operator)?,
        };
        Some(if self.negated { !base } else { base })
    }
}

fn values_equal(observation: &Observation, expected: &str) -> Option<bool> {
    match observation {
        Observation::Bool(value) => parse_bool(expected).map(|expected| *value == expected),
        Observation::Number(value) => expected.parse::<f64>().ok().map(|expected| {
            // Tolerate the usual human-facing precision of media counters.
            (*value - expected).abs() <= 0.000_001
        }),
        Observation::Text(value) | Observation::Enum(value) => Some(value == expected),
        Observation::Range { .. } => None,
    }
}

fn compare_order(observation: &Observation, expected: &str, operator: Operator) -> Option<bool> {
    match observation {
        Observation::Number(value) => {
            let expected = expected.parse::<f64>().ok()?;
            Some(match operator {
                Operator::Gt => *value > expected,
                Operator::Ge => *value >= expected,
                Operator::Lt => *value < expected,
                Operator::Le => *value <= expected,
                _ => return None,
            })
        }
        Observation::Text(value) | Observation::Enum(value) => Some(match operator {
            Operator::Gt => value.as_str() > expected,
            Operator::Ge => value.as_str() >= expected,
            Operator::Lt => value.as_str() < expected,
            Operator::Le => value.as_str() <= expected,
            _ => return None,
        }),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Result of one claim, carried on the wire and into reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimVerdict {
    pub claim: String,
    pub verdict: Verdict,
    pub confidence: Confidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The complete result of one probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub verdicts: Vec<ClaimVerdict>,
    pub evidence: Vec<Evidence>,
    pub confidence: Confidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unverifiable_reason: Option<String>,
    #[serde(default)]
    pub observed: BTreeMap<String, String>,
    pub window_ms: u32,
}

impl ProbeResult {
    /// The worst claim verdict wins for the step status.
    #[must_use]
    pub fn overall(&self) -> Verdict {
        if self.verdicts.iter().any(|v| v.verdict == Verdict::Fail) {
            Verdict::Fail
        } else if self
            .verdicts
            .iter()
            .any(|v| v.verdict == Verdict::Unverifiable)
        {
            Verdict::Unverifiable
        } else {
            Verdict::Pass
        }
    }

    /// Human-readable one-line summary for reports and failure messages.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.verdicts.is_empty() {
            return "no claims".to_string();
        }
        self.verdicts
            .iter()
            .map(|claim| format!("{}={}", claim.claim, claim.verdict.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Compute the confidence reached by agreeing evidence for one claim.
#[must_use]
pub fn confidence_for(claim: &str, evidence: &[Evidence], positive: bool) -> Confidence {
    if positive {
        confidence_from_items(evidence.iter().filter(|item| {
            item.supports.iter().any(|id| id == claim) && item.contradicts.is_empty()
        }))
    } else {
        confidence_from_items(evidence.iter().filter(|item| {
            item.contradicts.iter().any(|id| id == claim) && item.supports.is_empty()
        }))
    }
}

fn confidence_from_items<'a>(items: impl Iterator<Item = &'a Evidence>) -> Confidence {
    let mut sources = std::collections::BTreeSet::new();
    let mut authoritative = false;
    let mut system = false;
    let mut heuristics = 0usize;
    for item in items {
        match item.class {
            EvidenceClass::Authoritative => authoritative = true,
            EvidenceClass::System => {
                system = true;
                sources.insert(item.source.as_str().to_string());
            }
            EvidenceClass::Heuristic => {
                heuristics += 1;
                sources.insert(item.source.as_str().to_string());
            }
        }
    }
    if authoritative {
        Confidence::Authoritative
    } else if sources.len() >= 2 && system {
        Confidence::Corroborated
    } else if system || heuristics > 0 {
        Confidence::Inferred
    } else {
        Confidence::None
    }
}

/// The single verdict function (D-27).
///
/// * PASS: the expected side reaches `min_confidence` and no evidence at that
///   confidence contradicts it.
/// * FAIL: evidence for the opposite side reaches `min_confidence` and no
///   evidence at that confidence supports the expectation.
/// * UNVERIFIABLE: neither side reaches the threshold, or both sides do.
#[must_use]
pub fn evaluate_claim(
    claim: &Claim,
    evidence: &[Evidence],
    min_confidence: Confidence,
) -> ClaimVerdict {
    let expected_positive = confidence_for(&claim.id, evidence, true);
    let expected_negative = confidence_for(&claim.id, evidence, false);
    let (support, contradict) = if claim.negated {
        (expected_negative, expected_positive)
    } else {
        (expected_positive, expected_negative)
    };
    let supports = support.meets(min_confidence);
    let contradicts = contradict.meets(min_confidence);
    match (supports, contradicts) {
        (true, false) => ClaimVerdict {
            claim: claim.id.clone(),
            verdict: Verdict::Pass,
            confidence: support,
            reason: None,
        },
        (false, true) => ClaimVerdict {
            claim: claim.id.clone(),
            verdict: Verdict::Fail,
            confidence: contradict,
            reason: Some("evidence contradicts the claim".to_string()),
        },
        (false, false) if support == Confidence::None && contradict == Confidence::None => {
            ClaimVerdict {
                claim: claim.id.clone(),
                verdict: Verdict::Unverifiable,
                confidence: Confidence::None,
                reason: Some("no evidence reached the required confidence".to_string()),
            }
        }
        (false, false) => ClaimVerdict {
            claim: claim.id.clone(),
            verdict: Verdict::Unverifiable,
            confidence: support.max(contradict),
            reason: Some(format!(
                "evidence is below the required confidence '{}'",
                min_confidence.as_str()
            )),
        },
        (true, true) => ClaimVerdict {
            claim: claim.id.clone(),
            verdict: Verdict::Unverifiable,
            confidence: support.max(contradict),
            reason: Some("conflicting evidence from different sources".to_string()),
        },
    }
}

/// Evaluate assertions against observed evidence. A property claim id is
/// `property.<name>`; collectors attach an [`Observation`] to that id.
#[must_use]
pub fn evaluate_assertions(
    assertions: &[ProbeAssertion],
    evidence: &[Evidence],
    min_confidence: Confidence,
) -> Vec<ClaimVerdict> {
    assertions
        .iter()
        .map(|assertion| {
            let claim = assertion.claim_id();
            let matching: Vec<&Evidence> = evidence
                .iter()
                .filter(|item| item.supports.iter().any(|id| id == &claim))
                .collect();
            let confidence = confidence_from_items(matching.iter().copied());
            if !confidence.meets(min_confidence) {
                return ClaimVerdict {
                    claim,
                    verdict: Verdict::Unverifiable,
                    confidence,
                    reason: Some("the property was not observed at the required confidence".into()),
                };
            }
            let mut matched = false;
            let mut rejected = false;
            for item in matching {
                match assertion.matches(&item.observation) {
                    Some(true) => matched = true,
                    Some(false) => rejected = true,
                    None => {}
                }
            }
            let verdict = if matched && !rejected {
                Verdict::Pass
            } else if rejected && !matched {
                Verdict::Fail
            } else {
                Verdict::Unverifiable
            };
            ClaimVerdict {
                claim,
                verdict,
                confidence,
                reason: (verdict == Verdict::Unverifiable)
                    .then(|| "conflicting observations for the property".to_string()),
            }
        })
        .collect()
}

/// Player status vocabulary (TASK-3 section 5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerStatus {
    Idle,
    Loading,
    Buffering,
    Ready,
    Playing,
    Paused,
    Seeking,
    Ended,
    Error,
}

impl PlayerStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PlayerStatus::Idle => "Idle",
            PlayerStatus::Loading => "Loading",
            PlayerStatus::Buffering => "Buffering",
            PlayerStatus::Ready => "Ready",
            PlayerStatus::Playing => "Playing",
            PlayerStatus::Paused => "Paused",
            PlayerStatus::Seeking => "Seeking",
            PlayerStatus::Ended => "Ended",
            PlayerStatus::Error => "Error",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Idle" => Some(PlayerStatus::Idle),
            "Loading" => Some(PlayerStatus::Loading),
            "Buffering" => Some(PlayerStatus::Buffering),
            "Ready" => Some(PlayerStatus::Ready),
            "Playing" => Some(PlayerStatus::Playing),
            "Paused" => Some(PlayerStatus::Paused),
            "Seeking" => Some(PlayerStatus::Seeking),
            "Ended" => Some(PlayerStatus::Ended),
            "Error" => Some(PlayerStatus::Error),
            _ => None,
        }
    }

    pub const ALL: [PlayerStatus; 9] = [
        PlayerStatus::Idle,
        PlayerStatus::Loading,
        PlayerStatus::Buffering,
        PlayerStatus::Ready,
        PlayerStatus::Playing,
        PlayerStatus::Paused,
        PlayerStatus::Seeking,
        PlayerStatus::Ended,
        PlayerStatus::Error,
    ];
}

/// DRM provider vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrmProvider {
    Widevine,
    PlayReady,
    FairPlay,
    ClearKey,
}

impl DrmProvider {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DrmProvider::Widevine => "Widevine",
            DrmProvider::PlayReady => "PlayReady",
            DrmProvider::FairPlay => "FairPlay",
            DrmProvider::ClearKey => "ClearKey",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Widevine" | "widevine" => Some(DrmProvider::Widevine),
            "PlayReady" | "playready" => Some(DrmProvider::PlayReady),
            "FairPlay" | "fairplay" => Some(DrmProvider::FairPlay),
            "ClearKey" | "clearkey" => Some(DrmProvider::ClearKey),
            _ => None,
        }
    }

    pub const ALL: [DrmProvider; 4] = [
        DrmProvider::Widevine,
        DrmProvider::PlayReady,
        DrmProvider::FairPlay,
        DrmProvider::ClearKey,
    ];
}

/// Player/DRM properties that may appear in a `with` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeProperty {
    Position,
    Duration,
    Buffered,
    Rate,
    Volume,
    Muted,
    Live,
    Looping,
    Fullscreen,
    Width,
    Height,
    Resolution,
    Bitrate,
    Framerate,
    DroppedFrames,
    VideoCodec,
    AudioCodec,
    Decoder,
    Secure,
    AudioTrack,
    SubtitleTrack,
    Error,
    License,
    KeyStatus,
    SecurityLevel,
    KeySystem,
    Hdcp,
}

impl ProbeProperty {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeProperty::Position => "position",
            ProbeProperty::Duration => "duration",
            ProbeProperty::Buffered => "buffered",
            ProbeProperty::Rate => "rate",
            ProbeProperty::Volume => "volume",
            ProbeProperty::Muted => "muted",
            ProbeProperty::Live => "live",
            ProbeProperty::Looping => "looping",
            ProbeProperty::Fullscreen => "fullscreen",
            ProbeProperty::Width => "width",
            ProbeProperty::Height => "height",
            ProbeProperty::Resolution => "resolution",
            ProbeProperty::Bitrate => "bitrate",
            ProbeProperty::Framerate => "framerate",
            ProbeProperty::DroppedFrames => "droppedFrames",
            ProbeProperty::VideoCodec => "videoCodec",
            ProbeProperty::AudioCodec => "audioCodec",
            ProbeProperty::Decoder => "decoder",
            ProbeProperty::Secure => "secure",
            ProbeProperty::AudioTrack => "audioTrack",
            ProbeProperty::SubtitleTrack => "subtitleTrack",
            ProbeProperty::Error => "error",
            ProbeProperty::License => "license",
            ProbeProperty::KeyStatus => "keyStatus",
            ProbeProperty::SecurityLevel => "securityLevel",
            ProbeProperty::KeySystem => "keySystem",
            ProbeProperty::Hdcp => "hdcp",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "position" => Some(ProbeProperty::Position),
            "duration" => Some(ProbeProperty::Duration),
            "buffered" => Some(ProbeProperty::Buffered),
            "rate" => Some(ProbeProperty::Rate),
            "volume" => Some(ProbeProperty::Volume),
            "muted" => Some(ProbeProperty::Muted),
            "live" => Some(ProbeProperty::Live),
            "looping" => Some(ProbeProperty::Looping),
            "fullscreen" => Some(ProbeProperty::Fullscreen),
            "width" => Some(ProbeProperty::Width),
            "height" => Some(ProbeProperty::Height),
            "resolution" => Some(ProbeProperty::Resolution),
            "bitrate" => Some(ProbeProperty::Bitrate),
            "framerate" => Some(ProbeProperty::Framerate),
            "droppedFrames" => Some(ProbeProperty::DroppedFrames),
            "videoCodec" => Some(ProbeProperty::VideoCodec),
            "audioCodec" => Some(ProbeProperty::AudioCodec),
            "decoder" => Some(ProbeProperty::Decoder),
            "secure" => Some(ProbeProperty::Secure),
            "audioTrack" => Some(ProbeProperty::AudioTrack),
            "subtitleTrack" => Some(ProbeProperty::SubtitleTrack),
            "error" => Some(ProbeProperty::Error),
            "license" => Some(ProbeProperty::License),
            "keyStatus" => Some(ProbeProperty::KeyStatus),
            "securityLevel" => Some(ProbeProperty::SecurityLevel),
            "keySystem" => Some(ProbeProperty::KeySystem),
            "hdcp" => Some(ProbeProperty::Hdcp),
            _ => None,
        }
    }

    pub const ALL: [ProbeProperty; 27] = [
        ProbeProperty::Position,
        ProbeProperty::Duration,
        ProbeProperty::Buffered,
        ProbeProperty::Rate,
        ProbeProperty::Volume,
        ProbeProperty::Muted,
        ProbeProperty::Live,
        ProbeProperty::Looping,
        ProbeProperty::Fullscreen,
        ProbeProperty::Width,
        ProbeProperty::Height,
        ProbeProperty::Resolution,
        ProbeProperty::Bitrate,
        ProbeProperty::Framerate,
        ProbeProperty::DroppedFrames,
        ProbeProperty::VideoCodec,
        ProbeProperty::AudioCodec,
        ProbeProperty::Decoder,
        ProbeProperty::Secure,
        ProbeProperty::AudioTrack,
        ProbeProperty::SubtitleTrack,
        ProbeProperty::Error,
        ProbeProperty::License,
        ProbeProperty::KeyStatus,
        ProbeProperty::SecurityLevel,
        ProbeProperty::KeySystem,
        ProbeProperty::Hdcp,
    ];
}

/// Per-platform source availability. `reason` explains an unavailable
/// source and surfaces in `doctor`/the probe inspect screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSupport {
    pub source: EvidenceSource,
    pub available: bool,
    pub reason: Option<String>,
}

/// The platform probe matrix (single source for plan-time checks and PARITY).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSupport {
    pub showing: bool,
    pub player: bool,
    pub drm: bool,
    pub properties: Vec<ProbeProperty>,
    pub providers: Vec<DrmProvider>,
    pub sources: Vec<SourceSupport>,
}

impl ProbeSupport {
    #[must_use]
    pub fn supports_property(&self, property: ProbeProperty) -> bool {
        self.properties.contains(&property)
    }

    #[must_use]
    pub fn supports_provider(&self, provider: DrmProvider) -> bool {
        self.providers.contains(&provider)
    }

    #[must_use]
    pub fn source(&self, source: &EvidenceSource) -> Option<&SourceSupport> {
        self.sources.iter().find(|item| &item.source == source)
    }
}

const COMMON_API_PROPERTIES: &[ProbeProperty] = &[
    ProbeProperty::Position,
    ProbeProperty::Duration,
    ProbeProperty::Buffered,
    ProbeProperty::Rate,
    ProbeProperty::Volume,
    ProbeProperty::Muted,
    ProbeProperty::Live,
    ProbeProperty::Looping,
    ProbeProperty::Fullscreen,
    ProbeProperty::Width,
    ProbeProperty::Height,
    ProbeProperty::Resolution,
    ProbeProperty::Bitrate,
    ProbeProperty::Framerate,
    ProbeProperty::DroppedFrames,
    ProbeProperty::VideoCodec,
    ProbeProperty::AudioCodec,
    ProbeProperty::Decoder,
    ProbeProperty::Secure,
    ProbeProperty::AudioTrack,
    ProbeProperty::SubtitleTrack,
    ProbeProperty::Error,
    ProbeProperty::License,
    ProbeProperty::KeyStatus,
    ProbeProperty::SecurityLevel,
    ProbeProperty::KeySystem,
    ProbeProperty::Hdcp,
];

const WEB_PROPERTIES: &[ProbeProperty] = &[
    ProbeProperty::Position,
    ProbeProperty::Duration,
    ProbeProperty::Buffered,
    ProbeProperty::Rate,
    ProbeProperty::Volume,
    ProbeProperty::Muted,
    ProbeProperty::Live,
    ProbeProperty::Looping,
    ProbeProperty::Width,
    ProbeProperty::Height,
    ProbeProperty::Resolution,
    ProbeProperty::DroppedFrames,
    ProbeProperty::VideoCodec,
    ProbeProperty::AudioCodec,
    ProbeProperty::Decoder,
    ProbeProperty::Secure,
    ProbeProperty::AudioTrack,
    ProbeProperty::SubtitleTrack,
    ProbeProperty::Error,
    ProbeProperty::License,
    ProbeProperty::KeyStatus,
    ProbeProperty::SecurityLevel,
    ProbeProperty::KeySystem,
    ProbeProperty::Hdcp,
];

const APPLE_PROPERTIES: &[ProbeProperty] = &[
    ProbeProperty::Position,
    ProbeProperty::Duration,
    ProbeProperty::Buffered,
    ProbeProperty::Rate,
    ProbeProperty::Volume,
    ProbeProperty::Muted,
    ProbeProperty::Live,
    ProbeProperty::Looping,
    ProbeProperty::Width,
    ProbeProperty::Height,
    ProbeProperty::Resolution,
    ProbeProperty::DroppedFrames,
    ProbeProperty::VideoCodec,
    ProbeProperty::AudioCodec,
    ProbeProperty::Decoder,
    ProbeProperty::Secure,
    ProbeProperty::AudioTrack,
    ProbeProperty::SubtitleTrack,
    ProbeProperty::Error,
    ProbeProperty::License,
    ProbeProperty::KeyStatus,
    ProbeProperty::SecurityLevel,
    ProbeProperty::KeySystem,
    ProbeProperty::Hdcp,
];

/// The platform's probe support. This describes what may be *reached*; a
/// third-party app can still yield UNVERIFIABLE (e.g. Apple DRM details).
#[must_use]
pub fn support(platform: Platform) -> ProbeSupport {
    match platform {
        Platform::Web | Platform::WebOs | Platform::Tizen => {
            let mut sources = vec![
                available(EvidenceSource::Dom),
                available(EvidenceSource::HtmlMedia),
                available(EvidenceSource::EmeHook),
                available(EvidenceSource::ScreenshotRegion),
                available(EvidenceSource::AppHook),
            ];
            if platform == Platform::Tizen {
                sources.push(available(EvidenceSource::AvPlayHook));
            } else {
                sources.push(
                    EvidenceSource::AvPlayHook.into_unavailable("AVPlay exists on Tizen only"),
                );
            }
            sources.push(
                EvidenceSource::LunaMedia
                    .into_unavailable("webOS native pipeline is not read by the connector"),
            );
            sources.push(
                EvidenceSource::NetFlow
                    .into_unavailable("network-metadata collector not implemented"),
            );
            sources.push(
                EvidenceSource::PlatformLog
                    .into_unavailable("platform-log collector not implemented"),
            );
            ProbeSupport {
                showing: true,
                player: true,
                drm: true,
                properties: WEB_PROPERTIES.to_vec(),
                providers: DrmProvider::ALL.to_vec(),
                sources,
            }
        }
        Platform::Android | Platform::AndroidTv => ProbeSupport {
            showing: true,
            player: true,
            drm: true,
            properties: COMMON_API_PROPERTIES.to_vec(),
            providers: vec![
                DrmProvider::Widevine,
                DrmProvider::PlayReady,
                DrmProvider::ClearKey,
            ],
            sources: vec![
                available(EvidenceSource::A11yTree),
                available(EvidenceSource::ScreenshotRegion),
                available(EvidenceSource::MediaSession),
                EvidenceSource::AudioPlayback
                    .into_unavailable("global AudioManager state is not scoped to the target app"),
                available(EvidenceSource::MediaDrm),
                available(EvidenceSource::AppHook),
                EvidenceSource::MediaCodec
                    .into_unavailable("codec counters are not exposed for arbitrary apps"),
                EvidenceSource::SurfaceFlinger
                    .into_unavailable("SurfaceFlinger layer parser not implemented"),
                EvidenceSource::LogcatTag.into_unavailable(
                    "the in-app hook is reported as app_hook; no generic logcat collector",
                ),
                EvidenceSource::NetFlow
                    .into_unavailable("network-metadata collector not implemented"),
            ],
        },
        Platform::Ios | Platform::Tvos => ProbeSupport {
            showing: true,
            player: true,
            drm: true,
            properties: APPLE_PROPERTIES.to_vec(),
            providers: vec![DrmProvider::FairPlay],
            sources: vec![
                available(EvidenceSource::A11yTree),
                available(EvidenceSource::ScreenshotRegion),
                available(EvidenceSource::AppHook),
                EvidenceSource::AudioSession.into_unavailable(
                    "no public API for another app's audio session; the hook reports media facts",
                ),
                EvidenceSource::NowPlayingUi
                    .into_unavailable("now-playing UI collector not implemented"),
                EvidenceSource::Syslog
                    .into_unavailable("host-assisted syslog collector not implemented"),
                EvidenceSource::DvtSysmon
                    .into_unavailable("host-assisted DVT collector not implemented"),
                EvidenceSource::DvtGraphics
                    .into_unavailable("host-assisted DVT collector not implemented"),
                EvidenceSource::DvtNetwork
                    .into_unavailable("host-assisted DVT collector not implemented"),
                EvidenceSource::Pcapd
                    .into_unavailable("host-assisted pcapd collector not implemented"),
                EvidenceSource::NetFlow
                    .into_unavailable("Apple agents use pcapd flow metadata only"),
            ],
        },
    }
}

fn available(source: EvidenceSource) -> SourceSupport {
    SourceSupport {
        source,
        available: true,
        reason: None,
    }
}

trait UnavailableSource {
    fn into_unavailable(self, reason: &str) -> SourceSupport;
}

impl UnavailableSource for EvidenceSource {
    fn into_unavailable(self, reason: &str) -> SourceSupport {
        SourceSupport {
            source: self,
            available: false,
            reason: Some(reason.to_string()),
        }
    }
}

/// Policy for claims that cannot reach the required confidence (D-26).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnverifiablePolicy {
    #[default]
    Fail,
    Warn,
}

impl UnverifiablePolicy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UnverifiablePolicy::Fail => "fail",
            UnverifiablePolicy::Warn => "warn",
        }
    }
}

/// The default required confidence (`probe.min_confidence`).
#[must_use]
pub fn default_min_confidence() -> Confidence {
    Confidence::Corroborated
}

/// `IMP-PROBE-*` reason codes for the first unverifiable claim.
#[must_use]
pub fn unverifiable_reason(result: &ProbeResult) -> Option<&str> {
    result
        .verdicts
        .iter()
        .find(|verdict| verdict.verdict == Verdict::Unverifiable)
        .and_then(|verdict| verdict.reason.as_deref())
        .or(result.unverifiable_reason.as_deref())
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(source: EvidenceSource, claim: &str, value: bool) -> Evidence {
        Evidence::supporting(
            source,
            "test",
            Observation::Bool(value),
            EvidenceClass::Authoritative,
            claim,
        )
    }

    fn system(source: EvidenceSource, claim: &str) -> Evidence {
        Evidence::supporting(
            source,
            "test",
            Observation::Bool(true),
            EvidenceClass::System,
            claim,
        )
    }

    fn contra(source: EvidenceSource, claim: &str, class: EvidenceClass) -> Evidence {
        Evidence::contradicting(source, "test", Observation::Bool(false), class, claim)
    }

    #[test]
    fn authoritative_support_passes_at_corroborated() {
        let result = evaluate_claim(
            &Claim::new("state=Playing"),
            &[auth(EvidenceSource::HtmlMedia, "state=Playing", true)],
            Confidence::Corroborated,
        );
        assert_eq!(result.verdict, Verdict::Pass);
        assert_eq!(result.confidence, Confidence::Authoritative);
    }

    #[test]
    fn two_independent_system_sources_corroborate() {
        let evidence = [
            system(EvidenceSource::MediaCodec, "state=Playing"),
            system(EvidenceSource::SurfaceFlinger, "state=Playing"),
        ];
        let result = evaluate_claim(
            &Claim::new("state=Playing"),
            &evidence,
            Confidence::Corroborated,
        );
        assert_eq!(result.verdict, Verdict::Pass);
        assert_eq!(result.confidence, Confidence::Corroborated);
    }

    #[test]
    fn one_system_source_is_only_inferred() {
        let evidence = [system(EvidenceSource::MediaCodec, "state=Playing")];
        let result = evaluate_claim(
            &Claim::new("state=Playing"),
            &evidence,
            Confidence::Corroborated,
        );
        assert_eq!(result.verdict, Verdict::Unverifiable);
        assert_eq!(result.confidence, Confidence::Inferred);
    }

    #[test]
    fn conflicting_strong_evidence_is_unverifiable() {
        let evidence = [
            auth(EvidenceSource::HtmlMedia, "state=Playing", true),
            contra(
                EvidenceSource::MediaCodec,
                "state=Playing",
                EvidenceClass::Authoritative,
            ),
        ];
        let result = evaluate_claim(
            &Claim::new("state=Playing"),
            &evidence,
            Confidence::Corroborated,
        );
        assert_eq!(result.verdict, Verdict::Unverifiable);
    }

    #[test]
    fn no_evidence_is_unverifiable_not_false() {
        let result = evaluate_claim(&Claim::new("state=Playing"), &[], Confidence::Corroborated);
        assert_eq!(result.verdict, Verdict::Unverifiable);
    }

    #[test]
    fn negation_inverts_the_expectation() {
        let evidence = [contra(
            EvidenceSource::HtmlMedia,
            "state=Playing",
            EvidenceClass::Authoritative,
        )];
        let result = evaluate_claim(
            &Claim::negated("state=Playing"),
            &evidence,
            Confidence::Corroborated,
        );
        assert_eq!(result.verdict, Verdict::Pass);
    }

    #[test]
    fn assertions_compare_typed_numbers() {
        let evidence = [Evidence::supporting(
            EvidenceSource::HtmlMedia,
            "position",
            Observation::Number(42.5),
            EvidenceClass::Authoritative,
            "property.position",
        )];
        let assertion = ProbeAssertion {
            property: "position".to_string(),
            operator: Some(Operator::Gt),
            value: "30".to_string(),
            negated: false,
        };
        let result = evaluate_assertions(&[assertion], &evidence, Confidence::Corroborated);
        assert_eq!(result[0].verdict, Verdict::Pass);
    }

    #[test]
    fn assertions_support_negation_and_missing_values() {
        let evidence = [Evidence::supporting(
            EvidenceSource::HtmlMedia,
            "muted",
            Observation::Bool(false),
            EvidenceClass::Authoritative,
            "property.muted",
        )];
        let assertion = ProbeAssertion {
            property: "muted".to_string(),
            operator: None,
            value: String::new(),
            negated: true,
        };
        let result = evaluate_assertions(&[assertion], &evidence, Confidence::Corroborated);
        assert_eq!(result[0].verdict, Verdict::Pass);
        let missing = ProbeAssertion {
            property: "secure".to_string(),
            operator: None,
            value: String::new(),
            negated: false,
        };
        let result = evaluate_assertions(&[missing], &evidence, Confidence::Corroborated);
        assert_eq!(result[0].verdict, Verdict::Unverifiable);
    }

    #[test]
    fn every_platform_has_a_support_row() {
        for platform in Platform::ALL {
            let row = support(platform);
            assert!(row.showing, "{} has no Showing support", platform);
            assert!(row.player, "{} has no Player support", platform);
            assert!(!row.properties.is_empty(), "{} has no properties", platform);
            assert!(!row.sources.is_empty(), "{} has no sources", platform);
        }
    }

    #[test]
    fn matrix_vocabularies_are_complete() {
        assert_eq!(PlayerStatus::ALL.len(), 9);
        assert_eq!(DrmProvider::ALL.len(), 4);
        assert_eq!(ProbeProperty::ALL.len(), 27);
        for status in PlayerStatus::ALL {
            assert_eq!(PlayerStatus::parse(status.as_str()), Some(status));
        }
        for provider in DrmProvider::ALL {
            assert_eq!(DrmProvider::parse(provider.as_str()), Some(provider));
        }
        for property in ProbeProperty::ALL {
            assert_eq!(ProbeProperty::parse(property.as_str()), Some(property));
        }
    }
}
