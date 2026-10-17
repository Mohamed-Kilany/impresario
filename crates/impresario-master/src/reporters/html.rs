//! The html reporter (spec 8.4): `report.html`, self-contained (inline CSS,
//! screenshots embedded as base64). Screenshots are read back from the run
//! directory written by the executor; a missing file degrades to its path.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;

use crate::report::{
    ActReport, AttemptReport, DeviceReport, FailureReport, PlayReport, RunReport, Status,
    StepReport,
};

use super::{act_duration, failing_step, format_duration};

const CSS: &str = "\
<style>
:root { color-scheme: light dark; }
body { font: 14px/1.5 system-ui, sans-serif; margin: 2rem auto; max-width: 70rem; padding: 0 1rem; }
h1 { font-size: 1.4rem; } h2 { font-size: 1.15rem; margin-top: 1.5rem; border-bottom: 1px solid #8884; padding-bottom: .2rem; }
h3 { font-size: 1rem; margin: 1rem 0 .4rem; } h4, h5 { font-size: .95rem; margin: .8rem 0 .3rem; }
code, pre { font-family: ui-monospace, monospace; font-size: .9em; }
.meta { color: #888; } .dim { color: #888; }
.badge { display: inline-block; padding: 0 .5em; border-radius: .6em; font-size: .8em; font-weight: 600; }
.passed { background: #1a7f3722; color: #1a7f37; } .failed { background: #cf222e22; color: #cf222e; }
.skipped { background: #9a670022; color: #9a6700; } .aborted { background: #8250df22; color: #8250df; }
.run-passed { background: #1a7f37; color: #fff; } .run-failed, .run-error { background: #cf222e; color: #fff; }
.run-interrupted { background: #8250df; color: #fff; }
table { border-collapse: collapse; width: 100%; margin: .3rem 0 .8rem; }
th, td { text-align: left; border-bottom: 1px solid #8884; padding: .2rem .5rem; vertical-align: top; }
details.act { margin: .3rem 0 .3rem 1rem; } details.act > summary { cursor: pointer; }
.attempt { margin: .4rem 0 .4rem 1rem; } .stage { margin-left: 1rem; }
.failure { border: 1px solid #cf222e55; border-radius: .4rem; padding: .5rem .8rem; margin: .5rem 0 .5rem 1rem; }
.device-error { border: 1px solid #cf222e55; border-radius: .4rem; padding: .5rem .8rem; }
img.screenshot { max-width: 100%; border: 1px solid #8884; border-radius: .4rem; }
dl.details { display: grid; grid-template-columns: max-content 1fr; gap: 0 .8rem; margin: .3rem 0; }
dl.details dt { font-weight: 600; } dl.details dd { margin: 0; }
footer { color: #888; margin-top: 2rem; font-size: .85em; }
</style>
";

/// Render the complete `report.html` document.
pub fn render(report: &RunReport, run_dir: &Path) -> Result<String, String> {
    let mut html = String::with_capacity(16 * 1024);
    html.push_str(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n",
    );
    html.push_str(&format!(
        "<title>impresario run {}</title>\n",
        escape(&report.run_id)
    ));
    html.push_str(CSS);
    html.push_str("</head>\n<body>\n");
    html.push_str(&format!(
        "<h1>impresario run <code>{}</code> <span class=\"badge run-{}\">{}</span></h1>\n",
        escape(&report.run_id),
        report.status.as_str(),
        report.status.as_str()
    ));
    html.push_str(&format!(
        "<p class=\"meta\">started {} · {} · exit code {}</p>\n",
        escape(&report.started_at),
        format_duration(report.duration_ms),
        report.exit_code
    ));
    for device in &report.devices {
        write_device(&mut html, device, run_dir);
    }
    let summary = report.summary();
    html.push_str(&format!(
        "<footer>{} passed · {} failed · {} skipped · {} aborted acts · {} steps ({} failed)</footer>\n",
        summary.acts_passed,
        summary.acts_failed,
        summary.acts_skipped,
        summary.acts_aborted,
        summary.steps_total,
        summary.steps_failed
    ));
    html.push_str("</body>\n</html>\n");
    Ok(html)
}

fn write_device(html: &mut String, device: &DeviceReport, run_dir: &Path) {
    html.push_str("<section class=\"device\">\n");
    html.push_str(&format!(
        "<h2>{} <span class=\"dim\">· {}</span></h2>\n",
        escape(&device.name),
        escape(&device.platform)
    ));
    if let Some(info) = &device.device_info {
        html.push_str(&format!(
            "<p class=\"meta\">{} · {} {} · {}x{} px @{}x</p>\n",
            escape(&info.model),
            escape(&info.os_name),
            escape(&info.os_version),
            info.screen_width_px,
            info.screen_height_px,
            info.pixel_ratio
        ));
    }
    if let Some(error) = &device.error {
        html.push_str(&format!(
            "<div class=\"device-error\"><span class=\"badge failed\">device error</span> <code>{}</code>: {}{}</div>\n",
            escape(&error.code),
            escape(&error.message),
            details_dl(&error.details)
        ));
    }
    for play in &device.plays {
        write_play(html, play, run_dir);
    }
    html.push_str("</section>\n");
}

fn write_play(html: &mut String, play: &PlayReport, run_dir: &Path) {
    html.push_str("<section class=\"play\">\n");
    html.push_str(&format!(
        "<h3>{} <span class=\"dim\">({})</span></h3>\n",
        escape(&play.name),
        escape(&play.file)
    ));
    for act in &play.acts {
        write_act(html, act, run_dir);
    }
    html.push_str("</section>\n");
}

fn write_act(html: &mut String, act: &ActReport, run_dir: &Path) {
    let open = if act.status == Status::Passed {
        ""
    } else {
        " open"
    };
    let flaky = if act.flaky {
        " <span class=\"badge skipped\">flaky</span>"
    } else {
        ""
    };
    html.push_str(&format!(
        "<details class=\"act\"{open}><summary>{} {} <span class=\"dim\">{}</span>{}</summary>\n",
        badge(act.status),
        escape(&act.name),
        escape(&duration_of(act_duration(act), act.attempts.len())),
        flaky
    ));
    if !act.tags.is_empty() {
        html.push_str(&format!(
            "<p class=\"meta\">tags: {}</p>\n",
            escape(&act.tags.join(", "))
        ));
    }
    let multiple = act.attempts.len() > 1;
    for attempt in &act.attempts {
        write_attempt(html, attempt, multiple, run_dir);
    }
    html.push_str("</details>\n");
}

fn write_attempt(html: &mut String, attempt: &AttemptReport, multiple: bool, run_dir: &Path) {
    if multiple {
        html.push_str(&format!(
            "<div class=\"attempt\">{} <span class=\"dim\">attempt {} · {}</span></div>\n",
            badge(attempt.status),
            attempt.n,
            format_duration(attempt.duration_ms)
        ));
    }
    for stage in &attempt.stages {
        html.push_str(&format!(
            "<div class=\"stage\"><h5>{} Stage: {}</h5>\n",
            badge(stage.status),
            escape(&stage.name)
        ));
        if !stage.steps.is_empty() {
            html.push_str(
                "<table><tr><th>step</th><th>status</th><th>time</th><th>error</th></tr>\n",
            );
            for step in &stage.steps {
                write_step_row(html, step);
            }
            html.push_str("</table>\n");
        }
        html.push_str("</div>\n");
    }
    if let Some(failure) = &attempt.failure {
        write_failure(html, attempt, failure, run_dir);
    }
}

fn write_step_row(html: &mut String, step: &StepReport) {
    let error = match &step.error {
        Some(error) => format!("{}: {}", escape(&error.code), escape(&error.message)),
        None => String::new(),
    };
    html.push_str(&format!(
        "<tr class=\"{}\"><td><code>{}</code><br><span class=\"dim\">{}:{}:{}</span></td>\
         <td>{}</td><td>{}</td><td>{}</td></tr>\n",
        step.status.as_str(),
        escape(&step.source),
        escape(&step.span.file),
        step.span.line,
        step.span.col,
        badge(step.status),
        if step.duration_ms == 0 {
            String::new()
        } else {
            format_duration(step.duration_ms)
        },
        error
    ));
}

fn write_failure(
    html: &mut String,
    attempt: &AttemptReport,
    failure: &FailureReport,
    run_dir: &Path,
) {
    html.push_str("<div class=\"failure\">\n");
    html.push_str(&format!(
        "<p><span class=\"badge failed\">{}</span> {}</p>\n",
        escape(&failure.code),
        escape(&failure.message)
    ));
    let mut where_ = format!("step {}", failure.step_index + 1);
    if let Some(step) = failing_step(attempt, failure.step_index) {
        where_.push_str(&format!(
            " · {}:{}:{}",
            step.span.file, step.span.line, step.span.col
        ));
    }
    html.push_str(&format!("<p class=\"meta\">{}</p>\n", escape(&where_)));
    if !failure.details.is_empty() {
        html.push_str(&details_dl(&failure.details));
    }
    if let Some(screenshot) = &failure.screenshot {
        match embed_screenshot(run_dir, screenshot) {
            Some(src) => html.push_str(&format!(
                "<img class=\"screenshot\" alt=\"{}\" src=\"{}\">\n",
                escape(screenshot),
                src
            )),
            None => html.push_str(&format!(
                "<p class=\"meta\">screenshot: {}</p>\n",
                escape(screenshot)
            )),
        }
    }
    if !failure.log_tail.is_empty() {
        let lines = failure
            .log_tail
            .iter()
            .map(|line| escape(line))
            .collect::<Vec<_>>()
            .join("\n");
        html.push_str(&format!(
            "<details><summary>{} log entries</summary><pre>{}</pre></details>\n",
            failure.log_tail.len(),
            lines
        ));
    }
    html.push_str("</div>\n");
}

/// Read a report-relative screenshot and inline it as a data URL.
fn embed_screenshot(run_dir: &Path, relative: &str) -> Option<String> {
    let path = run_dir.join(relative);
    let bytes = std::fs::read(path).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:image/png;base64,{encoded}"))
}

fn badge(status: Status) -> String {
    format!(
        "<span class=\"badge {}\">{}</span>",
        status.as_str(),
        status.as_str()
    )
}

fn duration_of(ms: u64, attempts: usize) -> String {
    if attempts > 1 {
        format!("{}, {} attempts", format_duration(ms), attempts)
    } else {
        format_duration(ms)
    }
}

fn details_dl(details: &BTreeMap<String, String>) -> String {
    if details.is_empty() {
        return String::new();
    }
    let mut html = String::from("<dl class=\"details\">");
    for (key, value) in details {
        html.push_str(&format!(
            "<dt>{}</dt><dd>{}</dd>",
            escape(key),
            escape(value)
        ));
    }
    html.push_str("</dl>");
    html
}

/// Escape `&`, `<`, `>`, `"` and `'` (all dynamic text goes through this).
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{DeviceReport, RunReport, RunStatus};
    use std::collections::BTreeMap;

    #[test]
    fn escape_handles_markup() {
        assert_eq!(escape("<b>&\"'</b>"), "&lt;b&gt;&amp;&quot;&#39;&lt;/b&gt;");
    }

    #[test]
    fn html_embeds_screenshots_and_escapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let screenshots = dir.path().join("screenshots").join("d");
        std::fs::create_dir_all(&screenshots).expect("mkdir");
        std::fs::write(screenshots.join("failure-1.png"), b"png-bytes").expect("write");

        let report = RunReport {
            run_id: "20261016-185512-a1b2".to_string(),
            started_at: "2026-10-16T18:55:12.000Z".to_string(),
            duration_ms: 10,
            status: RunStatus::Failed,
            exit_code: 1,
            devices: vec![DeviceReport {
                name: "d <x>".to_string(),
                platform: "android".to_string(),
                device_info: None,
                error: None,
                plays: vec![PlayReport {
                    name: "P".to_string(),
                    file: "plays/p.play".to_string(),
                    acts: vec![ActReport {
                        name: "A".to_string(),
                        tags: vec![],
                        status: Status::Failed,
                        flaky: false,
                        attempts: vec![AttemptReport {
                            n: 1,
                            status: Status::Failed,
                            duration_ms: 10,
                            stages: vec![],
                            failure: Some(FailureReport {
                                step_index: 0,
                                code: "CHECK_FAILED".to_string(),
                                message: "it's broken".to_string(),
                                details: BTreeMap::new(),
                                screenshot: Some("screenshots/d/failure-1.png".to_string()),
                                log_tail: vec!["log <line>".to_string()],
                            }),
                        }],
                    }],
                }],
            }],
        };
        let html = render(&report, dir.path()).expect("renders");
        assert!(
            html.contains("<title>impresario run 20261016-185512-a1b2</title>"),
            "{html}"
        );
        assert!(html.contains("d &lt;x&gt;"), "{html}");
        assert!(html.contains("it&#39;s broken"), "{html}");
        assert!(html.contains("data:image/png;base64,"), "{html}");
        let expected = base64::engine::general_purpose::STANDARD.encode(b"png-bytes");
        assert!(html.contains(&expected), "{html}");
        assert!(html.contains("log &lt;line&gt;"), "{html}");
        assert!(!html.contains("<x>"), "{html}");
    }

    #[test]
    fn missing_screenshot_degrades_to_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(embed_screenshot(dir.path(), "screenshots/d/nope.png").is_none());
    }
}
