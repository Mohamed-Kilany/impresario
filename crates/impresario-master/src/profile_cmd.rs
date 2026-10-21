//! Saved run profiles (TASK-2 3.4).
//!
//! Profiles live under the top-level `profiles:` mapping of the global
//! `~/.impresario/config.yaml`. The settings schema deliberately ignores that
//! section; this module owns its shape. Profiles store selectors and run
//! options only, never resolved secrets.

use std::collections::BTreeMap;
use std::path::Path;

use clap::{Args, Subcommand};
use impresario_core::home::Home;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

use crate::cli::{RunArgs, EXIT_INFRA, EXIT_OK, EXIT_USAGE};

#[derive(Debug, Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Debug, Subcommand)]
// The `Save` variant carries the whole flag set; keeping it inline makes the
// clap derive readable and the enum is only built once per process.
#[allow(clippy::large_enum_variant)]
pub enum ProfileCommand {
    /// Save (or replace) a named profile.
    Save {
        /// Profile name.
        name: String,
        #[command(flatten)]
        options: SaveArgs,
    },
    /// List saved profile names.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Print one profile as YAML.
    Show {
        /// Profile name.
        name: String,
    },
    /// Delete one profile.
    #[command(visible_alias = "rm")]
    Delete {
        /// Profile name.
        name: String,
    },
}

#[derive(Debug, Args, Clone, Default)]
pub struct SaveArgs {
    /// Play file, directory, glob or name (repeatable).
    #[arg(long, value_name = "FILE|GLOB|NAME")]
    pub play: Vec<String>,
    /// App definition to bind.
    #[arg(long)]
    pub app: Option<String>,
    /// Device name (repeatable).
    #[arg(long)]
    pub device: Vec<String>,
    /// Platform filter (repeatable or comma list).
    #[arg(long)]
    pub platform: Vec<String>,
    /// Tag filter (repeatable).
    #[arg(long)]
    pub tag: Vec<String>,
    /// Excluded tag (repeatable).
    #[arg(long)]
    pub exclude_tag: Vec<String>,
    /// Act name filter (repeatable).
    #[arg(long)]
    pub act: Vec<String>,
    /// Variable override K=V (repeatable).
    #[arg(long, value_name = "K=V")]
    pub var: Vec<String>,
    /// Act parameter override ACT.PARAM=VALUE (repeatable).
    #[arg(long = "arg", value_name = "ACT.PARAM=VALUE")]
    pub arg: Vec<String>,
    /// Default command timeout in ms.
    #[arg(long)]
    pub timeout: Option<u64>,
    /// Retry failed acts this many times.
    #[arg(long)]
    pub retries: Option<u32>,
    /// Reporters, comma separated.
    #[arg(long)]
    pub reporter: Option<String>,
    /// Stop at the first failure.
    #[arg(long)]
    pub fail_fast: bool,
    /// Only failed acts from the newest report.
    #[arg(long)]
    pub only_failed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub play: Vec<String>,
    pub app: Option<String>,
    pub device: Vec<String>,
    pub platform: Vec<String>,
    pub tag: Vec<String>,
    pub exclude_tag: Vec<String>,
    pub act: Vec<String>,
    pub var: Vec<String>,
    pub arg: Vec<String>,
    pub timeout: Option<u64>,
    pub retries: Option<u32>,
    pub reporter: Option<String>,
    pub fail_fast: bool,
    pub only_failed: bool,
}

impl From<SaveArgs> for Profile {
    fn from(args: SaveArgs) -> Self {
        Self {
            play: args.play,
            app: args.app,
            device: args.device,
            platform: args.platform,
            tag: args.tag,
            exclude_tag: args.exclude_tag,
            act: args.act,
            var: args.var,
            arg: args.arg,
            timeout: args.timeout,
            retries: args.retries,
            reporter: args.reporter,
            fail_fast: args.fail_fast,
            only_failed: args.only_failed,
        }
    }
}

fn empty_mapping() -> Mapping {
    Mapping::new()
}

fn read_root(path: &Path) -> Result<Mapping, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(empty_mapping()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if source.trim().is_empty() {
        return Ok(empty_mapping());
    }
    let value: Value = serde_yaml_ng::from_str(&source)
        .map_err(|error| format!("{} is not valid YAML: {error}", path.display()))?;
    match value {
        Value::Mapping(mapping) => Ok(mapping),
        Value::Null => Ok(empty_mapping()),
        _ => Err(format!("{} must be a YAML mapping", path.display())),
    }
}

fn read_profiles(path: &Path) -> Result<BTreeMap<String, Profile>, String> {
    let root = read_root(path)?;
    let Some(value) = root.get(Value::String("profiles".to_string())) else {
        return Ok(BTreeMap::new());
    };
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let Some(mapping) = value.as_mapping() else {
        return Err("`profiles` must be a mapping of name -> profile".to_string());
    };
    let mut profiles = BTreeMap::new();
    for (key, value) in mapping {
        let Some(name) = key.as_str() else {
            return Err("profile names must be strings".to_string());
        };
        let profile: Profile = serde_yaml_ng::from_value(value.clone())
            .map_err(|error| format!("profile '{name}' is invalid: {error}"))?;
        profiles.insert(name.to_string(), profile);
    }
    Ok(profiles)
}

fn reject_tags(value: &Value) -> Result<(), String> {
    match value {
        Value::Tagged(tagged) => Err(format!(
            "profile values must not use the YAML tag {}",
            tagged.tag
        )),
        Value::Sequence(items) => items.iter().try_for_each(reject_tags),
        Value::Mapping(mapping) => {
            for (key, value) in mapping {
                reject_tags(key)?;
                reject_tags(value)?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

/// Render the profiles as a YAML block body (no `profiles:` key), indented by
/// two spaces and terminated by a newline.
fn render_profile_block(profiles: &BTreeMap<String, Profile>) -> Result<String, String> {
    let value = serde_yaml_ng::to_value(profiles)
        .map_err(|error| format!("cannot encode the profiles: {error}"))?;
    reject_tags(&value)?;
    let text = serde_yaml_ng::to_string(&value)
        .map_err(|error| format!("cannot encode the profiles: {error}"))?;
    let mut block = String::new();
    for line in text.lines() {
        if line.is_empty() {
            block.push('\n');
        } else {
            block.push_str("  ");
            block.push_str(line);
            block.push('\n');
        }
    }
    Ok(block)
}

fn line_content(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

/// The top-level `profiles:` key and the line range it owns. The block ends at
/// the next top-level mapping key; indented lines, blank lines and comments
/// belong to the section.
fn profiles_block(lines: &[&str]) -> Option<(usize, usize)> {
    let start = lines.iter().position(|line| {
        let content = line_content(line);
        content == "profiles:"
            || content
                .strip_prefix("profiles:")
                .is_some_and(|rest| rest.starts_with([' ', '\t', '#']))
    })?;
    let inline = line_content(lines.get(start)?)
        .strip_prefix("profiles:")
        .is_some_and(|rest| {
            let rest = rest.trim();
            !rest.is_empty() && !rest.starts_with('#')
        });
    if inline {
        return Some((start, start + 1));
    }
    let mut end = start + 1;
    while let Some(line) = lines.get(end) {
        let content = line_content(line);
        if content.is_empty() || content.starts_with([' ', '\t', '#']) {
            end += 1;
        } else {
            break;
        }
    }
    Some((start, end))
}

fn write_profiles(path: &Path, profiles: &BTreeMap<String, Profile>) -> Result<(), String> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if !source.trim().is_empty() {
        serde_yaml_ng::from_str::<Value>(&source)
            .map_err(|error| format!("{} is not valid YAML: {error}", path.display()))?;
    }
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let block = if profiles.is_empty() {
        String::new()
    } else {
        let block = render_profile_block(profiles)?;
        if newline == "\r\n" {
            block.replace('\n', "\r\n")
        } else {
            block
        }
    };

    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let text = match profiles_block(&lines) {
        Some((start, end)) => {
            let mut text = String::with_capacity(source.len() + block.len());
            let head = lines
                .get(..start)
                .ok_or_else(|| "internal error: profile block start is out of range".to_string())?;
            let tail = lines
                .get(end..)
                .ok_or_else(|| "internal error: profile block end is out of range".to_string())?;
            text.push_str(&head.concat());
            if !profiles.is_empty() {
                text.push_str("profiles:");
                text.push_str(newline);
                text.push_str(&block);
            }
            text.push_str(&tail.concat());
            text
        }
        None if profiles.is_empty() => source.clone(),
        None => {
            let mut text = source.clone();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push_str(newline);
            }
            if !text.is_empty() && !text.ends_with("\n\n") {
                text.push_str(newline);
            }
            text.push_str("profiles:");
            text.push_str(newline);
            text.push_str(&block);
            text
        }
    };

    // The candidate is parsed before the atomic replace. A bug in the block
    // splice must never leave an unreadable global config behind.
    let parsed: Value = serde_yaml_ng::from_str(&text).map_err(|error| {
        format!(
            "refusing to write invalid YAML to {}: {error}",
            path.display()
        )
    })?;
    if !profiles.is_empty() {
        let expected = serde_yaml_ng::to_value(profiles)
            .map_err(|error| format!("cannot encode the profiles: {error}"))?;
        if parsed.get("profiles") != Some(&expected) {
            return Err(format!(
                "refusing to write {}: the edited profiles do not round-trip",
                path.display()
            ));
        }
    } else if parsed
        .as_mapping()
        .is_some_and(|mapping| mapping.contains_key(Value::String("profiles".to_string())))
    {
        return Err(format!(
            "refusing to write {}: an empty profiles section could not be removed",
            path.display()
        ));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension("yaml.tmp");
    std::fs::write(&temporary, text)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    // Windows cannot rename over an existing file, so move the old file out
    // of the way first. A crash between the two renames leaves the `.bak`.
    let backup = path.with_extension("yaml.bak");
    let had_existing = path.exists();
    if had_existing {
        if let Err(error) = std::fs::rename(path, &backup) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("cannot replace {}: {error}", path.display()));
        }
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        if had_existing {
            let _ = std::fs::rename(&backup, path);
        }
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    if had_existing {
        let _ = std::fs::remove_file(&backup);
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | '.'))
}

pub fn run(home: &Home, args: &ProfileArgs) -> i32 {
    let path = home.config_path();
    match &args.command {
        ProfileCommand::Save { name, options } => {
            if !valid_name(name) {
                eprintln!("profile: names may contain letters, digits, '-', '_' and '.'");
                return EXIT_USAGE;
            }
            let mut profiles = match read_profiles(&path) {
                Ok(profiles) => profiles,
                Err(error) => {
                    eprintln!("profile: {error}");
                    return EXIT_INFRA;
                }
            };
            profiles.insert(name.clone(), Profile::from(options.clone()));
            match write_profiles(&path, &profiles) {
                Ok(()) => {
                    println!("saved profile '{name}' in {}", path.display());
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("profile: {error}");
                    EXIT_INFRA
                }
            }
        }
        ProfileCommand::List { json } => {
            let profiles = match read_profiles(&path) {
                Ok(profiles) => profiles,
                Err(error) => {
                    eprintln!("profile: {error}");
                    return EXIT_INFRA;
                }
            };
            if *json {
                let names: Vec<&String> = profiles.keys().collect();
                match serde_json::to_string_pretty(&names) {
                    Ok(text) => println!("{text}"),
                    Err(error) => {
                        eprintln!("profile: cannot serialize names: {error}");
                        return EXIT_INFRA;
                    }
                }
            } else if profiles.is_empty() {
                println!("no profiles");
            } else {
                for name in profiles.keys() {
                    println!("{name}");
                }
            }
            EXIT_OK
        }
        ProfileCommand::Show { name } => {
            let profiles = match read_profiles(&path) {
                Ok(profiles) => profiles,
                Err(error) => {
                    eprintln!("profile: {error}");
                    return EXIT_INFRA;
                }
            };
            let Some(profile) = profiles.get(name) else {
                eprintln!("profile: unknown profile '{name}'");
                return EXIT_USAGE;
            };
            match serde_yaml_ng::to_string(profile) {
                Ok(text) => {
                    print!("{text}");
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("profile: cannot encode '{name}': {error}");
                    EXIT_INFRA
                }
            }
        }
        ProfileCommand::Delete { name } => {
            let mut profiles = match read_profiles(&path) {
                Ok(profiles) => profiles,
                Err(error) => {
                    eprintln!("profile: {error}");
                    return EXIT_INFRA;
                }
            };
            if profiles.remove(name).is_none() {
                eprintln!("profile: unknown profile '{name}'");
                return EXIT_USAGE;
            }
            match write_profiles(&path, &profiles) {
                Ok(()) => {
                    println!("deleted profile '{name}'");
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("profile: {error}");
                    EXIT_INFRA
                }
            }
        }
    }
}

/// Apply a saved profile to `args`. Explicit command-line values win over the
/// profile; the profile only fills in what the user left empty.
pub fn apply(home: &Home, name: &str, args: &mut RunArgs) -> Result<(), String> {
    let profiles = read_profiles(&home.config_path())?;
    let Some(profile) = profiles.get(name) else {
        return Err(format!(
            "unknown profile '{name}' (see `impresario profile list`)"
        ));
    };
    if args.play.is_empty() {
        args.play = profile.play.clone();
    }
    if args.app.is_none() {
        args.app = profile.app.clone();
    }
    if args.device.is_empty() {
        args.device = profile.device.clone();
    }
    if args.platform.is_empty() {
        args.platform = profile.platform.clone();
    }
    if args.tag.is_empty() {
        args.tag = profile.tag.clone();
    }
    if args.exclude_tag.is_empty() {
        args.exclude_tag = profile.exclude_tag.clone();
    }
    if args.act.is_empty() {
        args.act = profile.act.clone();
    }
    if args.var.is_empty() {
        args.var = profile.var.clone();
    }
    if args.arg.is_empty() {
        args.arg = profile.arg.clone();
    }
    if args.timeout.is_none() {
        args.timeout = profile.timeout;
    }
    if args.retries.is_none() {
        args.retries = profile.retries;
    }
    if args.reporter.is_none() {
        args.reporter = profile.reporter.clone();
    }
    args.fail_fast |= profile.fail_fast;
    args.only_failed |= profile.only_failed;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_are_restricted() {
        assert!(valid_name("smoke"));
        assert!(valid_name("ci.android_tv"));
        assert!(!valid_name(""));
        assert!(!valid_name("two words"));
        assert!(!valid_name("a/b"));
    }

    fn temp_config(tag: &str, source: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "impresario-profile-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.yaml");
        std::fs::write(&path, source).expect("write config");
        path
    }

    #[test]
    fn profiles_are_spliced_into_the_existing_document() {
        let path = temp_config(
            "splice",
            "# top comment\nproject: demo\nui:\n  theme: dark  # keep\nprofiles:\n  old:\n    device:\n      - web1\nother: 1\n",
        );
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "ci".to_string(),
            Profile {
                device: vec!["web2".to_string()],
                play: vec!["smoke.play".to_string()],
                fail_fast: true,
                ..Profile::default()
            },
        );
        profiles.insert("old".to_string(), Profile::default());
        write_profiles(&path, &profiles).expect("writes");
        let text = std::fs::read_to_string(&path).expect("config");
        assert!(text.contains("# top comment"), "{text}");
        assert!(text.contains("theme: dark  # keep"), "{text}");
        assert!(text.contains("other: 1"), "{text}");
        let value: Value = serde_yaml_ng::from_str(&text).expect("valid");
        assert_eq!(value["profiles"]["ci"]["device"][0], "web2");
        assert_eq!(value["profiles"]["ci"]["fail_fast"], true);
        assert!(value["profiles"]["old"].is_mapping());

        write_profiles(&path, &BTreeMap::new()).expect("removes");
        let text = std::fs::read_to_string(&path).expect("config");
        assert!(!text.contains("profiles:"), "{text}");
        assert!(text.contains("# top comment"), "{text}");
        assert!(text.contains("other: 1"), "{text}");
        let value: Value = serde_yaml_ng::from_str(&text).expect("valid");
        assert_eq!(value["project"], "demo");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn profiles_are_inserted_when_the_section_is_missing() {
        let path = temp_config("insert", "project: demo\n");
        let mut profiles = BTreeMap::new();
        profiles.insert("ci".to_string(), Profile::default());
        write_profiles(&path, &profiles).expect("writes");
        let text = std::fs::read_to_string(&path).expect("config");
        assert!(
            text.starts_with("project: demo\n\nprofiles:\n  ci:\n"),
            "{text}"
        );
        let value: Value = serde_yaml_ng::from_str(&text).expect("valid");
        assert!(value["profiles"]["ci"].is_mapping());
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn profiles_are_replaced_in_flow_style_and_crlf_is_preserved() {
        let path = temp_config(
            "flow",
            "project: demo\r\nprofiles: {old: {device: [web1]}}\r\nother: 1\r\n",
        );
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "new".to_string(),
            Profile {
                device: vec!["web2".to_string()],
                ..Profile::default()
            },
        );
        write_profiles(&path, &profiles).expect("writes");
        let bytes = std::fs::read(&path).expect("config");
        assert!(bytes.windows(2).any(|pair| pair == b"\r\n"));
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                assert!(
                    index > 0 && bytes[index - 1] == b'\r',
                    "every line keeps CRLF"
                );
            }
        }
        let text = String::from_utf8(bytes).expect("utf8");
        let value: Value = serde_yaml_ng::from_str(&text).expect("valid");
        assert_eq!(value["profiles"]["new"]["device"][0], "web2");
        assert!(value["profiles"].get("old").is_none());
        assert_eq!(value["other"], 1);
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn tagged_profile_values_are_rejected() {
        let tagged = Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: serde_yaml_ng::value::Tag::new("!secret"),
            value: Value::String("x".to_string()),
        }));
        assert!(reject_tags(&tagged).is_err());
        assert!(reject_tags(&Value::String("plain".to_string())).is_ok());
    }
}
