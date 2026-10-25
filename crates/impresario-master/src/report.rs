//! The run report model (spec section 8.4 and Appendix B).
//!
//! The executor fills this structure and every reporter (console, junit,
//! json, html) renders from it. Strings stored here are already redacted
//! (`impresario_master::redact`); `source` fields intentionally hold the
//! *unresolved* step source text (spec 5.6).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Run-level status (Appendix B): `passed|failed|interrupted|error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Passed,
    Failed,
    Interrupted,
    Error,
}

impl RunStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Passed => "passed",
            RunStatus::Failed => "failed",
            RunStatus::Interrupted => "interrupted",
            RunStatus::Error => "error",
        }
    }
}

/// Act and step status: `passed|failed|skipped|aborted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Passed,
    Failed,
    Skipped,
    Aborted,
    Unverifiable,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
            Status::Aborted => "aborted",
            Status::Unverifiable => "unverifiable",
        }
    }
}

/// `{ code, message, details }` with everything already redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorJson {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: BTreeMap<String, String>,
}

/// Span of a step, as written (Appendix B shows file/line/col; `len` is
/// carried too so tooling can underline the line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanJson {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

impl From<&impresario_core::span::Span> for SpanJson {
    fn from(span: &impresario_core::span::Span) -> Self {
        Self {
            file: span.file.display().to_string(),
            line: span.line,
            col: span.col,
            len: span.len,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    pub source: String,
    pub span: SpanJson,
    pub status: Status,
    #[serde(default)]
    pub duration_ms: u64,
    pub error: Option<ErrorJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageReport {
    pub name: String,
    /// Source location of the stage declaration in the `.theatre` file.
    #[serde(default)]
    pub span: Option<SpanJson>,
    pub status: Status,
    pub steps: Vec<StepReport>,
}

/// Failure detail of a failed attempt (Appendix B).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureReport {
    pub step_index: usize,
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: BTreeMap<String, String>,
    pub screenshot: Option<String>,
    #[serde(default)]
    pub log_tail: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptReport {
    /// 1-based attempt number.
    pub n: u32,
    pub status: Status,
    #[serde(default)]
    pub duration_ms: u64,
    pub stages: Vec<StageReport>,
    pub failure: Option<FailureReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActReport {
    /// Display name, e.g. `Login rejects bad passwords [user=alice]`.
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Redacted `Examples:` values for this execution.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub example: BTreeMap<String, String>,
    pub status: Status,
    #[serde(default)]
    pub flaky: bool,
    pub attempts: Vec<AttemptReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayReport {
    pub name: String,
    pub file: String,
    pub acts: Vec<ActReport>,
}

/// Device info from the handshake (Appendix B shows model/os_version).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfoJson {
    pub model: String,
    pub os_name: String,
    pub os_version: String,
    pub runtime: String,
    pub screen_width_px: u32,
    pub screen_height_px: u32,
    pub pixel_ratio: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceReport {
    pub name: String,
    pub platform: String,
    #[serde(default)]
    pub device_info: Option<DeviceInfoJson>,
    /// Device-level failure (prepare/connect/handshake); its acts are
    /// `aborted`.
    #[serde(default)]
    pub error: Option<ErrorJson>,
    pub plays: Vec<PlayReport>,
}

/// The complete `report.json` document (Appendix B).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    /// `report.json` schema version (2 adds stage spans and example rows).
    #[serde(default = "report_schema_version")]
    pub schema_version: u32,
    pub run_id: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub status: RunStatus,
    /// Extra field: the process exit code the CLI will return.
    pub exit_code: i32,
    pub devices: Vec<DeviceReport>,
}

fn report_schema_version() -> u32 {
    2
}

impl RunReport {
    /// Counts for the console summary: (acts passed, failed, skipped,
    /// aborted, steps failed).
    #[must_use]
    pub fn summary(&self) -> Summary {
        let mut summary = Summary::default();
        for device in &self.devices {
            for play in &device.plays {
                for act in &play.acts {
                    match act.status {
                        Status::Passed => summary.acts_passed += 1,
                        Status::Failed => summary.acts_failed += 1,
                        Status::Skipped => summary.acts_skipped += 1,
                        Status::Aborted => summary.acts_aborted += 1,
                        Status::Unverifiable => summary.acts_unverifiable += 1,
                    }
                    for attempt in &act.attempts {
                        for stage in &attempt.stages {
                            for step in &stage.steps {
                                summary.steps_total += 1;
                                if step.status == Status::Failed {
                                    summary.steps_failed += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        summary
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub acts_passed: u32,
    pub acts_failed: u32,
    pub acts_skipped: u32,
    pub acts_aborted: u32,
    pub acts_unverifiable: u32,
    pub steps_total: u32,
    pub steps_failed: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(status: Status) -> StepReport {
        StepReport {
            source: "Action: click => B".to_string(),
            span: SpanJson {
                file: "p.play".to_string(),
                line: 1,
                col: 1,
                len: 10,
            },
            status,
            duration_ms: 5,
            error: None,
        }
    }

    fn report() -> RunReport {
        RunReport {
            schema_version: 2,
            run_id: "20261016-185512-a1b2".to_string(),
            started_at: "2026-10-16T18:55:12.000Z".to_string(),
            duration_ms: 42,
            status: RunStatus::Failed,
            exit_code: 1,
            devices: vec![DeviceReport {
                name: "d".to_string(),
                platform: "android".to_string(),
                device_info: None,
                error: None,
                plays: vec![PlayReport {
                    name: "P".to_string(),
                    file: "plays/p.play".to_string(),
                    acts: vec![ActReport {
                        name: "A".to_string(),
                        tags: vec!["smoke".to_string()],
                        example: BTreeMap::new(),
                        status: Status::Failed,
                        flaky: false,
                        attempts: vec![AttemptReport {
                            n: 1,
                            status: Status::Failed,
                            duration_ms: 10,
                            stages: vec![StageReport {
                                name: "S".to_string(),
                                span: None,
                                status: Status::Failed,
                                steps: vec![step(Status::Passed), step(Status::Failed)],
                            }],
                            failure: Some(FailureReport {
                                step_index: 1,
                                code: "CHECK_FAILED".to_string(),
                                message: "check failed".to_string(),
                                details: BTreeMap::new(),
                                screenshot: None,
                                log_tail: vec![],
                            }),
                        }],
                    }],
                }],
            }],
        }
    }

    #[test]
    fn json_shape_matches_appendix_b_keys() {
        let value = serde_json::to_value(report()).expect("serializes");
        for key in ["run_id", "started_at", "duration_ms", "status", "devices"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        let act = &value["devices"][0]["plays"][0]["acts"][0];
        for key in ["name", "tags", "status", "flaky", "attempts"] {
            assert!(act.get(key).is_some(), "missing act.{key}");
        }
        let attempt = &act["attempts"][0];
        for key in ["n", "status", "duration_ms", "stages", "failure"] {
            assert!(attempt.get(key).is_some(), "missing attempt.{key}");
        }
        let failure = &attempt["failure"];
        for key in [
            "step_index",
            "code",
            "message",
            "details",
            "screenshot",
            "log_tail",
        ] {
            assert!(failure.get(key).is_some(), "missing failure.{key}");
        }
        let step = &attempt["stages"][0]["steps"][0];
        for key in ["source", "span", "status", "duration_ms", "error"] {
            assert!(step.get(key).is_some(), "missing step.{key}");
        }
    }

    #[test]
    fn summary_counts_every_status() {
        let mut report = report();
        report.devices[0].plays[0].acts[0].status = Status::Failed;
        report.devices[0].plays[0].acts.push(ActReport {
            name: "B".to_string(),
            tags: vec![],
            example: BTreeMap::new(),
            status: Status::Skipped,
            flaky: false,
            attempts: vec![],
        });
        report.devices[0].plays[0].acts.push(ActReport {
            name: "C".to_string(),
            tags: vec![],
            example: BTreeMap::new(),
            status: Status::Aborted,
            flaky: false,
            attempts: vec![],
        });
        report.devices[0].plays[0].acts.push(ActReport {
            name: "D".to_string(),
            tags: vec![],
            example: BTreeMap::new(),
            status: Status::Passed,
            flaky: false,
            attempts: vec![],
        });
        let summary = report.summary();
        assert_eq!(summary.acts_passed, 1);
        assert_eq!(summary.acts_failed, 1);
        assert_eq!(summary.acts_skipped, 1);
        assert_eq!(summary.acts_aborted, 1);
        assert_eq!(summary.steps_total, 2);
        assert_eq!(summary.steps_failed, 1);
    }
}
