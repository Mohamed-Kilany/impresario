//! The console reporter (spec 8.4): a colored tree
//! `Play > Act > Stage > step` with durations, step source text, failures
//! with code/message, and a final summary.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write as _};

use crate::report::{
    ActReport, AttemptReport, DeviceReport, FailureReport, RunReport, RunStatus, Status, StepReport,
};

use super::{act_duration, failing_step, format_duration};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "1";
const DIM: &str = "2";
const GREEN: &str = "32";
const RED: &str = "31";
const YELLOW: &str = "33";
const MAGENTA: &str = "35";

#[derive(Clone, Copy)]
struct Style {
    color: bool,
}

impl Style {
    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    fn mark(self, status: Status) -> String {
        let (glyph, color) = match status {
            Status::Passed => ("✔", GREEN),
            Status::Failed => ("✖", RED),
            Status::Skipped => ("○", YELLOW),
            Status::Aborted => ("⊘", MAGENTA),
        };
        self.paint(color, glyph)
    }
}

/// Print the tree and the summary to stdout.
pub fn print(report: &RunReport, color: bool) -> io::Result<()> {
    let text = render(report, color);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// Render the tree and the summary (snapshot-testable, no I/O).
#[must_use]
pub fn render(report: &RunReport, color: bool) -> String {
    let style = Style { color };
    let mut out = String::new();
    for device in &report.devices {
        let _ = write_device(&mut out, style, device);
    }
    let _ = write_summary(&mut out, style, report);
    out
}

fn write_device(out: &mut impl fmt::Write, style: Style, device: &DeviceReport) -> fmt::Result {
    writeln!(
        out,
        "{}",
        style.paint(BOLD, &format!("▶ {} · {}", device.name, device.platform))
    )?;
    if let Some(info) = &device.device_info {
        writeln!(
            out,
            "  {}",
            style.paint(
                DIM,
                &format!(
                    "{} · {} {} · {}x{} @{:.2}x",
                    info.model,
                    info.os_name,
                    info.os_version,
                    info.screen_width_px,
                    info.screen_height_px,
                    info.pixel_ratio
                )
            )
        )?;
    }
    if let Some(error) = &device.error {
        writeln!(
            out,
            "  {} {}",
            style.mark(Status::Failed),
            style.paint(
                RED,
                &format!(
                    "{}: {}{}",
                    error.code,
                    error.message,
                    details_suffix(&error.details)
                )
            )
        )?;
    }
    for play in &device.plays {
        writeln!(
            out,
            "  {} {} {}",
            style.paint(DIM, "Play:"),
            play.name,
            style.paint(DIM, &format!("({})", play.file))
        )?;
        for act in &play.acts {
            write_act(out, style, act)?;
        }
    }
    Ok(())
}

fn write_act(out: &mut impl fmt::Write, style: Style, act: &ActReport) -> fmt::Result {
    let mut meta = vec![format_duration(act_duration(act))];
    if act.attempts.len() > 1 {
        meta.push(format!("{} attempts", act.attempts.len()));
    }
    if act.flaky {
        meta.push("flaky".to_string());
    }
    writeln!(
        out,
        "    {} {} {} {}",
        style.mark(act.status),
        style.paint(DIM, "Act:"),
        act.name,
        style.paint(DIM, &format!("({})", meta.join(", ")))
    )?;

    let multiple = act.attempts.len() > 1;
    for attempt in &act.attempts {
        if multiple {
            writeln!(
                out,
                "      {} {} {}",
                style.mark(attempt.status),
                style.paint(DIM, &format!("Attempt {}:", attempt.n)),
                style.paint(DIM, &format!("({})", format_duration(attempt.duration_ms)))
            )?;
        }
        write_attempt(out, style, attempt)?;
    }
    Ok(())
}

fn write_attempt(out: &mut impl fmt::Write, style: Style, attempt: &AttemptReport) -> fmt::Result {
    for stage in &attempt.stages {
        let source = stage
            .span
            .as_ref()
            .map(|span| format!(" ({}:{}:{})", span.file, span.line, span.col))
            .unwrap_or_default();
        writeln!(
            out,
            "      {} {}",
            style.mark(stage.status),
            style.paint(DIM, &format!("Stage: {}{source}", stage.name))
        )?;
        for step in &stage.steps {
            write_step(out, style, step)?;
        }
    }
    if let Some(failure) = &attempt.failure {
        write_failure(out, style, attempt, failure)?;
    }
    Ok(())
}

fn write_step(out: &mut impl fmt::Write, style: Style, step: &StepReport) -> fmt::Result {
    let duration = if step.duration_ms == 0 {
        String::new()
    } else {
        format!(" ({})", format_duration(step.duration_ms))
    };
    writeln!(
        out,
        "        {} {}{}",
        style.mark(step.status),
        step.source,
        style.paint(DIM, &duration)
    )
}

fn write_failure(
    out: &mut impl fmt::Write,
    style: Style,
    attempt: &AttemptReport,
    failure: &FailureReport,
) -> fmt::Result {
    let mut location = format!("step {}", failure.step_index + 1);
    if let Some(step) = failing_step(attempt, failure.step_index) {
        location.push_str(&format!(
            ", {}:{}:{}",
            step.span.file, step.span.line, step.span.col
        ));
    }
    writeln!(
        out,
        "      {} {}",
        style.mark(Status::Failed),
        style.paint(RED, &format!("{}: {}", failure.code, failure.message))
    )?;
    writeln!(
        out,
        "        {}",
        style.paint(DIM, &format!("({location})"))
    )?;
    if !failure.details.is_empty() {
        writeln!(
            out,
            "        {}",
            style.paint(
                DIM,
                &format!("details:{}", details_suffix(&failure.details))
            )
        )?;
    }
    if let Some(screenshot) = &failure.screenshot {
        writeln!(
            out,
            "        {}",
            style.paint(DIM, &format!("screenshot: {screenshot}"))
        )?;
    }
    if !failure.log_tail.is_empty() {
        writeln!(
            out,
            "        {}",
            style.paint(
                DIM,
                &format!("{} log entries attached", failure.log_tail.len())
            )
        )?;
    }
    Ok(())
}

fn write_summary(out: &mut impl fmt::Write, style: Style, report: &RunReport) -> fmt::Result {
    let summary = report.summary();
    let status_color = match report.status {
        RunStatus::Passed => GREEN,
        RunStatus::Failed | RunStatus::Error => RED,
        RunStatus::Interrupted => MAGENTA,
    };
    let status = format!(
        "{} ({}, exit {})",
        report.status.as_str(),
        format_duration(report.duration_ms),
        report.exit_code
    );
    writeln!(
        out,
        "{} {}",
        style.paint(BOLD, &format!("run {}:", report.run_id)),
        style.paint(status_color, &status)
    )?;

    let devices = report.devices.len();
    let plays: usize = report.devices.iter().map(|device| device.plays.len()).sum();
    writeln!(
        out,
        "{} · {} · {} passed, {} failed, {} skipped, {} aborted · {} steps, {} failed",
        plural(devices, "device"),
        plural(plays, "play"),
        summary.acts_passed,
        summary.acts_failed,
        summary.acts_skipped,
        summary.acts_aborted,
        summary.steps_total,
        summary.steps_failed
    )?;
    Ok(())
}

fn details_suffix(details: &BTreeMap<String, String>) -> String {
    if details.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = details
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!(" ({})", parts.join(", "))
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}
