//! Completion contexts (spec section 13).
//!
//! Everything is computed from the current line prefix and the nearest
//! enclosing `Scene:` line of lower indentation, scanning upward, plus the
//! workspace index — so it keeps working while the rest of the file has
//! errors.

use std::collections::BTreeSet;

use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::support::{self, CheckVerb, Verb};
use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind};

use super::index::Index;
use super::position::{enclosing_scene, indent_of};

/// The inputs of one completion request.
pub struct CompleteCtx<'a> {
    /// Full document text (overlay first).
    pub text: &'a str,
    /// 0-based cursor line.
    pub line: usize,
    /// Cursor column, as a 0-based scalar index in that line.
    pub col: usize,
    /// True for `.scene` files.
    pub is_scene: bool,
    pub index: &'a Index,
    /// Platforms of the configured devices; empty = the device set is unknown
    /// and keys are not filtered.
    pub platforms: &'a [Platform],
}

/// The action verbs offered by the completion, in DSL order.
const VERBS: &[Verb] = &[
    Verb::Click,
    Verb::Type,
    Verb::Append,
    Verb::Clear,
    Verb::LongPress,
    Verb::Focus,
    Verb::Select,
    Verb::Scroll,
    Verb::ScrollTo,
    Verb::Swipe,
    Verb::Back,
    Verb::Key,
    Verb::Wait,
    Verb::Screenshot,
    Verb::AppLaunch,
    Verb::AppTerminate,
    Verb::AppRestart,
    Verb::AppBackground,
];

/// Suggestions for the cursor position, or `None` when the position has no
/// completion context at all.
#[must_use]
pub fn complete(ctx: &CompleteCtx<'_>) -> Option<Vec<CompletionItem>> {
    let lines: Vec<&str> = ctx.text.lines().collect();
    let line = *lines.get(ctx.line)?;
    let prefix: String = line.chars().take(ctx.col).collect();
    let indent = indent_of(line);
    let trimmed = prefix.trim_start();

    if !ctx.is_scene {
        if let Some(rest) = trimmed.strip_prefix("Perform:") {
            let after = rest.trim_start();
            if let Some((stage_name, arguments)) = after.split_once('(') {
                if !after.contains(')') || arguments.is_empty() {
                    return Some(stage_param_items(ctx.index, stage_name.trim()));
                }
            }
            return Some(stage_items(ctx.index));
        }
    }
    if ctx.is_scene && inside_pieces(&lines, ctx.line) {
        return Some(piece_entry_items(&prefix));
    }
    if trimmed.strip_prefix("Scene:").is_some() {
        if ctx.is_scene {
            return Some(Vec::new());
        }
        return Some(scene_items(ctx.index));
    }
    if let Some(rest) = trimmed.strip_prefix("Action:") {
        if rest.contains("=>") {
            return Some(match enclosing_scene(&lines, ctx.line, indent) {
                Some(scene) => piece_items(ctx.index, &scene),
                None => Vec::new(),
            });
        }
        let after = rest.trim_start();
        if let Some(partial) = after.strip_prefix("key ") {
            if !partial.contains(char::is_whitespace) && !partial.contains('\'') {
                return Some(key_items(ctx.platforms));
            }
        }
        return Some(verb_items());
    }
    if let Some(rest) = trimmed.strip_prefix("Check:") {
        let after = rest.trim_start();
        let words = after.split_whitespace().count();
        let trailing = after.chars().last().is_some_and(char::is_whitespace);
        if words >= 2 || (words == 1 && trailing) {
            return Some(condition_items());
        }
        return Some(match enclosing_scene(&lines, ctx.line, indent) {
            Some(scene) => piece_items(ctx.index, &scene),
            None => Vec::new(),
        });
    }
    if trimmed.starts_with("Tags:") {
        return Some(tag_items(ctx.index));
    }
    if trimmed.starts_with("Platforms:") || trimmed.starts_with("Platform:") {
        return Some(platform_items(ctx.is_scene));
    }
    None
}

/// True when the nearest line above with a lower indentation is `Pieces:`.
fn inside_pieces(lines: &[&str], cursor_line: usize) -> bool {
    let indent = lines.get(cursor_line).map_or(0, |line| indent_of(line));
    for index in (0..cursor_line).rev() {
        let Some(line) = lines.get(index) else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if indent_of(line) < indent {
            return trimmed == "Pieces:";
        }
    }
    false
}

fn item(label: impl Into<String>, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: Some(kind),
        ..CompletionItem::default()
    }
}

fn scene_items(index: &Index) -> Vec<CompletionItem> {
    index
        .scenes
        .iter()
        .map(|(name, info)| {
            let mut entry = item(name.as_str(), CompletionItemKind::FILE);
            entry.detail = Some(info.platforms_summary());
            entry
        })
        .collect()
}

fn stage_items(index: &Index) -> Vec<CompletionItem> {
    index
        .stages
        .iter()
        .map(|(name, info)| {
            let mut entry = item(name.as_str(), CompletionItemKind::FUNCTION);
            entry.detail = Some(format!("{} ({})", info.signature, info.theatre));
            entry
        })
        .collect()
}

fn stage_param_items(index: &Index, stage_name: &str) -> Vec<CompletionItem> {
    let Some(info) = index.stage(stage_name) else {
        return Vec::new();
    };
    info.params
        .iter()
        .map(|(name, default)| {
            let mut entry = item(format!("{name} = "), CompletionItemKind::PROPERTY);
            entry.detail = Some(match default {
                Some(default) => format!("default: '{default}'"),
                None => "required".to_string(),
            });
            entry
        })
        .collect()
}

fn piece_items(index: &Index, scene: &str) -> Vec<CompletionItem> {
    let Some(info) = index.scenes.get(scene) else {
        return Vec::new();
    };
    let mut pieces: Vec<_> = info.pieces.iter().collect();
    pieces.sort_by(|a, b| a.name.cmp(&b.name));
    pieces
        .into_iter()
        .map(|piece| {
            let mut entry = item(piece.name.as_str(), CompletionItemKind::REFERENCE);
            entry.detail = Some(piece.selectors_summary());
            entry
        })
        .collect()
}

fn condition_items() -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = [
        CheckVerb::Visible,
        CheckVerb::Hidden,
        CheckVerb::Enabled,
        CheckVerb::Disabled,
        CheckVerb::Focused,
    ]
    .into_iter()
    .map(|verb| item(format!("is {}", verb.as_str()), CompletionItemKind::KEYWORD))
    .collect();
    for verb in [
        CheckVerb::TextEquals,
        CheckVerb::TextContains,
        CheckVerb::Count,
    ] {
        items.push(item(verb.as_str(), CompletionItemKind::KEYWORD));
    }
    items.push(item("timeout", CompletionItemKind::KEYWORD));
    items
}

fn verb_items() -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = VERBS
        .iter()
        .map(|verb| item(verb.as_str(), CompletionItemKind::KEYWORD))
        .collect();
    let mut starter = item("'", CompletionItemKind::SNIPPET);
    starter.insert_text = Some("''".to_string());
    items.push(starter);
    items
}

fn key_items(platforms: &[Platform]) -> Vec<CompletionItem> {
    let known = platform_set(platforms);
    RemoteKey::ALL
        .iter()
        .copied()
        .filter(|key| {
            known
                .iter()
                .any(|platform| support::key_support(*platform, *key).is_supported())
        })
        .map(|key| {
            let supported: Vec<&str> = known
                .iter()
                .filter(|platform| support::key_support(**platform, key).is_supported())
                .map(|platform| platform.as_str())
                .collect();
            let mut entry = item(key.as_str(), CompletionItemKind::CONSTANT);
            entry.detail = Some(format!("keys: {}", supported.join(", ")));
            entry
        })
        .collect()
}

/// The distinct platforms of the configured devices, or every platform when
/// the device set is unknown.
fn platform_set(platforms: &[Platform]) -> Vec<Platform> {
    if platforms.is_empty() {
        return Platform::ALL.to_vec();
    }
    let mut seen = Vec::new();
    for platform in platforms {
        if !seen.contains(platform) {
            seen.push(*platform);
        }
    }
    seen
}

fn tag_items(index: &Index) -> Vec<CompletionItem> {
    index
        .tags
        .iter()
        .map(|tag| item(tag.as_str(), CompletionItemKind::VALUE))
        .collect()
}

fn platform_items(is_scene: bool) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Platform::ALL
        .iter()
        .map(|platform| item(platform.as_str(), CompletionItemKind::VALUE))
        .collect();
    if is_scene {
        items.push(item("cross-platform", CompletionItemKind::VALUE));
    }
    items
}

/// The key of a `Name: key="selector"` fragment: the text between the last
/// comma and the assignment, after the piece name.
fn key_of(fragment: &str) -> &str {
    let before_eq = fragment.split_once('=').map_or(fragment, |(left, _)| left);
    let after_colon = before_eq
        .rsplit_once(':')
        .map_or(before_eq, |(_, right)| right);
    after_colon.trim()
}

fn platform_by_name(name: &str) -> Option<Platform> {
    Platform::ALL
        .iter()
        .copied()
        .find(|platform| platform.as_str() == name)
}

fn selector_prefix_items(key: Platform) -> Vec<CompletionItem> {
    let prefixes: &[&str] = match key {
        Platform::Android | Platform::AndroidTv => {
            &["id:", "text:", "textContains:", "desc:", "class:"]
        }
        Platform::Ios | Platform::Tvos => &[
            "accessibilityId:",
            "label:",
            "labelContains:",
            "predicate:",
            "type:",
        ],
        Platform::Web | Platform::WebOs | Platform::Tizen => {
            &["css:", "xpath:", "text:", "textContains:"]
        }
    };
    prefixes
        .iter()
        .map(|prefix| item(*prefix, CompletionItemKind::ENUM_MEMBER))
        .collect()
}

/// Suggestions inside a `Pieces:` entry: platform keys, then selector prefixes
/// of the current key.
fn piece_entry_items(prefix: &str) -> Vec<CompletionItem> {
    let segment = prefix.rsplit(',').next().unwrap_or(prefix);
    let mut inside_string = false;
    let mut content = String::new();
    for ch in segment.chars() {
        match ch {
            '"' => {
                inside_string = !inside_string;
                content.clear();
            }
            _ if inside_string => content.push(ch),
            _ => {}
        }
    }
    if inside_string {
        if content.contains(':') {
            return Vec::new();
        }
        return match platform_by_name(key_of(segment)) {
            Some(key) => selector_prefix_items(key),
            None => Vec::new(),
        };
    }
    if segment.ends_with('"') {
        return Vec::new();
    }
    if segment.contains('=') {
        return vec![item("\"", CompletionItemKind::OPERATOR)];
    }
    let used: BTreeSet<&str> = prefix.split(',').map(key_of).collect();
    Platform::ALL
        .iter()
        .filter(|platform| !used.contains(platform.as_str()))
        .map(|platform| {
            let mut entry = item(
                format!("{}=", platform.as_str()),
                CompletionItemKind::PROPERTY,
            );
            entry.detail = Some(selector_words(*platform));
            entry
        })
        .collect()
}

fn selector_words(key: Platform) -> String {
    selector_prefix_items(key)
        .iter()
        .map(|item| item.label.trim_end_matches(':').to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = r##"Scene: LoginScreen
Platform: cross-platform

Pieces:
    LoginButton: android="id:login_button", web="#login"
    UsernameInput: android="id:username", ios="accessibilityId:username"
"##;

    const PLAY: &str = r#"Play: Login
Description: sign in

Var: USER = 'alice'

Act: Successful Login
Tags: smoke, auth
Platforms: android, webos
    Perform: Navigate

Stage: Navigate
    Scene: LoginScreen
        Action: click => LoginButton
        Action: type 'x' => UsernameInput
        Check: LoginButton
"#;

    const THEATRE: &str = r#"Theatre: Auth

Stage: Navigate
    Scene: LoginScreen
        Action: click => LoginButton

Stage: Fill(user, password = 'secret')
    Scene: LoginScreen
        Action: type '${param.user}' => UsernameInput
"#;

    fn test_index() -> Index {
        let scene_path = std::env::temp_dir().join("impresario-lsp/scenes/LoginScreen.scene");
        let play_path = std::env::temp_dir().join("impresario-lsp/plays/login.play");
        let theatre_path = std::env::temp_dir().join("impresario-lsp/theatres/auth.theatre");
        let scene = impresario_core::parser::parse_scene(SCENE, &scene_path)
            .ast
            .expect("scene parses");
        let theatre = impresario_core::parser::parse_theatre(THEATRE, &theatre_path)
            .ast
            .expect("theatre parses");
        let play = impresario_core::parser::parse_play(PLAY, &play_path)
            .ast
            .expect("play parses");
        Index::build(&[scene], &[theatre], &[play])
    }

    #[test]
    fn perform_suggests_stages_and_their_parameters_with_defaults() {
        let stages = "Play: P\n\nAct: A\n    Perform: |\n";
        let items = complete_at(stages, false, &[]);
        let stage_labels = labels(&items);
        assert!(
            stage_labels.contains(&"Fill".to_string()),
            "{stage_labels:?}"
        );
        assert!(
            stage_labels.contains(&"Navigate".to_string()),
            "{stage_labels:?}"
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.label == "Fill")
                .and_then(|item| item.detail.as_deref()),
            Some("Stage: Fill(user, password = 'secret') (Auth)")
        );

        let params = "Play: P\n\nAct: A\n    Perform: Fill(|\n";
        let items = complete_at(params, false, &[]);
        assert_eq!(labels(&items), vec!["user = ", "password = "]);
        assert_eq!(items[0].detail.as_deref(), Some("required"));
        assert_eq!(items[1].detail.as_deref(), Some("default: 'secret'"));
    }

    /// Split the caret marker `|` out of a playground text.
    fn split_caret(text: &str) -> (String, usize, usize) {
        let mut line = 0usize;
        let mut col = 0usize;
        let mut found = false;
        let mut out = String::new();
        for ch in text.chars() {
            if ch == '|' && !found {
                found = true;
                continue;
            }
            if !found {
                if ch == '\n' {
                    line += 1;
                    col = 0;
                } else {
                    col += 1;
                }
            }
            out.push(ch);
        }
        (out, line, col)
    }

    fn complete_at(text: &str, is_scene: bool, platforms: &[Platform]) -> Vec<CompletionItem> {
        let (text, line, col) = split_caret(text);
        let index = test_index();
        let ctx = CompleteCtx {
            text: &text,
            line,
            col,
            is_scene,
            platforms,
            index: &index,
        };
        complete(&ctx).expect("a known completion context")
    }

    fn labels(items: &[CompletionItem]) -> Vec<String> {
        items.iter().map(|item| item.label.clone()).collect()
    }

    #[test]
    fn scene_line_suggests_every_scene() {
        let play = "Act: A\n\nStage: S\n    Scene: |\n";
        let items = complete_at(play, false, &[]);
        assert_eq!(labels(&items), vec!["LoginScreen"]);
    }

    #[test]
    fn action_arrow_suggests_sorted_pieces_with_selector_details() {
        let play = "Act: A\n\nStage: S\n    Scene: LoginScreen\n        Action: click => |\n";
        let items = complete_at(play, false, &[]);
        assert_eq!(labels(&items), vec!["LoginButton", "UsernameInput"]);
        assert_eq!(
            items[0].detail.as_deref(),
            Some("android=\"id:login_button\", web=\"#login\"")
        );
        assert_eq!(items[0].kind, Some(CompletionItemKind::REFERENCE));
    }

    #[test]
    fn check_starts_with_pieces_then_conditions() {
        let pieces = "Stage: S\n    Scene: LoginScreen\n        Check: |\n";
        assert_eq!(
            labels(&complete_at(pieces, false, &[])),
            vec!["LoginButton", "UsernameInput"]
        );
        let after_piece = "Stage: S\n    Scene: LoginScreen\n        Check: LoginButton |\n";
        assert_eq!(
            labels(&complete_at(after_piece, false, &[])),
            vec![
                "is visible",
                "is hidden",
                "is enabled",
                "is disabled",
                "is focused",
                "text",
                "textContains",
                "count",
                "timeout"
            ]
        );
    }

    #[test]
    fn action_start_suggests_verbs_and_the_string_starter() {
        let play = "Stage: S\n    Scene: LoginScreen\n        Action: |\n";
        let items = complete_at(play, false, &[]);
        let all = labels(&items);
        assert!(all.contains(&"click".to_string()));
        assert!(all.contains(&"app background".to_string()));
        assert!(all.contains(&"scrollto".to_string()));
        let starter = items.last().expect("starter");
        assert_eq!(starter.label, "'");
        assert_eq!(starter.insert_text.as_deref(), Some("''"));
    }

    #[test]
    fn key_names_are_filtered_by_the_device_platforms() {
        let play = "Stage: S\n    Scene: LoginScreen\n        Action: key |\n";
        let unfiltered = complete_at(play, false, &[]);
        assert_eq!(unfiltered.len(), RemoteKey::ALL.len());
        let web_only = complete_at(play, false, &[Platform::Web]);
        let expected: Vec<&str> = RemoteKey::ALL
            .iter()
            .filter(|key| support::key_support(Platform::Web, **key).is_supported())
            .map(|key| key.as_str())
            .collect();
        assert_eq!(labels(&web_only), expected);
        assert!(web_only.len() < unfiltered.len());
    }

    #[test]
    fn tags_come_from_the_workspace() {
        let play = "Act: A\nTags: |\n";
        assert_eq!(
            labels(&complete_at(play, false, &[])),
            vec!["auth", "smoke"]
        );
    }

    #[test]
    fn platforms_and_cross_platform_in_scenes() {
        let play = "Act: A\nPlatforms: |\n";
        let play_labels = labels(&complete_at(play, false, &[]));
        assert_eq!(
            play_labels,
            vec![
                "web",
                "webos",
                "tizen",
                "android",
                "androidtv",
                "ios",
                "tvos"
            ]
        );
        let scene = "Scene: S\nPlatform: |\n";
        assert_eq!(
            labels(&complete_at(scene, true, &[]))
                .last()
                .map(String::as_str),
            Some("cross-platform")
        );
    }

    #[test]
    fn pieces_entry_offers_platform_keys_and_prefixes() {
        let empty = "Scene: S\n\nPieces:\n    |\n";
        assert_eq!(
            labels(&complete_at(empty, true, &[])),
            vec![
                "web=",
                "webos=",
                "tizen=",
                "android=",
                "androidtv=",
                "ios=",
                "tvos="
            ]
        );
        let after_key = "Scene: S\n\nPieces:\n    Login: web=\"#login\", |\n";
        let items = complete_at(after_key, true, &[]);
        assert_eq!(
            labels(&items),
            vec![
                "webos=",
                "tizen=",
                "android=",
                "androidtv=",
                "ios=",
                "tvos="
            ]
        );
        assert_eq!(
            items[0].detail.as_deref(),
            Some("css, xpath, text, textContains")
        );
        let inside = "Scene: S\n\nPieces:\n    Login: android=\"|\"\n";
        assert_eq!(
            labels(&complete_at(inside, true, &[])),
            vec!["id:", "text:", "textContains:", "desc:", "class:"]
        );
        let deep = "Scene: S\n\nPieces:\n    Login: android=\"id:lo|\"\n";
        assert!(complete_at(deep, true, &[]).is_empty());
        let after_eq = "Scene: S\n\nPieces:\n    Login: android=|\n";
        assert_eq!(labels(&complete_at(after_eq, true, &[])), vec!["\""]);
    }

    #[test]
    fn context_survives_broken_files_and_missing_scenes() {
        let broken =
            "Stage: S\n    Scene: LoginScreen\n        Action: click => |\n??garbage((( \n";
        assert_eq!(
            labels(&complete_at(broken, false, &[])),
            vec!["LoginButton", "UsernameInput"]
        );
        let unknown = "Stage: S\n    Scene: Nope\n        Action: click => |\n";
        assert!(complete_at(unknown, false, &[]).is_empty());
    }
}
