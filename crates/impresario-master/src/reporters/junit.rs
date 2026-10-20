//! The junit reporter (spec 8.4): `junit.xml` with one `testsuite` per
//! device+play and one `testcase` per Act; the attempt count travels in a
//! property. Failed attempts are failures, aborted acts are errors and
//! skipped acts are skipped; nothing is silently dropped.
//!
//! `timestamp` attributes are omitted (quick-junit wants a `chrono` value
//! and no timezone crate is pinned); `started_at` is in `report.json`
//! (decision I-29).

use std::time::Duration;

use quick_junit::{NonSuccessKind, Property, Report, TestCase, TestCaseStatus, TestSuite};

use crate::report::{ActReport, PlayReport, RunReport, Status};

use super::act_duration;

/// Render the complete `junit.xml` document.
pub fn render(report: &RunReport) -> Result<String, String> {
    let mut junit = Report::new(format!("impresario {}", report.run_id));
    junit.set_time(Duration::from_millis(report.duration_ms));
    for device in &report.devices {
        for play in &device.plays {
            let mut suite = TestSuite::new(format!("{} · {}", device.name, play.name));
            suite.set_time(Duration::from_millis(play_duration(play)));
            suite.set_system_out(play.file.clone());
            for act in &play.acts {
                suite.add_test_case(test_case(play, act));
            }
            junit.add_test_suite(suite);
        }
    }
    let mut buffer = Vec::new();
    junit
        .serialize(&mut buffer)
        .map_err(|err| format!("junit: cannot serialize: {err}"))?;
    String::from_utf8(buffer).map_err(|err| format!("junit: output is not UTF-8: {err}"))
}

fn test_case(play: &PlayReport, act: &ActReport) -> TestCase {
    let mut status = match act.status {
        Status::Passed => TestCaseStatus::success(),
        Status::Failed => TestCaseStatus::non_success(NonSuccessKind::Failure),
        Status::Aborted => TestCaseStatus::non_success(NonSuccessKind::Error),
        Status::Skipped => TestCaseStatus::skipped(),
    };
    match act.status {
        Status::Passed => {}
        Status::Failed => {
            if let Some(failure) = last_failure(act) {
                status.set_message(format!("{}: {}", failure.code, failure.message));
                status.set_type(failure.code.clone());
            }
        }
        Status::Aborted => {
            status.set_message("aborted");
        }
        Status::Skipped => {
            status.set_message("not run");
        }
    }

    let mut case = TestCase::new(act.name.clone(), status);
    case.set_time(Duration::from_millis(act_duration(act)));
    case.set_classname(play.file.clone());
    case.add_property(Property::new("attempts", act.attempts.len().to_string()));
    if act.flaky {
        case.add_property(Property::new("flaky", "true"));
    }
    if !act.tags.is_empty() {
        case.add_property(Property::new("tags", act.tags.join(",")));
    }
    // The JUnit schema has no tree below the testcase. Expose the source
    // location of every stage of the first attempt as one property so a
    // failure still points back to the theatre declaration.
    if let Some(attempt) = act.attempts.first() {
        let mut seen = std::collections::BTreeSet::new();
        let mut sources = Vec::new();
        for stage in &attempt.stages {
            let Some(span) = stage.span.as_ref() else {
                continue;
            };
            let source = format!("{}@{}:{}:{}", stage.name, span.file, span.line, span.col);
            if seen.insert(source.clone()) {
                sources.push(source);
            }
        }
        if !sources.is_empty() {
            case.add_property(Property::new("stage_sources", sources.join("; ")));
        }
    }
    case
}

fn last_failure(act: &ActReport) -> Option<&crate::report::FailureReport> {
    act.attempts
        .iter()
        .rev()
        .find_map(|attempt| attempt.failure.as_ref())
}

fn play_duration(play: &PlayReport) -> u64 {
    play.acts.iter().map(act_duration).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{ActReport, AttemptReport, FailureReport, PlayReport};
    use std::collections::BTreeMap;

    fn act(status: Status) -> ActReport {
        ActReport {
            name: "A".to_string(),
            tags: vec!["smoke".to_string()],
            example: BTreeMap::new(),
            status,
            flaky: false,
            attempts: vec![AttemptReport {
                n: 1,
                status,
                duration_ms: 10,
                stages: vec![],
                failure: if status == Status::Failed {
                    Some(FailureReport {
                        step_index: 0,
                        code: "CHECK_FAILED".to_string(),
                        message: "nope".to_string(),
                        details: BTreeMap::new(),
                        screenshot: None,
                        log_tail: vec![],
                    })
                } else {
                    None
                },
            }],
        }
    }

    fn render_one(status: Status) -> String {
        let play = PlayReport {
            name: "P".to_string(),
            file: "plays/p.play".to_string(),
            acts: vec![act(status)],
        };
        let report = RunReport {
            schema_version: 2,
            run_id: "20261016-185512-a1b2".to_string(),
            started_at: "2026-10-16T18:55:12.000Z".to_string(),
            duration_ms: 10,
            status: crate::report::RunStatus::Failed,
            exit_code: 1,
            devices: vec![crate::report::DeviceReport {
                name: "d".to_string(),
                platform: "android".to_string(),
                device_info: None,
                error: None,
                plays: vec![play],
            }],
        };
        render(&report).expect("renders")
    }

    #[test]
    fn testcase_statuses_map_to_junit() {
        let passed = render_one(Status::Passed);
        assert!(passed.contains(r#"<testcase name="A""#), "{passed}");
        assert!(!passed.contains("<failure"), "{passed}");

        let failed = render_one(Status::Failed);
        assert!(failed.contains("<failure"), "{failed}");
        assert!(failed.contains("CHECK_FAILED: nope"), "{failed}");

        let aborted = render_one(Status::Aborted);
        assert!(aborted.contains("<error"), "{aborted}");

        let skipped = render_one(Status::Skipped);
        assert!(skipped.contains("<skipped"), "{skipped}");
    }

    #[test]
    fn attempts_are_a_property_and_the_suite_is_per_device_play() {
        let xml = render_one(Status::Passed);
        assert!(
            xml.contains(r#"<property name="attempts" value="1"/>"#),
            "{xml}"
        );
        assert!(xml.contains(r#"<testsuite name="d · P""#), "{xml}");
    }
}
