//! The json reporter (spec 8.4): `report.json`, Appendix B schema.
//!
//! The document is the serialized [`RunReport`]; the extra `exit_code` field
//! is documented in decision I-28.

use crate::report::RunReport;

/// Render the complete `report.json` document.
pub fn render(report: &RunReport) -> Result<String, String> {
    serde_json::to_string_pretty(report)
        .map_err(|err| format!("json: cannot serialize the report: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{ActReport, AttemptReport, DeviceReport, PlayReport, RunStatus, Status};

    #[test]
    fn json_round_trips_and_keeps_appendix_b_keys() {
        let report = RunReport {
            schema_version: 2,
            run_id: "20261016-185512-a1b2".to_string(),
            started_at: "2026-10-16T18:55:12.000Z".to_string(),
            duration_ms: 42,
            status: RunStatus::Passed,
            exit_code: 0,
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
                        tags: vec![],
                        example: std::collections::BTreeMap::new(),
                        status: Status::Passed,
                        flaky: false,
                        attempts: vec![AttemptReport {
                            n: 1,
                            status: Status::Passed,
                            duration_ms: 5,
                            stages: vec![],
                            failure: None,
                        }],
                    }],
                }],
            }],
        };
        let text = render(&report).expect("renders");
        let value: serde_json::Value = serde_json::from_str(&text).expect("parses");
        assert_eq!(value["run_id"], "20261016-185512-a1b2");
        assert_eq!(value["status"], "passed");
        assert_eq!(
            value["devices"][0]["plays"][0]["acts"][0]["attempts"][0]["n"],
            1
        );
        // Round-trips back into the model.
        let parsed: RunReport = serde_json::from_str(&text).expect("deserializes");
        assert_eq!(parsed, report);
    }
}
