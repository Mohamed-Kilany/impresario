//! The embedded fixture suite is a real workspace: every `.scene`, `.theatre`
//! and `.play` must parse, resolve and validate on every platform it claims.
//!
//! Seeding is exercised too, so the files compiled into the binary can never
//! drift from the files a first run writes.

use impresario_core::diag::Diagnostics;
use impresario_core::home::Home;
use impresario_core::platforms::Platform;
use impresario_core::validation::{self, DeviceRef};
use impresario_core::workspace;
use impresario_master::install;

fn render(diagnostics: &Diagnostics) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn embedded_stub_suite_parses_and_validates_on_every_platform() {
    let dir = tempfile::tempdir().expect("temp home");
    let home = Home::at(dir.path());
    let report = install::ensure_stub_tests(&home).expect("seed the suite");
    assert!(report.written > 0, "a new home seeds the suite");
    assert_eq!(report.kept, 0);
    let loaded = workspace::load_paths(&[report.dir.clone()]);
    assert_eq!(
        loaded.diagnostics.errors().count(),
        0,
        "{}",
        render(&loaded.diagnostics)
    );

    let workspace = loaded.workspace;
    assert_eq!(workspace.scenes.len(), 9, "one scene per fixture screen");
    assert_eq!(workspace.theatres.len(), 3, "common, touch and remote");
    assert_eq!(workspace.plays.len(), 1);

    let play = &workspace.plays[0];
    assert_eq!(play.name, "Impresario stub suite");
    let acts: Vec<&str> = play.acts.iter().map(|act| act.name.as_str()).collect();
    assert_eq!(
        acts,
        vec![
            "Home",
            "Basic",
            "Text",
            "Scroll",
            "Selection",
            "Flags",
            "Keys",
            "Info",
            "LongPress",
            "Media",
        ]
    );

    // Every stage in the play exists for every platform it can be planned on.
    for act in &play.acts {
        let restricted: Vec<Platform> = act
            .platforms
            .iter()
            .filter_map(|name| name.name.parse::<Platform>().ok())
            .collect();
        for platform in Platform::ALL {
            if act.name == "LongPress" && platform == Platform::Web {
                continue;
            }
            if !restricted.is_empty() && !restricted.contains(&platform) {
                continue;
            }
            let stage = act.performs[0].stage_name.as_str();
            assert!(
                workspace.stage_for(stage, platform).is_some(),
                "stage '{stage}' (act '{}') has no {platform} implementation",
                act.name
            );
        }
    }

    let devices = [
        DeviceRef::new("browser", Platform::Web),
        DeviceRef::new("phone", Platform::Android),
        DeviceRef::new("tv", Platform::AndroidTv),
        DeviceRef::new("iphone", Platform::Ios),
        DeviceRef::new("appletv", Platform::Tvos),
        DeviceRef::new("lg", Platform::WebOs),
        DeviceRef::new("samsung", Platform::Tizen),
    ];
    let diagnostics = validation::validate(&workspace, &devices);
    assert_eq!(
        diagnostics.errors().count(),
        0,
        "suite validation: {}",
        render(&diagnostics)
    );
    assert_eq!(
        diagnostics.warnings().count(),
        0,
        "suite lints: {}",
        render(&diagnostics)
    );

    // Long press is deliberately absent for a plain browser.
    assert!(workspace
        .stage_for("LongPress", Platform::Android)
        .is_some());
    assert!(workspace.stage_for("LongPress", Platform::Ios).is_some());
    assert!(workspace.stage_for("LongPress", Platform::Tvos).is_some());
    assert!(workspace.stage_for("LongPress", Platform::Web).is_none());

    // The key log is implemented for every platform, with disjoint audiences.
    for platform in Platform::ALL {
        assert!(
            workspace.stage_for("Keys", platform).is_some(),
            "no Keys implementation for {platform}"
        );
    }
}
