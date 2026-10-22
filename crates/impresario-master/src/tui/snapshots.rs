//! Headless `TestBackend` frames (TASK-2 3.6 acceptance).
//!
//! These render the same [`super::render::draw`] used by the interactive TUI,
//! with fixed state and no child processes, so a layout regression shows up as
//! a snapshot diff instead of a manual terminal check.

use std::path::PathBuf;

use impresario_core::home::Home;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use super::app::{
    App, AppRow, ConfigRow, DeviceRow, LineStatus, TestKind, TestRow, TreeLine, WorkspaceRow,
};
use super::{Context, Focus, Screen};
use crate::jobs::{JobRecord, JobStatus};

fn context() -> Context {
    // Short, relative paths keep frames deterministic without touching disk.
    Context {
        home: Home::at("demo-home"),
        base: PathBuf::from("demo"),
        config_path: PathBuf::from("demo/impresario.config"),
        devices_path: PathBuf::from("demo/impresario.devices.config"),
        color: false,
        ascii: true,
    }
}

fn populated() -> App {
    let mut app = App::empty(context());
    app.first_run = false;
    app.overlay = None;
    app.tests = vec![
        test_row(0, "scene TestScreen", TestKind::Scene, "TestScreen", 1),
        test_row(1, "piece A", TestKind::Piece, "A", 4),
        test_row(1, "piece B", TestKind::Piece, "B", 5),
        test_row(0, "theatre Auth", TestKind::Theatre, "Auth", 1),
        test_row(1, "stage Login", TestKind::Stage, "Login", 3),
        test_row(0, "play Perform", TestKind::Play, "Perform", 1),
        test_row(
            1,
            "act LoginAct [auth, smoke]",
            TestKind::Act,
            "LoginAct",
            3,
        ),
        test_row(1, "act WebAct", TestKind::Act, "WebAct", 15),
    ];
    app.devices = vec![
        DeviceRow {
            name: "web1".to_string(),
            platform: "web".to_string(),
            connection: "launch".to_string(),
        },
        DeviceRow {
            name: "phone".to_string(),
            platform: "android".to_string(),
            connection: "adb".to_string(),
        },
    ];
    app.apps = vec![AppRow {
        name: "shop".to_string(),
        description: "Storefront".to_string(),
        platforms: "web, android".to_string(),
    }];
    app.config_rows = vec![
        ConfigRow {
            key: "timeout_default_ms".to_string(),
            value: "5000".to_string(),
            source: "workspace".to_string(),
        },
        ConfigRow {
            key: "parallel_devices".to_string(),
            value: "2".to_string(),
            source: "workspace".to_string(),
        },
    ];
    app.activity = vec![
        "Welcome. Type a command, or /help for keys and examples.".to_string(),
        "devices: 2 configured".to_string(),
        "tests: 1 scene(s), 1 theatre(s), 1 play(s)".to_string(),
        "run 20261021-120000-run01 submitted".to_string(),
    ]
    .into();
    app.screen = Screen::Home;
    app.workspaces = vec![
        WorkspaceRow {
            path: PathBuf::from("demo"),
            name: "demo".to_string(),
            current: true,
            missing: false,
        },
        WorkspaceRow {
            path: PathBuf::from("shop"),
            name: "shop".to_string(),
            current: false,
            missing: false,
        },
        WorkspaceRow {
            path: PathBuf::from("archive"),
            name: "archive".to_string(),
            current: false,
            missing: true,
        },
    ];
    app.jobs = vec![finished_job()];
    app
}

fn finished_job() -> JobRecord {
    JobRecord {
        schema_version: 1,
        id: "20261021-114500-run00".to_string(),
        status: JobStatus::Passed,
        detached: true,
        created_at: "2026-10-21T11:45:00Z".to_string(),
        started_at: Some("2026-10-21T11:45:01Z".to_string()),
        finished_at: Some("2026-10-21T11:45:13Z".to_string()),
        duration_ms: Some(12_000),
        exit_code: Some(0),
        workspace: "demo".to_string(),
        devices: vec!["web1".to_string()],
        plays: vec!["Perform".to_string()],
        label: "1 play(s) x 1 device(s)".to_string(),
        pid: Some(1),
        queue_position: None,
        run_dir: "demo/reports/20261021-114500-run00".to_string(),
        message: Some("run passed (1 play, 1 device)".to_string()),
    }
}

fn test_row(indent: usize, label: &str, kind: TestKind, name: &str, line: usize) -> TestRow {
    TestRow {
        indent,
        label: label.to_string(),
        kind,
        file: Some(PathBuf::from(format!("demo/{}", file_for(kind)))),
        line: Some(line),
        name: name.to_string(),
    }
}

fn file_for(kind: TestKind) -> &'static str {
    match kind {
        TestKind::Scene | TestKind::Piece => "scenes/TestScreen.scene",
        TestKind::Theatre | TestKind::Stage => "theatres/auth.theatre",
        TestKind::Play | TestKind::Act => "plays/perform.play",
    }
}

fn running_job() -> JobRecord {
    JobRecord {
        schema_version: 1,
        id: "20261021-120000-run01".to_string(),
        status: JobStatus::Running,
        detached: true,
        created_at: "2026-10-21T12:00:00Z".to_string(),
        started_at: Some("2026-10-21T12:00:01Z".to_string()),
        finished_at: None,
        duration_ms: None,
        exit_code: None,
        workspace: "demo".to_string(),
        devices: vec!["web1".to_string(), "phone".to_string()],
        plays: vec!["Perform".to_string()],
        label: "1 play(s) x 2 device(s)".to_string(),
        pid: Some(1),
        queue_position: None,
        run_dir: "demo/reports/20261021-120000-run01".to_string(),
        message: None,
    }
}

fn render(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| super::render::draw(frame, app))
        .expect("frame draws");
    let buffer = terminal.backend().buffer();
    let mut lines = Vec::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            if let Some(cell) = buffer.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

#[test]
fn home_dashboard_frame() {
    insta::assert_snapshot!("home_dashboard", render(&populated(), 110, 30));
}

#[test]
fn run_view_following_frame() {
    let mut app = populated();
    app.screen = Screen::Runs;
    app.focus = Focus::Content;
    app.jobs = vec![running_job()];
    app.job_index = 0;
    app.follow = Some("20261021-120000-run01".to_string());
    app.follow_started = Some(std::time::Instant::now());
    app.current_command = "Action: click => B".to_string();
    app.tree = vec![
        tree_line(0, "web1  session started", LineStatus::Running),
        tree_line(0, "web1  act Pass started", LineStatus::Running),
        tree_line(1, "web1  step 1/3 check A", LineStatus::Passed),
        tree_line(1, "web1  step 2/3 click B", LineStatus::Passed),
        tree_line(1, "web1  step 3/3 type Input", LineStatus::Running),
        tree_line(0, "phone  session started", LineStatus::Running),
        tree_line(0, "phone  act Pass started", LineStatus::Running),
    ];
    app.logs = vec![
        "[web1] validating A".to_string(),
        "[web1] click B ok".to_string(),
        "[phone] connecting to id:a".to_string(),
        "[phone] waiting for the keyboard".to_string(),
    ]
    .into();
    app.log_follow = true;
    app.device_filter = Some("web1".to_string());
    insta::assert_snapshot!("run_view_following", render(&app, 120, 32));
}

#[test]
fn tests_explorer_frame() {
    let mut app = populated();
    app.screen = Screen::Tests;
    app.focus = Focus::Content;
    app.test_index = 6;
    app.test_marks.insert(0);
    app.test_marks.insert(6);
    insta::assert_snapshot!("tests_explorer", render(&app, 120, 30));
}

#[test]
fn narrow_home_frame() {
    insta::assert_snapshot!("home_dashboard_narrow", render(&populated(), 80, 24));
}

#[test]
fn narrow_tests_explorer_frame() {
    let mut app = populated();
    app.screen = Screen::Tests;
    app.focus = Focus::Content;
    app.test_index = 6;
    app.test_marks.insert(0);
    app.test_marks.insert(6);
    insta::assert_snapshot!("tests_explorer_narrow", render(&app, 80, 24));
}

#[test]
fn landing_frame() {
    let mut app = populated();
    app.screen = Screen::Landing;
    app.focus = Focus::Content;
    app.landing_index = 0;
    insta::assert_snapshot!("landing", render(&app, 110, 30));
}

#[test]
fn narrow_landing_frame() {
    let mut app = populated();
    app.screen = Screen::Landing;
    app.focus = Focus::Content;
    app.landing_index = 1;
    insta::assert_snapshot!("landing_narrow", render(&app, 80, 24));
}

#[test]
fn workspace_menu_frame() {
    let mut app = populated();
    app.screen = Screen::Menu;
    app.focus = Focus::Content;
    app.menu_index = 1;
    insta::assert_snapshot!("workspace_menu", render(&app, 100, 28));
}

#[test]
fn filter_and_help_overlay_frame() {
    let mut app = populated();
    app.screen = Screen::Runs;
    app.jobs = vec![running_job()];
    app.filter = "phone".to_string();
    app.overlay = Some(super::app::help_text());
    insta::assert_snapshot!("filter_and_help_overlay", render(&app, 100, 34));
}

#[test]
fn first_run_overlay_frame() {
    let mut app = App::empty(context());
    assert!(app.first_run);
    app.screen = Screen::Menu;
    app.focus = Focus::Content;
    app.first_run_prompt = true;
    app.overlay = Some(super::app::first_run_text(&app.ctx));
    insta::assert_snapshot!("first_run_overlay", render(&app, 90, 26));
}

#[test]
fn small_terminal_frame() {
    let app = populated();
    insta::assert_snapshot!("small_terminal", render(&app, 30, 8));
}

fn tree_line(indent: usize, text: &str, status: LineStatus) -> TreeLine {
    TreeLine {
        indent,
        text: text.to_string(),
        status,
    }
}
