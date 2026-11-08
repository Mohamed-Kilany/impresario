//! Guard for the committed CI workflow: the workflow must stay valid YAML and
//! keep the documented gates as separate, named jobs (docs/STATUS.md, spec
//! 17/18). This host cannot run the full suite locally, so this test pins the
//! commands and runner choices that CI relies on.

use std::path::Path;

fn workflow() -> serde_yaml_ng::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml");
    let text = std::fs::read_to_string(&path).expect(".github/workflows/ci.yml exists");
    serde_yaml_ng::from_str(&text).expect("ci.yml is valid YAML")
}

fn job<'a>(doc: &'a serde_yaml_ng::Value, name: &str) -> &'a serde_yaml_ng::Value {
    doc.get("jobs")
        .and_then(|jobs| jobs.get(name))
        .unwrap_or_else(|| panic!("{name} job"))
}

fn run_commands(value: &serde_yaml_ng::Value) -> Vec<String> {
    value
        .get("steps")
        .and_then(|steps| steps.as_sequence())
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| step.get("run").and_then(|run| run.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn assert_runs(job_name: &str, value: &serde_yaml_ng::Value, expected: &[&str]) {
    let runs = run_commands(value);
    for command in expected {
        assert!(
            runs.iter().any(|run| run.contains(*command)),
            "{job_name} job is missing `{command}`: {runs:?}"
        );
    }
}

fn assert_runs_on(value: &serde_yaml_ng::Value, expected: &str) {
    assert_eq!(
        value.get("runs-on").and_then(|runner| runner.as_str()),
        Some(expected)
    );
}

#[test]
fn ci_workflow_stays_valid_and_runs_the_documented_gates() {
    let doc = workflow();

    let fmt = job(&doc, "fmt");
    assert_runs_on(fmt, "ubuntu-latest");
    assert_runs("fmt", fmt, &["cargo fmt --all -- --check"]);

    let clippy = job(&doc, "clippy");
    assert_runs_on(clippy, "ubuntu-latest");
    assert_runs(
        "clippy",
        clippy,
        &["cargo clippy --workspace --all-targets --locked -- -D warnings"],
    );

    let test = job(&doc, "test");
    assert_runs_on(test, "ubuntu-latest");
    assert_runs("test", test, &["cargo test --workspace --locked --jobs 1"]);
    let test_env = test.get("env").expect("test job env");
    for key in ["CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG"] {
        assert_eq!(
            test_env.get(key).and_then(|value| value.as_str()),
            Some("0"),
            "test job must disable debug info ({key})"
        );
    }

    let windows = job(&doc, "windows");
    assert_runs_on(windows, "windows-latest");
    assert_runs(
        "windows",
        windows,
        &["cargo build --locked -p impresario-master -p impresario-connector --release"],
    );

    let editor = job(&doc, "editor");
    assert_runs_on(editor, "ubuntu-latest");
    assert_runs(
        "editor",
        editor,
        &[
            "npm ci",
            "npm run build:wasm",
            "npm run check",
            "npm test",
            "npm run package",
        ],
    );

    let android = job(&doc, "android");
    assert_runs_on(android, "ubuntu-latest");
    assert_runs(
        "android",
        android,
        &[
            "gradle --no-daemon test",
            "gradle --no-daemon assembleDebug",
            "bbaeb2fef8710818cf0e261201dab964c572f92b942812df0c3620d62a529a01",
            "repo.huaweicloud.com/gradle",
        ],
    );

    let apple = job(&doc, "apple");
    assert_runs_on(apple, "ubuntu-24.04");
    assert_runs(
        "apple",
        apple,
        &["swift package resolve", "swift build", "swift test"],
    );

    let gate = job(&doc, "gate");
    let needs = gate
        .get("needs")
        .and_then(|value| value.as_sequence())
        .expect("gate job needs");
    let needs: Vec<&str> = needs.iter().filter_map(|value| value.as_str()).collect();
    for expected in [
        "fmt", "clippy", "test", "windows", "editor", "android", "apple",
    ] {
        assert!(
            needs.contains(&expected),
            "gate job must depend on {expected}: {needs:?}"
        );
    }
    assert_runs("gate", gate, &["needs.fmt.result", "needs.apple.result"]);
}
