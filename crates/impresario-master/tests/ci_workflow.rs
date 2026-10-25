//! Guard for the committed CI workflow: this host has no GitHub remote, so
//! at minimum the document must stay valid YAML with the jobs, runners and
//! commands the project promises (docs/STATUS.md, spec 17/18).

use std::path::Path;

fn workflow_file(name: &str) -> serde_yaml_ng::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.github/workflows")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{name} exists"));
    serde_yaml_ng::from_str(&text).unwrap_or_else(|_| panic!("{name} is valid YAML"))
}

fn workflow() -> serde_yaml_ng::Value {
    workflow_file("ci.yml")
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

    let editor = doc
        .get("jobs")
        .and_then(|jobs| jobs.get("editor"))
        .expect("editor job");
    assert_eq!(
        editor.get("runs-on").and_then(|value| value.as_str()),
        Some("ubuntu-latest")
    );
    let editor_runs = run_commands(editor);
    for expected in [
        "npm ci",
        "npm run build:wasm",
        "npm run check",
        "npm test",
        "npm run package",
    ] {
        assert!(
            editor_runs.iter().any(|run| run.contains(expected)),
            "editor job is missing `{expected}`: {editor_runs:?}"
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

#[test]
fn apple_simulator_workflow_runs_every_family_without_a_local_mac() {
    let doc = workflow_file("apple-simulator.yml");
    assert_eq!(
        doc.get("name").and_then(|value| value.as_str()),
        Some("Apple Simulator Tests")
    );

    let plan = doc
        .get("jobs")
        .and_then(|jobs| jobs.get("plan"))
        .expect("plan job");
    let plan_runs = run_commands(plan);
    for family in ["ios", "ipados", "tvos"] {
        assert!(
            plan_runs
                .iter()
                .any(|run| run.contains(&format!("\"{family}\""))),
            "the matrix plan must include {family}: {plan_runs:?}"
        );
    }

    let simulator = doc
        .get("jobs")
        .and_then(|jobs| jobs.get("simulator"))
        .expect("simulator job");
    assert_eq!(
        simulator.get("runs-on").and_then(|value| value.as_str()),
        Some("macos-15"),
        "GitHub's hosted macOS image is the Xcode host"
    );
    let simulator_runs = run_commands(simulator);
    for expected in [
        "scripts/apple-simulator.py boot",
        "build-for-testing",
        "SimulatorSmokeTests/testDriverAgainstStub",
        "apple-smoke.play",
        "IMPRESARIO_XCTESTRUN",
    ] {
        assert!(
            simulator_runs.iter().any(|run| run.contains(expected)),
            "the simulator job is missing `{expected}`: {simulator_runs:?}"
        );
    }
    assert!(
        simulator_runs
            .iter()
            .any(|run| run.contains("xcrun simctl install")),
        "the job must install the fixture app: {simulator_runs:?}"
    );
}
