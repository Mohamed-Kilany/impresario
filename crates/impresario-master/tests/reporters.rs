//! Reporter snapshots (spec 8.4): console (plain + color), junit.xml,
//! report.json and report.html rendered from one crafted, deterministic
//! [`RunReport`].

use std::collections::BTreeMap;

use base64::Engine as _;

use impresario_master::report::{
    ActReport, AttemptReport, DeviceInfoJson, DeviceReport, ErrorJson, FailureReport, PlayReport,
    RunReport, RunStatus, SpanJson, StageReport, Status, StepReport,
};
use impresario_master::reporters::{console, html, json, junit};

fn span(line: usize) -> SpanJson {
    SpanJson {
        file: "plays/login.play".to_string(),
        line,
        col: 9,
        len: 27,
    }
}

fn step(line: usize, source: &str, status: Status, duration_ms: u64) -> StepReport {
    StepReport {
        source: source.to_string(),
        span: span(line),
        status,
        duration_ms,
        error: match status {
            Status::Failed => Some(ErrorJson {
                code: "CHECK_FAILED".to_string(),
                message: "expected 'Welcome' but saw 'Invalid credentials'".to_string(),
                details: BTreeMap::from([(
                    "observed_text".to_string(),
                    "Invalid credentials".to_string(),
                )]),
            }),
            _ => None,
        },
    }
}

fn stage(name: &str, steps: Vec<StepReport>) -> StageReport {
    let status = if steps.iter().any(|step| step.status == Status::Failed) {
        Status::Failed
    } else {
        Status::Passed
    };
    StageReport {
        name: name.to_string(),
        span: Some(SpanJson {
            file: "theatres/auth.theatre".to_string(),
            line: 4,
            col: 1,
            len: 5,
        }),
        status,
        steps,
    }
}

fn failure(step_index: usize, screenshot: Option<&str>) -> FailureReport {
    FailureReport {
        step_index,
        code: "CHECK_FAILED".to_string(),
        message: "expected 'Welcome' but saw 'Invalid credentials'".to_string(),
        details: BTreeMap::from([(
            "observed_text".to_string(),
            "Invalid credentials".to_string(),
        )]),
        screenshot: screenshot.map(str::to_string),
        log_tail: vec![
            "[INFO] agent: form submitted".to_string(),
            "[ERROR] agent: login rejected".to_string(),
        ],
    }
}

fn passed_attempt() -> AttemptReport {
    AttemptReport {
        n: 1,
        status: Status::Passed,
        duration_ms: 850,
        stages: vec![stage(
            "Login",
            vec![
                step(4, "Check: email", Status::Passed, 120),
                step(5, "Action: click => submit", Status::Passed, 700),
            ],
        )],
        failure: None,
    }
}

fn report() -> RunReport {
    let failed_attempt = AttemptReport {
        n: 1,
        status: Status::Failed,
        duration_ms: 1_250,
        stages: vec![stage(
            "Login",
            vec![
                step(11, "Check: welcome", Status::Failed, 1_100),
                step(12, "Action: click => retry", Status::Skipped, 0),
            ],
        )],
        failure: Some(failure(0, Some("screenshots/pixel-8/failure-1.png"))),
    };
    let flaky_attempts = vec![
        AttemptReport {
            n: 1,
            status: Status::Failed,
            duration_ms: 400,
            stages: vec![stage(
                "Checkout",
                vec![step(3, "Check: receipt", Status::Failed, 380)],
            )],
            failure: Some(failure(0, Some("screenshots/pixel-8/missing.png"))),
        },
        AttemptReport {
            n: 2,
            status: Status::Passed,
            duration_ms: 900,
            stages: vec![stage(
                "Checkout",
                vec![
                    step(3, "Check: receipt", Status::Passed, 300),
                    step(4, "Action: click => done", Status::Passed, 500),
                ],
            )],
            failure: None,
        },
    ];

    RunReport {
        schema_version: 2,
        run_id: "20261016-120000-abcd".to_string(),
        started_at: "2026-10-16T12:00:00.000Z".to_string(),
        duration_ms: 12_345,
        status: RunStatus::Failed,
        exit_code: 1,
        devices: vec![DeviceReport {
            name: "pixel-8".to_string(),
            platform: "android".to_string(),
            device_info: Some(DeviceInfoJson {
                model: "Pixel 8".to_string(),
                os_name: "Android".to_string(),
                os_version: "14".to_string(),
                runtime: "MockAgent 0.1.0".to_string(),
                screen_width_px: 1080,
                screen_height_px: 2400,
                pixel_ratio: 2.625,
            }),
            error: None,
            plays: vec![
                PlayReport {
                    name: "Login".to_string(),
                    file: "plays/login.play".to_string(),
                    acts: vec![
                        ActReport {
                            name: "Sign in".to_string(),
                            tags: vec![],
                            example: BTreeMap::new(),
                            status: Status::Passed,
                            flaky: false,
                            attempts: vec![passed_attempt()],
                        },
                        ActReport {
                            name: "Wrong password".to_string(),
                            tags: vec![],
                            example: BTreeMap::new(),
                            status: Status::Failed,
                            flaky: false,
                            attempts: vec![failed_attempt],
                        },
                    ],
                },
                PlayReport {
                    name: "Checkout".to_string(),
                    file: "plays/checkout.play".to_string(),
                    acts: vec![
                        ActReport {
                            name: "Pay".to_string(),
                            tags: vec!["smoke".to_string()],
                            example: BTreeMap::new(),
                            status: Status::Passed,
                            flaky: true,
                            attempts: flaky_attempts,
                        },
                        ActReport {
                            name: "Refund".to_string(),
                            tags: vec![],
                            example: BTreeMap::new(),
                            status: Status::Skipped,
                            flaky: false,
                            attempts: vec![],
                        },
                    ],
                },
            ],
        }],
    }
}

#[test]
fn console_without_color() {
    insta::assert_snapshot!("console", console::render(&report(), false));
}

#[test]
fn console_with_color() {
    insta::assert_snapshot!("console_color", console::render(&report(), true));
}

#[test]
fn junit_document() {
    let xml = junit::render(&report()).expect("junit renders");
    insta::assert_snapshot!("junit", xml);
}

#[test]
fn json_document() {
    let text = json::render(&report()).expect("json renders");
    insta::assert_snapshot!("json", text);
}

#[test]
fn html_document_embeds_available_screenshots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let screenshots = temp.path().join("screenshots/pixel-8");
    std::fs::create_dir_all(&screenshots).expect("mkdir screenshots");
    std::fs::write(screenshots.join("failure-1.png"), b"PNG-DATA").expect("write png");
    let text = html::render(&report(), temp.path()).expect("html renders");

    let engine = base64::engine::general_purpose::STANDARD;
    let data_uri = format!("data:image/png;base64,{}", engine.encode(b"PNG-DATA"));
    assert!(
        text.contains(&data_uri),
        "available screenshots are embedded"
    );
    assert!(
        text.contains("screenshots/pixel-8/missing.png"),
        "a missing file degrades to its path"
    );
    insta::assert_snapshot!("html", text);
}
