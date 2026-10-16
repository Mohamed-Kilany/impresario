//! Never-panics fuzzing for the parser and validator (spec 5.7).
//!
//! Three generators: arbitrary text, DSL-shaped text assembled from line
//! fragments, and mutations of the committed examples. Every generated input
//! is fed to `parse_scene`, `parse_play` and (when it yields an AST) the
//! semantic validation pass. A panic anywhere fails the test.
//!
//! A `cargo-fuzz` target mirroring this corpus lives in `fuzz/` and is
//! documented as unverified in `docs/STATUS.md` (it needs nightly).

use std::path::Path;

use impresario_core::parser::{parse_play, parse_scene};
use impresario_core::platforms::Platform;
use impresario_core::validation::{validate, DeviceRef};
use impresario_core::workspace::Workspace;
use proptest::prelude::*;

/// The committed examples (also parsed by `tests/examples_snapshot.rs`).
const EXAMPLE_SCENE: &str = include_str!("../../../examples/scenes/LoginScreen.scene");
const EXAMPLE_PLAY: &str = include_str!("../../../examples/plays/login.play");

/// Parse and validate one input under every entry point that takes it.
fn exercise(source: &str) {
    let scene = parse_scene(source, Path::new("fuzz.scene"));
    let play = parse_play(source, Path::new("fuzz.play"));

    let workspace = Workspace {
        scenes: scene.ast.into_iter().collect(),
        plays: play.ast.into_iter().collect(),
    };
    let devices = [
        DeviceRef::new("tv", Platform::WebOs),
        DeviceRef::new("phone", Platform::Android),
        DeviceRef::new("iphone", Platform::Ios),
    ];
    let _ = validate(&workspace, &devices);
}

/// Fragments of a plausible DSL file; joined with newlines they produce a
/// mix of valid lines, near misses and nonsense.
fn fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Scene: LoginScreen".to_string()),
        Just("Scene: login_screen".to_string()),
        Just("Platform: cross-platform".to_string()),
        Just("Platform: webos, tizen".to_string()),
        Just("Platform: web".to_string()),
        Just("Pieces:".to_string()),
        Just("    LoginTitle: web=\"h1.title\", android=\"id:login_title\", ios=\"accessibilityId:login\""
            .to_string()),
        Just("    Weird: web=\"#a\", index=3".to_string()),
        Just("    Broken: nope=\"#x\"".to_string()),
        Just("Play: Fuzz".to_string()),
        Just("Description: generated".to_string()),
        Just("Var: USER = 'value'".to_string()),
        Just("Var: USER = '${env.TEST_PASSWORD}'".to_string()),
        Just("Act: Flow".to_string()),
        Just("Tags: smoke, auth".to_string()),
        Just("Platforms: android, ios".to_string()),
        Just("Stage: Enter".to_string()),
        Just("    Timeout: 15s".to_string()),
        Just("    Scene: LoginScreen".to_string()),
        Just("        Action: '${var.USER}' => LoginTitle".to_string()),
        Just("        Action: append 'x' => LoginTitle timeout 2s".to_string()),
        Just("        Action: clear => LoginTitle".to_string()),
        Just("        Action: longpress 500ms => LoginTitle".to_string()),
        Just("        Action: scroll down 3 => LoginTitle".to_string()),
        Just("        Action: swipe up".to_string()),
        Just("        Action: key rewind 2".to_string()),
        Just("        Action: back".to_string()),
        Just("        Action: wait 1s".to_string()),
        Just("        Action: screenshot 'shot'".to_string()),
        Just("        Action: app restart".to_string()),
        Just("        Check: LoginTitle".to_string()),
        Just("        Check: LoginTitle is focused timeout 20s".to_string()),
        Just("        Check: LoginTitle textContains 'x'".to_string()),
        Just("        Check: LoginTitle count 0".to_string()),
        Just("\tScene: Tab".to_string()),
        Just("  Bad: web=\"#a\"".to_string()),
        Just("// comment".to_string()),
        Just("".to_string()),
        Just("${env.X} ${var.Y} \\u{1F600} \\n".to_string()),
        Just("        Action: ".to_string()),
        Just("        Check: ".to_string()),
        any::<String>(),
    ]
}

#[derive(Debug, Clone)]
enum Mutation {
    Delete(usize),
    Insert(usize, char),
    Replace(usize, char),
    Swap(usize, usize),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (0usize..4096).prop_map(Mutation::Delete),
        (0usize..4096, any::<char>()).prop_map(|(index, ch)| Mutation::Insert(index, ch)),
        (0usize..4096, any::<char>()).prop_map(|(index, ch)| Mutation::Replace(index, ch)),
        (0usize..4096, 0usize..4096).prop_map(|(a, b)| Mutation::Swap(a, b)),
    ]
}

fn apply(source: &str, mutations: &[Mutation]) -> String {
    let mut chars: Vec<char> = source.chars().collect();
    for mutation in mutations {
        match *mutation {
            Mutation::Delete(index) => {
                if !chars.is_empty() {
                    let at = index % chars.len();
                    chars.remove(at);
                }
            }
            Mutation::Insert(index, ch) => {
                let at = if chars.is_empty() {
                    0
                } else {
                    index % (chars.len() + 1)
                };
                chars.insert(at, ch);
            }
            Mutation::Replace(index, ch) => {
                if !chars.is_empty() {
                    let at = index % chars.len();
                    if let Some(slot) = chars.get_mut(at) {
                        *slot = ch;
                    }
                }
            }
            Mutation::Swap(a, b) => {
                if !chars.is_empty() {
                    let x = a % chars.len();
                    let y = b % chars.len();
                    chars.swap(x, y);
                }
            }
        }
    }
    chars.into_iter().collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_text_never_panics(text in any::<String>()) {
        exercise(&text);
    }

    #[test]
    fn dsl_shaped_text_never_panics(lines in prop::collection::vec(fragment(), 0..40)) {
        exercise(&lines.join("\n"));
    }

    #[test]
    fn mutated_examples_never_panic(
        mutations in prop::collection::vec(mutation(), 1..24),
        which in 0usize..2,
    ) {
        let source = if which == 0 { EXAMPLE_SCENE } else { EXAMPLE_PLAY };
        exercise(&apply(source, &mutations));
    }
}

#[test]
fn nasty_corpus_never_panics() {
    let corpus: Vec<String> = vec![
        "\u{feff}".to_string(),
        "\r".to_string(),
        "\n\n\n".to_string(),
        "\0\u{1}\u{7f}".to_string(),
        "\u{10FFFF}".to_string(),
        "a\nb\nc".to_string(),
        "Scene:\tA".to_string(),
        "Scene: A\n\t\tPieces:\t".to_string(),
        "Scene: A\nPlatform: web\nPieces:\n    P: web=\"\\u{110000}\"".to_string(),
        "Scene: A\nPlatform: web\nPieces:\n    P: web=\"#a\", index=4294967295\n".to_string(),
        "Scene: A\nPlatform: web\nPieces:\n    P: web=\"#a\", index=99999999999999999999\n"
            .to_string(),
        "Play: P\nVar: X = '${".to_string(),
        "Play: P\n\nAct: A\n\nStage: S\n    Scene: S\n        Action: ".to_string(),
        format!("Play: {}\n", "x".repeat(10_000)),
        format!("Play: P\n\nAct: A\n\nStage: S\n    Timeout: {}ms\n    Scene: S\n        Check: A\n", u64::MAX),
        "Scene: A\r\nPlatform: web\r\nPieces:\r\n    P: web=\"#p\"".to_string(),
        // Deeply nested indentation (still only depth 3 grammar-wise).
        "Play: P\n\nAct: A\n\nStage: S\n    Scene: S\n        Check: A\n            Check: B\n                Check: C\n"
            .to_string(),
        // A line made only of escapes and interpolation tokens.
        "        Action: '\\n\\t\\\\\\'\\\"${var.X}${env.}${}' => P".to_string(),
    ];
    for source in &corpus {
        exercise(source);
    }
}
