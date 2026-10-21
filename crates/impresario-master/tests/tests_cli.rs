//! Test explorer and test-file operations (TASK-2 3.4) against a real
//! workspace. The explorer must expose the same tree the TUI shows, and every
//! file operation must either fail loudly or leave the workspace parseable.

mod common;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use common::{write_workspace, PLAY_BASIC};
use serde_json::Value;

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 2;
const EXIT_INFRA: i32 = 3;

const THEATRE: &str = r#"Theatre: Auth

Stage: Login(user = 'x')
    Platforms: web, android
    Scene: TestScreen
        Check: A
        Action: click => B

Stage: WebOnly
    Platforms: web
    Scene: TestScreen
        Check: A
"#;

const PERFORM_PLAY: &str = r#"Play: Perform

Act: LoginAct
Tags: auth, smoke
Platforms: web, android
    Examples:
        | user  |
        |-------|
        | alice |
        | bob   |
    Perform: Login(user = '${example.user}')

Act: WebAct
    Perform: WebOnly

Act: PlainAct
Tags: plain
    Perform: Login
"#;

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn impresario(home: &Path, workspace: &Path) -> Command {
    let mut command = Command::cargo_bin("impresario").expect("binary");
    command
        .env("IMPRESARIO_HOME", home)
        .arg("--workspace")
        .arg(workspace)
        .arg("--no-tui")
        .arg("--plain")
        .arg("--no-color");
    command
}

fn run(home: &Path, workspace: &Path, args: &[&str]) -> Output {
    let output = impresario(home, workspace)
        .args(args)
        .output()
        .expect("impresario runs");
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn fixture() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(
        workspace.path(),
        "",
        &[],
        &[("basic.play", PLAY_BASIC), ("perform.play", PERFORM_PLAY)],
    );
    std::fs::create_dir_all(workspace.path().join("theatres")).expect("theatres dir");
    std::fs::write(workspace.path().join("theatres/auth.theatre"), THEATRE).expect("theatre file");
    (home, workspace)
}

fn find<'a>(items: &'a Value, name: &str) -> &'a Value {
    items
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("no entry named {name} in {items}"))
}

fn declared_acts(value: &Value) -> Vec<String> {
    value["plays"]
        .as_array()
        .expect("plays")
        .iter()
        .flat_map(|play| {
            play["acts"]
                .as_array()
                .expect("acts")
                .iter()
                .map(|act| act["name"].as_str().expect("act name").to_string())
        })
        .collect()
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).expect("read file")
}

#[test]
fn list_explorer_tree_and_filters() {
    let (home, workspace) = fixture();

    let listed = run(home.path(), workspace.path(), &["tests", "--json"]);
    assert_eq!(listed.code, EXIT_OK, "stderr={}", listed.stderr);
    let tree: Value = serde_json::from_str(&listed.stdout).expect("tree json");

    let scene = find(&tree["scenes"], "TestScreen");
    assert_eq!(scene["pieces"].as_array().expect("pieces").len(), 3);
    assert_eq!(scene["platforms"][0], "cross-platform");

    let theatre = find(&tree["theatres"], "Auth");
    let login = find(&theatre["stages"], "Login");
    assert_eq!(login["params"][0]["name"], "user");
    assert_eq!(login["params"][0]["required"], false);
    assert_eq!(login["platforms"][0], "web");
    assert_eq!(login["platforms"][1], "android");
    assert_eq!(login["items"][0]["kind"], "scene");
    assert_eq!(login["items"][0]["steps"], 2);
    let web_only = find(&theatre["stages"], "WebOnly");
    assert_eq!(web_only["platforms"][0], "web");

    let perform = find(&tree["plays"], "Perform");
    let act = find(&perform["acts"], "LoginAct");
    assert_eq!(act["tags"][0], "auth");
    assert_eq!(act["tags"][1], "smoke");
    assert_eq!(act["examples"]["columns"][0], "user");
    assert_eq!(act["examples"]["rows"], 2);
    assert_eq!(act["stages"][0]["name"], "Login");
    assert_eq!(act["stages"][0]["steps"], 2);
    assert_eq!(act["steps"], 2);
    assert_eq!(perform["acts"].as_array().expect("acts").len(), 3);

    let text = run(home.path(), workspace.path(), &["tests"]);
    assert_eq!(text.code, EXIT_OK, "stderr={}", text.stderr);
    assert!(text.stdout.contains("Scenes"), "{}", text.stdout);
    assert!(text.stdout.contains("stage Login"), "{}", text.stdout);
    assert!(text.stdout.contains("act LoginAct"), "{}", text.stdout);
    assert!(
        text.stdout.contains("tags=[auth, smoke]"),
        "{}",
        text.stdout
    );

    let tagged = run(
        home.path(),
        workspace.path(),
        &["tests", "--json", "--tag", "smoke"],
    );
    assert_eq!(tagged.code, EXIT_OK, "stderr={}", tagged.stderr);
    let tagged: Value = serde_json::from_str(&tagged.stdout).expect("tagged json");
    assert_eq!(declared_acts(&tagged), vec!["LoginAct"]);

    let excluded = run(
        home.path(),
        workspace.path(),
        &["tests", "--json", "--exclude-tag", "smoke"],
    );
    assert_eq!(excluded.code, EXIT_OK, "stderr={}", excluded.stderr);
    let excluded: Value = serde_json::from_str(&excluded.stdout).expect("excluded json");
    assert_eq!(declared_acts(&excluded), vec!["Pass", "WebAct", "PlainAct"]);

    let act = run(
        home.path(),
        workspace.path(),
        &["tests", "--json", "--act", "PlainAct"],
    );
    assert_eq!(act.code, EXIT_OK, "stderr={}", act.stderr);
    let act: Value = serde_json::from_str(&act.stdout).expect("act json");
    assert_eq!(declared_acts(&act), vec!["PlainAct"]);

    let android = run(
        home.path(),
        workspace.path(),
        &["tests", "--json", "--platform", "android"],
    );
    assert_eq!(android.code, EXIT_OK, "stderr={}", android.stderr);
    let android: Value = serde_json::from_str(&android.stdout).expect("android json");
    assert_eq!(
        declared_acts(&android),
        vec!["Pass", "LoginAct", "PlainAct"]
    );

    let web = run(
        home.path(),
        workspace.path(),
        &["tests", "--json", "--platform", "web"],
    );
    assert_eq!(web.code, EXIT_OK, "stderr={}", web.stderr);
    let web: Value = serde_json::from_str(&web.stdout).expect("web json");
    assert_eq!(
        declared_acts(&web),
        vec!["Pass", "LoginAct", "WebAct", "PlainAct"]
    );

    let only = run(
        home.path(),
        workspace.path(),
        &["tests", "--json", "--play", "perform.play"],
    );
    assert_eq!(only.code, EXIT_OK, "stderr={}", only.stderr);
    let only: Value = serde_json::from_str(&only.stdout).expect("play json");
    assert_eq!(only["plays"].as_array().expect("plays").len(), 1);
    assert_eq!(only["plays"][0]["name"], "Perform");

    let unknown = run(
        home.path(),
        workspace.path(),
        &["tests", "--platform", "symbian"],
    );
    assert_eq!(unknown.code, EXIT_USAGE);
    assert!(
        unknown.stderr.contains("unknown platform"),
        "{}",
        unknown.stderr
    );
}

#[test]
fn new_show_where_grep_and_open() {
    let (home, workspace) = fixture();

    for (kind, name, file) in [
        ("scene", "NewScene", "scenes/NewScene.scene"),
        ("theatre", "NewTheatre", "theatres/NewTheatre.theatre"),
        ("play", "NewPlay", "plays/NewPlay.play"),
    ] {
        let created = run(home.path(), workspace.path(), &["tests", "new", kind, name]);
        assert_eq!(
            created.code, EXIT_OK,
            "kind={kind} stdout={} stderr={}",
            created.stdout, created.stderr
        );
        assert!(
            created.stdout.contains("created"),
            "kind={kind}: {}",
            created.stdout
        );
        assert!(workspace.path().join(file).is_file(), "missing {file}");
    }

    let reloaded = run(home.path(), workspace.path(), &["tests", "--json"]);
    assert_eq!(reloaded.code, EXIT_OK, "stderr={}", reloaded.stderr);
    let tree: Value = serde_json::from_str(&reloaded.stdout).expect("reloaded json");
    assert!(find(&tree["scenes"], "NewScene")["pieces"]
        .as_array()
        .is_some_and(|pieces| pieces.len() == 1));
    assert!(tree["theatres"]
        .as_array()
        .expect("theatres")
        .iter()
        .any(|theatre| theatre["name"] == "NewTheatre"));
    assert!(tree["plays"]
        .as_array()
        .expect("plays")
        .iter()
        .any(|play| play["name"] == "NewPlay"));

    let exists = run(
        home.path(),
        workspace.path(),
        &["tests", "new", "scene", "NewScene"],
    );
    assert_eq!(exists.code, EXIT_USAGE);
    assert!(
        exists.stderr.contains("already exists"),
        "{}",
        exists.stderr
    );
    let forced = run(
        home.path(),
        workspace.path(),
        &["tests", "new", "scene", "NewScene", "--force"],
    );
    assert_eq!(forced.code, EXIT_OK, "stderr={}", forced.stderr);

    let bad_kind = run(
        home.path(),
        workspace.path(),
        &["tests", "new", "act", "Nope"],
    );
    assert_eq!(bad_kind.code, EXIT_USAGE);
    assert!(
        bad_kind.stderr.contains("scene, theatre or play"),
        "{}",
        bad_kind.stderr
    );
    let bad_name = run(
        home.path(),
        workspace.path(),
        &["tests", "new", "scene", "Bad Name"],
    );
    assert_eq!(bad_name.code, EXIT_USAGE);
    assert!(
        bad_name.stderr.contains("not a valid declaration name"),
        "{}",
        bad_name.stderr
    );

    let shown = run(
        home.path(),
        workspace.path(),
        &["tests", "show", "TestScreen"],
    );
    assert_eq!(shown.code, EXIT_OK, "stderr={}", shown.stderr);
    assert!(
        shown.stdout.contains("Scene: TestScreen"),
        "{}",
        shown.stdout
    );
    assert!(shown.stdout.contains("references:"), "{}", shown.stdout);
    assert!(shown.stdout.contains("theatres"), "{}", shown.stdout);

    let missing = run(home.path(), workspace.path(), &["tests", "show", "Nope"]);
    assert_eq!(missing.code, EXIT_USAGE);
    assert!(
        missing.stderr.contains("no declaration"),
        "{}",
        missing.stderr
    );

    let located = run(home.path(), workspace.path(), &["tests", "where", "Login"]);
    assert_eq!(located.code, EXIT_OK, "stderr={}", located.stderr);
    assert!(
        located.stdout.contains("auth.theatre"),
        "{}",
        located.stdout
    );
    assert!(
        located.stdout.contains("perform.play"),
        "{}",
        located.stdout
    );
    assert!(
        located.stdout.contains("Perform: Login"),
        "{}",
        located.stdout
    );

    let nowhere = run(home.path(), workspace.path(), &["tests", "where", "Nope"]);
    assert_eq!(nowhere.code, EXIT_USAGE);
    assert!(
        nowhere.stderr.contains("no occurrence"),
        "{}",
        nowhere.stderr
    );

    let grep = run(home.path(), workspace.path(), &["tests", "grep", "smoke"]);
    assert_eq!(grep.code, EXIT_OK, "stderr={}", grep.stderr);
    assert!(grep.stdout.contains("Tags: auth, smoke"), "{}", grep.stdout);
    let insensitive = run(
        home.path(),
        workspace.path(),
        &["tests", "grep", "-i", "AUTH"],
    );
    assert_eq!(insensitive.code, EXIT_OK, "stderr={}", insensitive.stderr);
    assert!(
        insensitive.stdout.contains("Tags: auth, smoke"),
        "{}",
        insensitive.stdout
    );
    let none = run(home.path(), workspace.path(), &["tests", "grep", "zzzz"]);
    assert_eq!(none.code, EXIT_OK, "stderr={}", none.stderr);
    assert!(none.stdout.contains("no matches"), "{}", none.stdout);

    let unknown = run(home.path(), workspace.path(), &["tests", "open", "Nope"]);
    assert_eq!(unknown.code, EXIT_USAGE);
    assert!(
        unknown.stderr.contains("no declaration or file"),
        "{}",
        unknown.stderr
    );

    let missing_editor = impresario(home.path(), workspace.path())
        .env("VISUAL", "impresario-test-editor-that-does-not-exist")
        .env("EDITOR", "impresario-test-editor-that-does-not-exist")
        .args(["tests", "open", "TestScreen"])
        .output()
        .expect("impresario runs");
    assert_eq!(missing_editor.status.code(), Some(EXIT_INFRA));
    let stderr = String::from_utf8_lossy(&missing_editor.stderr);
    assert!(stderr.contains("cannot start the editor"), "{stderr}");
}

#[test]
fn rename_is_lossless_and_restores_broken_edits() {
    let (home, workspace) = fixture();
    let scene_file: PathBuf = workspace.path().join("scenes/TestScreen.scene");
    let theatre_file = workspace.path().join("theatres/auth.theatre");
    let play_file = workspace.path().join("plays/basic.play");
    let before = read(&scene_file);

    let dry = run(
        home.path(),
        workspace.path(),
        &[
            "tests",
            "rename",
            "scene",
            "TestScreen",
            "Renamed",
            "--dry-run",
        ],
    );
    assert_eq!(dry.code, EXIT_OK, "stderr={}", dry.stderr);
    assert!(dry.stdout.contains("would rename"), "{}", dry.stdout);
    assert!(dry.stdout.contains("- Scene: TestScreen"), "{}", dry.stdout);
    assert!(dry.stdout.contains("+ Scene: Renamed"), "{}", dry.stdout);
    assert_eq!(read(&scene_file), before, "dry run never writes");

    let renamed = run(
        home.path(),
        workspace.path(),
        &["tests", "rename", "scene", "TestScreen", "Renamed"],
    );
    assert_eq!(renamed.code, EXIT_OK, "stderr={}", renamed.stderr);
    assert!(renamed.stdout.contains("renamed"), "{}", renamed.stdout);
    assert!(read(&scene_file).contains("Scene: Renamed"));
    assert!(read(&theatre_file).contains("Scene: Renamed"));
    assert!(!read(&theatre_file).contains("Scene: TestScreen"));
    assert!(
        !read(&play_file).contains("TestScreen"),
        "unrelated file untouched"
    );

    let shown = run(home.path(), workspace.path(), &["tests", "show", "Renamed"]);
    assert_eq!(shown.code, EXIT_OK, "stderr={}", shown.stderr);
    let old = run(
        home.path(),
        workspace.path(),
        &["tests", "show", "TestScreen"],
    );
    assert_eq!(old.code, EXIT_USAGE);

    // A new name with a space breaks the Scene header; every changed file
    // must be restored and the CLI must say so.
    let broken = run(
        home.path(),
        workspace.path(),
        &["tests", "rename", "scene", "Renamed", "Bad Name"],
    );
    assert_eq!(broken.code, EXIT_USAGE, "stderr={}", broken.stderr);
    assert!(
        broken.stderr.contains("all files were restored"),
        "{}",
        broken.stderr
    );
    assert!(read(&scene_file).contains("Scene: Renamed"));
    assert!(read(&theatre_file).contains("Scene: Renamed"));
    assert!(!read(&theatre_file).contains("Bad Name"));

    let bad_kind = run(
        home.path(),
        workspace.path(),
        &["tests", "rename", "widget", "Renamed", "Other"],
    );
    assert_eq!(bad_kind.code, EXIT_USAGE);
    assert!(
        bad_kind.stderr.contains("rename kind"),
        "{}",
        bad_kind.stderr
    );
    let unknown = run(
        home.path(),
        workspace.path(),
        &["tests", "rename", "scene", "Nope", "Other"],
    );
    assert_eq!(unknown.code, EXIT_USAGE);
    assert!(
        unknown.stderr.contains("no declaration"),
        "{}",
        unknown.stderr
    );
}
