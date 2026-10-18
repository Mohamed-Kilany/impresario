//! Guard for the committed CI workflow: this host has no GitHub remote, so
//! at minimum the document must stay valid YAML with the jobs, runners and
//! commands the project promises (docs/STATUS.md, spec 17/18).

use std::path::Path;

fn workflow() -> serde_yaml_ng::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/ci.yml");
    let text = std::fs::read_to_string(&path).expect(".github/workflows/ci.yml exists");
    serde_yaml_ng::from_str(&text).expect("ci.yml is valid YAML")
}

fn run_commands(job: &serde_yaml_ng::Value) -> Vec<String> {
    job.get("steps")
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

#[test]
fn ci_workflow_stays_valid_and_runs_the_documented_gates() {
    let doc = workflow();

    let rust = doc
        .get("jobs")
        .and_then(|jobs| jobs.get("rust"))
        .expect("rust job");
    assert_eq!(
        rust.get("runs-on").and_then(|value| value.as_str()),
        Some("ubuntu-latest"),
        "the rust job must run where google-chrome exists (connector fixtures)"
    );
    let rust_runs = run_commands(rust);
    for expected in [
        "cargo fmt --check",
        "cargo clippy --workspace --all-targets -- -D warnings",
        "cargo test --workspace",
    ] {
        assert!(
            rust_runs.iter().any(|run| run.contains(expected)),
            "rust job is missing `{expected}`: {rust_runs:?}"
        );
    }

    let android = doc
        .get("jobs")
        .and_then(|jobs| jobs.get("android"))
        .expect("android job");
    assert_eq!(
        android.get("runs-on").and_then(|value| value.as_str()),
        Some("ubuntu-latest")
    );
    let android_runs = run_commands(android);
    assert!(
        android_runs.iter().any(|run| run.contains("gradlew")),
        "android job must run the Gradle unit tests: {android_runs:?}"
    );
}
