//! `docs/CONFIG.md` is generated from `impresario-core::settings` (TASK-2
//! section 4.4): every key, its scope, type, default and description.
//!
//! Regenerate with `IMPRESARIO_CONFIG_BLESS=1 cargo test -p impresario-master
//! --test config_doc`.

use std::path::{Path, PathBuf};

use impresario_core::settings;

const BEGIN: &str = "<!-- BEGIN GENERATED CONFIG -->";
const END: &str = "<!-- END GENERATED CONFIG -->";

fn config_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/CONFIG.md")
}

fn between_markers(text: &str) -> &str {
    let begin = text.find(BEGIN).expect("BEGIN marker in docs/CONFIG.md");
    let end = text.find(END).expect("END marker in docs/CONFIG.md");
    let inner = text
        .get(begin + BEGIN.len()..end)
        .expect("markers are in order");
    inner.trim_matches('\n')
}

fn splice(text: &str, generated: &str) -> String {
    let begin = text.find(BEGIN).expect("BEGIN marker in docs/CONFIG.md");
    let end = text.find(END).expect("END marker in docs/CONFIG.md");
    format!(
        "{}\n{}\n{}",
        &text[..begin + BEGIN.len()],
        generated.trim_end(),
        &text[end..]
    )
}

#[test]
fn config_doc_matches_the_settings_schema() {
    let path = config_path();
    let raw = std::fs::read_to_string(&path).expect("docs/CONFIG.md exists");
    let text = raw.replace("\r\n", "\n");
    let generated = settings::docs_table();

    if std::env::var("IMPRESARIO_CONFIG_BLESS").as_deref() == Ok("1") {
        std::fs::write(&path, splice(&text, &generated)).expect("write docs/CONFIG.md");
        eprintln!("config_doc: rewrote {}", path.display());
        return;
    }

    assert_eq!(
        between_markers(&text),
        generated.trim_end(),
        "docs/CONFIG.md's generated block is stale; rerun with IMPRESARIO_CONFIG_BLESS=1"
    );
}
