//! Golden DSL corpus (TASK-2 section 10, Phase 2): formatter output and the
//! complete migration of a legacy workspace. The fixtures are plain files so
//! a diff shows exactly what changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use impresario_core::fmt::format_source;
use impresario_core::workspace;

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dsl-golden")
        .join(path)
}

#[test]
fn formatter_golden_files() {
    for (input, expected) in [
        ("messy/LoginScreen.scene", "expected/LoginScreen.scene"),
        ("messy/Auth.theatre", "expected/Auth.theatre"),
        ("messy/login.play", "expected/login.play"),
    ] {
        let source = std::fs::read_to_string(fixture(input)).expect("input fixture");
        let want = std::fs::read_to_string(fixture(expected)).expect("expected fixture");
        let formatted = format_source(&source, &fixture(input)).expect("formatter succeeds");
        assert_eq!(formatted, want, "{input} does not match {expected}");
        let again = format_source(&formatted, &fixture(input)).expect("formatted output parses");
        assert_eq!(again, formatted, "{input} is not idempotent");
    }
}

#[test]
fn migration_golden_workspace() {
    let input = fixture("input");
    let loaded =
        workspace::load_paths(&[input.join("LoginScreen.scene"), input.join("login.play")]);
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let migration =
        impresario_core::migrate::migrate(&loaded.workspace, &input.join("theatres"), &[]);
    assert!(!migration.errors.has_errors(), "{migration:?}");
    assert!(
        migration
            .warnings
            .iter()
            .any(|warning| warning.contains("renamed")),
        "{:?}",
        migration.warnings
    );

    let mut files = BTreeMap::new();
    for file in &migration.files {
        let path = file
            .path
            .strip_prefix(&input)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        files.insert(path, file.contents.clone());
    }
    insta::assert_yaml_snapshot!("migration_golden_workspace", files);
}
