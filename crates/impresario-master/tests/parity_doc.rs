//! Guard for `docs/PARITY.md` (spec 18): the generated matrices must stay
//! byte-identical to the planner's support tables (`impresario-core`'s
//! `action_support`/`check_support`/`key_support`, the single source of
//! truth for `IMP-DSL-013`).
//!
//! Run `IMPRESARIO_PARITY_BLESS=1 cargo test -p impresario-master --test
//! parity_doc` to rewrite the block between the markers after a table
//! change.

use std::path::{Path, PathBuf};

use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::support::{
    action_support, check_support, key_support, CheckVerb, Support, Verb,
};

const BEGIN: &str = "<!-- BEGIN GENERATED MATRIX -->";
const END: &str = "<!-- END GENERATED MATRIX -->";

/// Action rows of the matrix; `type`/`append`/`clear` share one row exactly
/// like the spec table. Order follows spec 6.1 (plus `wait`, which the spec
/// keeps master-side).
const ACTION_ROWS: &[&[Verb]] = &[
    &[Verb::Click],
    &[Verb::Type, Verb::Append, Verb::Clear],
    &[Verb::LongPress],
    &[Verb::Focus],
    &[Verb::Select],
    &[Verb::Scroll],
    &[Verb::ScrollTo],
    &[Verb::Swipe],
    &[Verb::Back],
    &[Verb::Key],
    &[Verb::Screenshot],
    &[Verb::AppLaunch],
    &[Verb::AppTerminate],
    &[Verb::AppRestart],
    &[Verb::AppBackground],
    &[Verb::Wait],
];

const CHECK_ROWS: &[&[CheckVerb]] = &[
    &[
        CheckVerb::Visible,
        CheckVerb::Hidden,
        CheckVerb::Enabled,
        CheckVerb::Disabled,
        CheckVerb::TextEquals,
        CheckVerb::TextContains,
        CheckVerb::Count,
    ],
    &[CheckVerb::Focused],
];

fn header(first: &str) -> String {
    let platforms = Platform::ALL
        .iter()
        .map(|platform| platform.as_str())
        .collect::<Vec<_>>();
    format!(
        "| {first} | {} |\n|{}\n",
        platforms.join(" | "),
        "---|".repeat(1 + platforms.len())
    )
}

fn row(label: &str, cells: impl Iterator<Item = String>) -> String {
    format!("| {label} | {} |\n", cells.collect::<Vec<_>>().join(" | "))
}

fn platforms() -> impl Iterator<Item = Platform> {
    Platform::ALL.into_iter()
}

/// The generated block between the markers of `docs/PARITY.md`.
fn render_matrix() -> String {
    let mut out = String::new();
    out.push_str(
        "Generated from the planner's own tables (`impresario-core::support`) — the same\n\
         functions that raise `IMP-DSL-013` at plan time. Kept in sync by\n\
         `cargo test -p impresario-master --test parity_doc`.\n\n",
    );
    out.push_str(
        "Legend: ✅ full · ◐ limited (see the list below) · ✖ unsupported — the planner\n\
         rejects ✖ before any device is touched.\n\n",
    );

    out.push_str("**Actions** (spec 6.1)\n\n");
    out.push_str(&header("Action"));
    for action_row in ACTION_ROWS {
        // The merged `type`/`append`/`clear` row must really be uniform.
        for platform in platforms() {
            let support = action_support(platform, action_row[0]);
            for verb in &action_row[1..] {
                assert_eq!(
                    action_support(platform, *verb),
                    support,
                    "{} does not share `{}`'s support on {}",
                    verb.as_str(),
                    action_row[0].as_str(),
                    platform.as_str()
                );
            }
        }
        let key_row = matches!(action_row, [Verb::Key]);
        let master_side = action_row.iter().all(|verb| verb.is_master_side());
        let label = if key_row {
            "`key <k>`".to_string()
        } else {
            action_row
                .iter()
                .map(|verb| format!("`{}`", verb.as_str()))
                .collect::<Vec<_>>()
                .join(" / ")
        };
        out.push_str(&row(
            &label,
            platforms().map(|platform| {
                if key_row {
                    "6.2".to_string()
                } else if master_side {
                    "—".to_string()
                } else {
                    action_support(platform, action_row[0]).glyph().to_string()
                }
            }),
        ));
    }
    out.push_str(
        "\n`wait` is master-side (decision D-10) and never crosses the wire; `key <k>` is\n\
         resolved per key in table 6.2 below.\n\n",
    );

    out.push_str("**Checks** (spec 6.1)\n\n");
    out.push_str(&header("Check"));
    for check_row in CHECK_ROWS {
        let label = check_row
            .iter()
            .map(|check| format!("`{}`", check.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&row(
            &label,
            platforms().map(|platform| check_support(platform, check_row[0]).glyph().to_string()),
        ));
    }
    out.push_str("\n`Check: P` with no condition means `is visible`.\n\n");

    out.push_str("**Remote/hardware keys** (spec 6.2)\n\n");
    out.push_str(&header("Key"));
    for key in RemoteKey::ALL {
        out.push_str(&row(
            &format!("`{}`", key.as_str()),
            platforms().map(|platform| key_support(platform, key).glyph().to_string()),
        ));
    }
    out.push_str(
        "\nDispatch codes/constants per platform are in spec 6.2 and in each executor's\n\
         key table (`KeyTable.kt`, `keys.rs`); the planner enforces exactly this ✅/✖ split.\n\n",
    );

    out.push_str("**Limitations behind the ◐ cells**\n\n");
    struct Group {
        note: &'static str,
        verbs: Vec<&'static str>,
        platforms: Vec<&'static str>,
    }
    let mut groups: Vec<Group> = Vec::new();
    for action_row in ACTION_ROWS {
        if matches!(action_row, [Verb::Key]) {
            continue;
        }
        for verb in action_row.iter() {
            for platform in platforms() {
                if let Support::Limited(note) = action_support(platform, *verb) {
                    let index = match groups.iter().position(|group| group.note == note) {
                        Some(index) => index,
                        None => {
                            groups.push(Group {
                                note,
                                verbs: Vec::new(),
                                platforms: Vec::new(),
                            });
                            groups.len() - 1
                        }
                    };
                    let group = groups.get_mut(index).expect("group was just created");
                    if !group.verbs.contains(&verb.as_str()) {
                        group.verbs.push(verb.as_str());
                    }
                    if !group.platforms.contains(&platform.as_str()) {
                        group.platforms.push(platform.as_str());
                    }
                }
            }
        }
    }
    for group in &groups {
        let verbs = group
            .verbs
            .iter()
            .map(|verb| format!("`{verb}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let platforms = group
            .platforms
            .iter()
            .map(|platform| format!("`{platform}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "- **{}** — {} on {}\n",
            group.note, verbs, platforms
        ));
    }

    out.trim_end().to_string()
}

fn parity_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/PARITY.md")
}

fn between_markers(text: &str) -> &str {
    let begin = text.find(BEGIN).expect("BEGIN marker in docs/PARITY.md");
    let end = text.find(END).expect("END marker in docs/PARITY.md");
    let inner = text
        .get(begin + BEGIN.len()..end)
        .expect("markers are in order");
    inner.trim_matches('\n')
}

fn splice(text: &str, generated: &str) -> String {
    let begin = text.find(BEGIN).expect("BEGIN marker in docs/PARITY.md");
    let end = text.find(END).expect("END marker in docs/PARITY.md");
    format!(
        "{}\n{}\n{}",
        &text[..begin + BEGIN.len()],
        generated.trim_end(),
        &text[end..]
    )
}

#[test]
fn parity_doc_matches_the_support_tables() {
    let path = parity_path();
    let raw = std::fs::read_to_string(&path).expect("docs/PARITY.md exists");
    let text = raw.replace("\r\n", "\n");
    let generated = render_matrix();

    if std::env::var("IMPRESARIO_PARITY_BLESS").as_deref() == Ok("1") {
        std::fs::write(&path, splice(&text, &generated)).expect("write docs/PARITY.md");
        eprintln!("parity_doc: rewrote {}", path.display());
        return;
    }

    assert_eq!(
        between_markers(&text),
        generated,
        "docs/PARITY.md's generated block is stale; rerun with IMPRESARIO_PARITY_BLESS=1"
    );
}
