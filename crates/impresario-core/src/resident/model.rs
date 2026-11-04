//! Configuration model for the resident runner (TASK.md section 8).
//!
//! The same types are used by the workspace `impresario.config`, the global
//! `config.yaml` sections and by the CLI/TUI editors. Validation is kept in
//! this module so every front end reports the same problem for the same YAML.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::resident::schedule::{parse_span, validate_when};

/// Top-level `schedules:` and `watchers:` sections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidentSections {
    #[serde(default)]
    pub schedules: Vec<Schedule>,
    #[serde(default)]
    pub watchers: Vec<Watcher>,
}

impl ResidentSections {
    /// Read the two optional sections from an already-parsed YAML document.
    /// Unknown top-level keys are ignored because the global `config.yaml`
    /// also contains ordinary settings and profiles.
    pub fn from_value(value: &serde_yaml_ng::Value) -> Result<Self, String> {
        // A comment-only file parses as YAML null; it simply has no sections.
        if value.is_null() {
            return Ok(Self::default());
        }
        let Some(mapping) = value.as_mapping() else {
            return Err("config root must be a mapping".to_string());
        };
        let schedules = match mapping.get(serde_yaml_ng::Value::String("schedules".to_string())) {
            Some(value) => serde_yaml_ng::from_value(value.clone())
                .map_err(|error| format!("invalid schedules: {error}"))?,
            None => Vec::new(),
        };
        let watchers = match mapping.get(serde_yaml_ng::Value::String("watchers".to_string())) {
            Some(value) => serde_yaml_ng::from_value(value.clone())
                .map_err(|error| format!("invalid watchers: {error}"))?,
            None => Vec::new(),
        };
        let sections = Self {
            schedules,
            watchers,
        };
        sections.validate().map_err(|errors| errors.join("; "))?;
        Ok(sections)
    }

    /// Parse a whole global config file, ignoring unrelated top-level keys.
    pub fn from_yaml(source: &str) -> Result<Self, String> {
        let value = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(source)
            .map_err(|error| format!("invalid YAML: {error}"))?;
        Self::from_value(&value)
    }

    /// Validate both section lists, including uniqueness across each section.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let mut names = BTreeSet::new();
        for schedule in &self.schedules {
            if !names.insert(schedule.name.as_str()) {
                errors.push(format!("duplicate schedule name '{}'", schedule.name));
            }
            schedule.validate_into(&mut errors);
        }
        let mut names = BTreeSet::new();
        for watcher in &self.watchers {
            if !names.insert(watcher.name.as_str()) {
                errors.push(format!("duplicate watcher name '{}'", watcher.name));
            }
            watcher.validate_into(&mut errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Merge with another layer; a later schedule/watcher with the same name
    /// replaces the earlier one, matching the rest of the configuration UX.
    #[must_use]
    pub fn merged(mut self, overlay: Self) -> Self {
        for schedule in overlay.schedules {
            if let Some(slot) = self
                .schedules
                .iter_mut()
                .find(|existing| existing.name == schedule.name)
            {
                *slot = schedule;
            } else {
                self.schedules.push(schedule);
            }
        }
        for watcher in overlay.watchers {
            if let Some(slot) = self
                .watchers
                .iter_mut()
                .find(|existing| existing.name == watcher.name)
            {
                *slot = watcher;
            } else {
                self.watchers.push(watcher);
            }
        }
        self
    }
}

/// One scheduled run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    #[serde(default)]
    pub name: String,
    pub when: ScheduleWhen,
    #[serde(default)]
    pub run: RunSelection,
    #[serde(default)]
    pub on_missed: MissedPolicy,
    #[serde(default)]
    pub on_busy: BusyPolicy,
    #[serde(default)]
    pub notify: NotifySpec,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Schedule {
    fn validate_into(&self, errors: &mut Vec<String>) {
        validate_name("schedule", &self.name, errors);
        if let Err(error) = validate_when(&self.when) {
            errors.push(format!("schedule '{}': {error}", self.name));
        }
        self.run.validate_into("schedule", &self.name, errors);
        self.notify.validate_into("schedule", &self.name, errors);
    }

    /// The next fire strictly after `after` in the host local timezone.
    pub fn next_after(
        &self,
        after: &chrono::DateTime<chrono::Local>,
    ) -> Result<Option<chrono::DateTime<chrono::Local>>, String> {
        crate::resident::schedule::next_after(&self.when, after)
    }
}

/// When a schedule fires. Every wall-clock form is interpreted in the host's
/// local timezone; an interval is measured from the previous fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleWhen {
    Once { at: String },
    Daily { at: String },
    Weekly { days: Vec<String>, at: String },
    Interval { every: String },
    Cron { expr: String },
}

/// What to do when the host missed one or more fires while asleep/off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissedPolicy {
    #[default]
    Skip,
    RunOnce,
}

/// What to do when another job holds the selected device lease.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusyPolicy {
    #[default]
    Queue,
    Skip,
    Fail,
}

/// The run selection carried by a schedule or watcher.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSelection {
    #[serde(default)]
    pub plays: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    #[serde(default)]
    pub actors: Vec<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default)]
    pub profile: Option<String>,
}

impl RunSelection {
    fn validate_into(&self, kind: &str, name: &str, errors: &mut Vec<String>) {
        for (field, values) in [
            ("plays", &self.plays),
            ("tags", &self.tags),
            ("exclude_tags", &self.exclude_tags),
            ("actors", &self.actors),
        ] {
            for value in values {
                if value.trim().is_empty() {
                    errors.push(format!(
                        "{kind} '{name}': run.{field} contains an empty value"
                    ));
                }
            }
        }
        if let Some(retries) = self.retries {
            if retries > 5 {
                errors.push(format!(
                    "{kind} '{name}': run.retries must be between 0 and 5"
                ));
            }
        }
        for (field, value) in [("app", &self.app), ("profile", &self.profile)] {
            if value.as_deref().is_some_and(str::is_empty) {
                errors.push(format!("{kind} '{name}': run.{field} must not be empty"));
            }
        }
    }
}

/// Notification events and channels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifySpec {
    #[serde(default)]
    pub on: Vec<NotifyEvent>,
    #[serde(default)]
    pub channels: Vec<String>,
}

impl NotifySpec {
    fn validate_into(&self, kind: &str, name: &str, errors: &mut Vec<String>) {
        let mut events = BTreeSet::new();
        for event in &self.on {
            if !events.insert(*event) {
                errors.push(format!(
                    "{kind} '{name}': notify.on contains a duplicate event"
                ));
            }
        }
        for channel in &self.channels {
            if channel == "toast" {
                continue;
            }
            if let Some(webhook) = channel.strip_prefix("webhook:") {
                if valid_name(webhook) {
                    continue;
                }
            }
            errors.push(format!(
                "{kind} '{name}': unknown notify channel '{channel}' (expected toast or webhook:NAME)"
            ));
        }
    }
}

/// A notification trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyEvent {
    Failure,
    Unverifiable,
    DeviceUnreachable,
    Recovery,
}

/// An always-on watcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watcher {
    #[serde(default)]
    pub name: String,
    pub run: RunSelection,
    #[serde(default)]
    pub pacing: Pacing,
    #[serde(default)]
    pub priority: WatcherPriority,
    #[serde(default)]
    pub on_failure: FailurePolicy,
    #[serde(default)]
    pub device_health: DeviceHealth,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Watcher {
    fn validate_into(&self, errors: &mut Vec<String>) {
        validate_name("watcher", &self.name, errors);
        self.run.validate_into("watcher", &self.name, errors);
        if let Pacing::MinGap { gap } = &self.pacing {
            match parse_span(gap) {
                Ok(duration) if !duration.is_zero() => {}
                Ok(_) => errors.push(format!(
                    "watcher '{}': pacing.gap must be greater than zero",
                    self.name
                )),
                Err(error) => errors.push(format!("watcher '{}': pacing.gap {error}", self.name)),
            }
        }
        self.on_failure.validate_into(&self.name, errors);
        self.device_health.validate_into(&self.name, errors);
        self.retention.validate_into(&self.name, errors);
    }
}

/// Pacing between watcher iterations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Pacing {
    #[default]
    BackToBack,
    MinGap {
        gap: String,
    },
}

/// Watcher scheduling priority.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatcherPriority {
    #[default]
    Normal,
    Low,
}

/// What a failed/unverifiable watcher iteration records and notifies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailurePolicy {
    #[serde(default = "default_dump")]
    pub dump: Vec<DumpKind>,
    #[serde(default)]
    pub throttle: Throttle,
    #[serde(default)]
    pub notify: Vec<String>,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        Self {
            dump: default_dump(),
            throttle: Throttle::default(),
            notify: Vec::new(),
        }
    }
}

impl FailurePolicy {
    fn validate_into(&self, name: &str, errors: &mut Vec<String>) {
        let mut kinds = BTreeSet::new();
        for kind in &self.dump {
            if !kinds.insert(*kind) {
                errors.push(format!(
                    "watcher '{name}': on_failure.dump contains a duplicate item"
                ));
            }
        }
        self.throttle.validate_into(name, errors);
        let notify = NotifySpec {
            on: Vec::new(),
            channels: self.notify.clone(),
        };
        notify.validate_into("watcher", name, errors);
    }
}

/// Artifacts captured in a watcher dump bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DumpKind {
    Report,
    Screenshot,
    Logs,
    DeviceState,
    Evidence,
}

fn default_dump() -> Vec<DumpKind> {
    vec![
        DumpKind::Report,
        DumpKind::Screenshot,
        DumpKind::Logs,
        DumpKind::DeviceState,
        DumpKind::Evidence,
    ]
}

/// Deduplicates identical failures inside a moving window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Throttle {
    #[serde(default = "default_throttle")]
    pub same_failure_window: String,
}

impl Default for Throttle {
    fn default() -> Self {
        Self {
            same_failure_window: default_throttle(),
        }
    }
}

fn default_throttle() -> String {
    "15m".to_string()
}

impl Throttle {
    fn validate_into(&self, name: &str, errors: &mut Vec<String>) {
        match parse_span(&self.same_failure_window) {
            Ok(duration) if !duration.is_zero() => {}
            Ok(_) => errors.push(format!(
                "watcher '{name}': on_failure.throttle.same_failure_window must be greater than zero"
            )),
            Err(error) => errors.push(format!(
                "watcher '{name}': on_failure.throttle.same_failure_window {error}"
            )),
        }
    }
}

/// Device-health classification and backoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceHealth {
    #[serde(default)]
    pub treat_unreachable_as: FailureClass,
    #[serde(default = "default_backoff")]
    pub backoff: Vec<String>,
}

impl Default for DeviceHealth {
    fn default() -> Self {
        Self {
            treat_unreachable_as: FailureClass::Infrastructure,
            backoff: default_backoff(),
        }
    }
}

fn default_backoff() -> Vec<String> {
    vec![
        "10s".to_string(),
        "30s".to_string(),
        "2m".to_string(),
        "10m".to_string(),
    ]
}

impl DeviceHealth {
    fn validate_into(&self, name: &str, errors: &mut Vec<String>) {
        if self.backoff.is_empty() {
            errors.push(format!(
                "watcher '{name}': device_health.backoff must not be empty"
            ));
            return;
        }
        if self.backoff.len() > 16 {
            errors.push(format!(
                "watcher '{name}': device_health.backoff has more than 16 entries"
            ));
        }
        for value in &self.backoff {
            match parse_span(value) {
                Ok(duration) if !duration.is_zero() => {}
                Ok(_) => errors.push(format!(
                    "watcher '{name}': device_health.backoff values must be greater than zero"
                )),
                Err(error) => {
                    errors.push(format!("watcher '{name}': device_health.backoff {error}"))
                }
            }
        }
    }
}

/// Failure classes kept separate by the watcher.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Test,
    #[default]
    Infrastructure,
}

/// Report retention limits for a watcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retention {
    #[serde(default = "default_max_reports")]
    pub max_reports: u64,
    #[serde(default = "default_max_age")]
    pub max_age: String,
    #[serde(default = "default_max_disk")]
    pub max_disk: String,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_reports: default_max_reports(),
            max_age: default_max_age(),
            max_disk: default_max_disk(),
        }
    }
}

fn default_max_reports() -> u64 {
    500
}

fn default_max_age() -> String {
    "14d".to_string()
}

fn default_max_disk() -> String {
    "10GiB".to_string()
}

impl Retention {
    /// Retention values are validated here and enforced by the service.
    pub fn limits(&self) -> Result<(usize, chrono::Duration, u64), String> {
        if self.max_reports == 0 {
            return Err("retention.max_reports must be greater than zero".to_string());
        }
        let reports = usize::try_from(self.max_reports)
            .map_err(|_| "retention.max_reports is too large".to_string())?;
        let age = parse_span(&self.max_age)?;
        if age.is_zero() {
            return Err("retention.max_age must be greater than zero".to_string());
        }
        let disk = parse_size(&self.max_disk)?;
        if disk == 0 {
            return Err("retention.max_disk must be greater than zero".to_string());
        }
        Ok((reports, age, disk))
    }

    fn validate_into(&self, name: &str, errors: &mut Vec<String>) {
        if let Err(error) = self.limits() {
            errors.push(format!("watcher '{name}': {error}"));
        }
    }
}

fn default_true() -> bool {
    true
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn validate_name(kind: &str, name: &str, errors: &mut Vec<String>) {
    if !valid_name(name) {
        errors.push(format!(
            "{kind} '{name}': name must be 1..=64 ASCII letters, digits, '.', '-' or '_' and must not start with '.'"
        ));
    }
}

fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    if number.is_empty() {
        return Err(format!("'{text}' is not a size"));
    }
    let number: f64 = number
        .parse()
        .map_err(|_| format!("'{text}' is not a size"))?;
    if !number.is_finite() || number < 0.0 {
        return Err(format!("'{text}' is not a size"));
    }
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1_000.0,
        "mb" => 1_000_000.0,
        "gb" => 1_000_000_000.0,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("'{text}' has an unknown size unit")),
    };
    let bytes = number * multiplier;
    if bytes > u64::MAX as f64 {
        return Err(format!("'{text}' is too large"));
    }
    Ok(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resident::schedule::{parse_hhmm, validate_cron};

    fn schedule(yaml: &str) -> Schedule {
        serde_yaml_ng::from_str(yaml).expect("schedule")
    }

    #[test]
    fn parses_the_spec_example_and_rejects_duplicates() {
        let sections = ResidentSections::from_yaml(
            r#"
schedules:
  - name: nightly-smoke
    when: { kind: daily, at: "02:30" }
    run: { plays: [login, plays/streaming/*.play], tags: [smoke], exclude_tags: [wip],
           actors: [tv1, phone2, "@tvs"], app: shop, retries: 1, profile: smoke }
    on_missed: skip
    on_busy: queue
    notify: { on: [failure, unverifiable, device_unreachable], channels: [toast, "webhook:ops"] }
    enabled: true
watchers: []
"#,
        )
        .expect("sections");
        let first = sections.schedules.first().expect("schedule");
        assert_eq!(first.name, "nightly-smoke");
        assert_eq!(first.run.retries, Some(1));
        assert_eq!(first.notify.channels.len(), 2);

        let duplicate = ResidentSections {
            schedules: vec![
                schedule("name: one\nwhen: { kind: interval, every: 1h }"),
                schedule("name: one\nwhen: { kind: interval, every: 2h }"),
            ],
            watchers: Vec::new(),
        };
        let errors = duplicate.validate().expect_err("duplicate");
        assert!(errors
            .iter()
            .any(|error| error.contains("duplicate schedule")));
    }

    #[test]
    fn validates_channels_pacing_and_retention() {
        let sections = ResidentSections::from_yaml(
            r#"
schedules:
  - name: bad
    when: { kind: daily, at: "25:00" }
    notify: { channels: [email, "webhook:"] }
watchers:
  - name: soak
    run: { plays: [streaming_soak] }
    pacing: { mode: min_gap, gap: 0s }
    on_failure: { throttle: { same_failure_window: 0m } }
    device_health: { backoff: [] }
    retention: { max_reports: 0, max_age: 0d, max_disk: 0B }
"#,
        )
        .expect_err("invalid");
        assert!(
            sections.contains("02:30") || sections.contains("25:00"),
            "{sections}"
        );
        assert!(sections.contains("email"));
        assert!(sections.contains("pacing.gap"));
        assert!(sections.contains("same_failure_window"));
        assert!(sections.contains("backoff"));
        assert!(sections.contains("retention"));

        let zero_disk = Retention {
            max_reports: 1,
            max_age: "1d".to_string(),
            max_disk: "0B".to_string(),
        };
        assert!(zero_disk.limits().is_err());
    }

    #[test]
    fn parses_every_when_kind_and_sizes() {
        for text in [
            "name: once\nwhen: { kind: once, at: '2026-11-01T09:00' }",
            "name: daily\nwhen: { kind: daily, at: '02:30' }",
            "name: weekly\nwhen: { kind: weekly, days: [mon, thu], at: '08:00' }",
            "name: interval\nwhen: { kind: interval, every: 6h }",
            "name: cron\nwhen: { kind: cron, expr: '0 2 * * 1-5' }",
        ] {
            let schedule = schedule(text);
            let _ = schedule
                .next_after(&chrono::Local::now())
                .expect("valid expression");
        }
        let retention = Retention {
            max_reports: 2,
            max_age: "14d".to_string(),
            max_disk: "10GiB".to_string(),
        };
        let (reports, age, disk) = retention.limits().expect("limits");
        assert_eq!(reports, 2);
        assert_eq!(age, chrono::Duration::days(14));
        assert_eq!(disk, 10 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1.5 KiB").expect("size"), 1536);
    }

    #[test]
    fn merges_layers_by_name() {
        let base = ResidentSections {
            schedules: vec![schedule("name: one\nwhen: { kind: interval, every: 1h }")],
            watchers: Vec::new(),
        };
        let overlay = ResidentSections {
            schedules: vec![schedule("name: one\nwhen: { kind: interval, every: 2h }")],
            watchers: Vec::new(),
        };
        let merged = base.merged(overlay);
        assert_eq!(merged.schedules.len(), 1);
        assert!(matches!(
            merged.schedules.first().map(|value| &value.when),
            Some(crate::resident::model::ScheduleWhen::Interval { every }) if every == "2h"
        ));
    }

    #[test]
    fn parse_hhmm_rejects_out_of_range() {
        assert!(parse_hhmm("02:30").is_ok());
        assert!(parse_hhmm("2:30").is_ok());
        assert!(parse_hhmm("24:00").is_err());
        assert!(validate_cron("0 2 * * 1-5").is_ok());
        assert!(validate_cron("0 2 * *").is_err());
    }
}
