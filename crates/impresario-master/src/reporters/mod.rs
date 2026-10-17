//! Reporters (spec 8.4): console, junit, json, html.
//!
//! The executor has already written screenshots under
//! `<run_dir>/screenshots/<device>/`; these renderers only read the finished
//! [`RunReport`]. All strings in the report are already redacted (`5.6`).

pub mod console;
pub mod html;
pub mod json;
pub mod junit;

use std::path::Path;

use impresario_core::config::Reporter;

use crate::report::RunReport;

/// Parse a `--reporter` comma list (spec 8.1); unknown names are errors.
/// Duplicates collapse; an empty list is an error.
pub fn parse_list(spec: &str) -> Result<Vec<Reporter>, String> {
    let mut reporters: Vec<Reporter> = Vec::new();
    for name in spec.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let reporter = match name {
            "console" => Reporter::Console,
            "junit" => Reporter::Junit,
            "json" => Reporter::Json,
            "html" => Reporter::Html,
            other => {
                return Err(format!(
                    "unknown reporter '{other}' (expected console, junit, json, html)"
                ));
            }
        };
        if !reporters.contains(&reporter) {
            reporters.push(reporter);
        }
    }
    if reporters.is_empty() {
        return Err("--reporter needs at least one name (console, junit, json, html)".to_string());
    }
    Ok(reporters)
}

/// Render the selected reporters. The console goes to stdout, junit/json/html
/// are written into `run_dir`. Returns one message per reporter that failed;
/// the run itself is already over, so failures never panic.
#[must_use]
pub fn write(
    report: &RunReport,
    run_dir: &Path,
    reporters: &[Reporter],
    color: bool,
) -> Vec<String> {
    let mut errors = Vec::new();
    for &reporter in reporters {
        let result = match reporter {
            Reporter::Console => {
                console::print(report, color).map_err(|err| format!("console: {err}"))
            }
            Reporter::Junit => junit::render(report)
                .and_then(|xml| write_file(run_dir, "junit.xml", xml.as_bytes())),
            Reporter::Json => json::render(report)
                .and_then(|text| write_file(run_dir, "report.json", text.as_bytes())),
            Reporter::Html => html::render(report, run_dir)
                .and_then(|text| write_file(run_dir, "report.html", text.as_bytes())),
        };
        if let Err(message) = result {
            errors.push(message);
        }
    }
    errors
}

/// Create `run_dir` if needed and write one reporter file into it.
pub(crate) fn write_file(run_dir: &Path, name: &str, content: &[u8]) -> Result<(), String> {
    if let Err(err) = std::fs::create_dir_all(run_dir) {
        return Err(format!(
            "cannot create the report directory {}: {err}",
            run_dir.display()
        ));
    }
    let path = run_dir.join(name);
    std::fs::write(&path, content).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

/// Wall-clock an Act captured across all of its attempts.
pub(crate) fn act_duration(act: &crate::report::ActReport) -> u64 {
    act.attempts.iter().map(|attempt| attempt.duration_ms).sum()
}

/// The step a failure's `step_index` refers to, when the attempt recorded
/// stages (setup failures have none).
pub(crate) fn failing_step(
    attempt: &crate::report::AttemptReport,
    index: usize,
) -> Option<&crate::report::StepReport> {
    attempt
        .stages
        .iter()
        .flat_map(|stage| stage.steps.iter())
        .nth(index)
}

/// Human duration for reports: `850 ms`, `1.25 s`, `12.3 s`.
#[must_use]
pub fn format_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 10_000 {
        format!("{:.2} s", ms as f64 / 1_000.0)
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1_000.0)
    } else {
        let secs = ms / 1_000;
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_accepts_every_name_and_dedupes() {
        assert_eq!(
            parse_list("console, junit,json ,html").expect("parses"),
            [
                Reporter::Console,
                Reporter::Junit,
                Reporter::Json,
                Reporter::Html
            ]
        );
        assert_eq!(parse_list("json,json").expect("parses"), [Reporter::Json]);
    }

    #[test]
    fn parse_list_rejects_unknown_and_empty() {
        assert!(parse_list("nope").is_err());
        assert!(parse_list(" , , ").is_err());
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(0), "0 ms");
        assert_eq!(format_duration(850), "850 ms");
        assert_eq!(format_duration(1_250), "1.25 s");
        assert_eq!(format_duration(12_340), "12.3 s");
        assert_eq!(format_duration(95_000), "1m35s");
    }
}
