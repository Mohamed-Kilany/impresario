//! Resident configuration discovery and lossless editing helpers.

use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use impresario_core::home::Home;
use impresario_core::resident::{ResidentSections, Schedule, Watcher};
use yaml_edit::path::YamlPath as _;
use yaml_edit::{YamlFile, YamlValue};

/// Which configuration layer a mutating command writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Workspace,
    Global,
}

/// The file a mutating command targets.
#[must_use]
pub fn layer_path(layer: Layer, home: &Home, config_path: &Path) -> PathBuf {
    match layer {
        Layer::Workspace => config_path.to_path_buf(),
        Layer::Global => home.config_path(),
    }
}

/// Read one file's resident sections. A missing file is an empty layer.
pub fn read_layer(path: &Path) -> Result<ResidentSections, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ResidentSections::default());
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if source.trim().is_empty() {
        return Ok(ResidentSections::default());
    }
    ResidentSections::from_yaml(&source).map_err(|error| format!("{}: {error}", path.display()))
}

/// Effective sections: global first, workspace overlays by name.
pub fn load_effective(home: &Home, config_path: &Path) -> Result<ResidentSections, String> {
    let global = read_layer(&home.config_path())?;
    let workspace = read_layer(config_path)?;
    Ok(global.merged(workspace))
}

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Schedule,
    Watcher,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Self::Schedule => "schedules",
            Self::Watcher => "watchers",
        }
    }

    fn item_count(self, sections: &ResidentSections) -> usize {
        match self {
            Self::Schedule => sections.schedules.len(),
            Self::Watcher => sections.watchers.len(),
        }
    }

    fn find(self, sections: &ResidentSections, name: &str) -> Option<usize> {
        match self {
            Self::Schedule => sections
                .schedules
                .iter()
                .position(|value| value.name == name),
            Self::Watcher => sections
                .watchers
                .iter()
                .position(|value| value.name == name),
        }
    }
}

fn source_or_empty(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn parse_file(source: &str, path: &Path) -> Result<YamlFile, String> {
    if source.trim().is_empty() {
        Ok(YamlFile::new())
    } else {
        YamlFile::from_str(source).map_err(|error| format!("{}: {error}", path.display()))
    }
}

fn write_file(path: &Path, file: &YamlFile) -> Result<(), String> {
    // Validate the complete prospective file before replacing it: a hand-edited
    // schedule or watcher must never leave the workspace unparsable.
    ResidentSections::from_yaml(&file.to_string())
        .map_err(|error| format!("refusing to write {}: {error}", path.display()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, file.to_string())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn sections_before(source: &str, path: &Path) -> Result<ResidentSections, String> {
    if source.trim().is_empty() {
        return Ok(ResidentSections::default());
    }
    ResidentSections::from_yaml(source).map_err(|error| format!("{}: {error}", path.display()))
}

/// Append a schedule or watcher while preserving the rest of the YAML file.
pub fn append(path: &Path, kind: Kind, yaml: &str) -> Result<(), String> {
    let source = source_or_empty(path)?;
    let sections = sections_before(&source, path)?;
    let index = kind.item_count(&sections);
    let file = parse_file(&source, path)?;
    let document = file.ensure_document();
    let value = YamlValue::parse_raw(yaml);
    document
        .try_set_path(&format!("{}[{index}]", kind.key()), value)
        .map_err(|error| format!("cannot append to {}: {error}", path.display()))?;
    write_file(path, &file)
}

/// Replace one item by name, preserving surrounding comments.
pub fn replace(path: &Path, kind: Kind, name: &str, yaml: &str) -> Result<(), String> {
    let source = source_or_empty(path)?;
    let sections = sections_before(&source, path)?;
    let index = kind
        .find(&sections, name)
        .ok_or_else(|| format!("no {} named '{name}' in {}", kind.key(), path.display()))?;
    let file = parse_file(&source, path)?;
    let document = file.ensure_document();
    let value = YamlValue::parse_raw(yaml);
    document
        .try_set_path(&format!("{}[{index}]", kind.key()), value)
        .map_err(|error| {
            format!(
                "cannot replace {} in {}: {error}",
                kind.key(),
                path.display()
            )
        })?;
    write_file(path, &file)
}

/// Remove one item by name.
pub fn remove(path: &Path, kind: Kind, name: &str) -> Result<(), String> {
    let source = source_or_empty(path)?;
    let sections = sections_before(&source, path)?;
    let index = kind
        .find(&sections, name)
        .ok_or_else(|| format!("no {} named '{name}' in {}", kind.key(), path.display()))?;
    let file = parse_file(&source, path)?;
    let document = file.ensure_document();
    // `YamlPath::try_remove_path` refuses sequence indexes on purpose
    // (`schedules[0]` would shift every later item). Remove through the
    // sequence handle so the lossless edit is still index-safe.
    let sequence = document
        .get_sequence(kind.key())
        .ok_or_else(|| format!("{} in {} is not a sequence", kind.key(), path.display()))?;
    if sequence.remove(index).is_none() {
        return Err(format!(
            "cannot remove {}[{}] from {}",
            kind.key(),
            index,
            path.display()
        ));
    }
    write_file(path, &file)
}

/// Toggle `enabled` in place.
pub fn set_enabled(path: &Path, kind: Kind, name: &str, enabled: bool) -> Result<(), String> {
    let source = source_or_empty(path)?;
    let sections = sections_before(&source, path)?;
    let index = kind
        .find(&sections, name)
        .ok_or_else(|| format!("no {} named '{name}' in {}", kind.key(), path.display()))?;
    let file = parse_file(&source, path)?;
    let document = file.ensure_document();
    document
        .try_set_path(
            &format!("{}[{index}].enabled", kind.key()),
            YamlValue::parse_raw(if enabled { "true" } else { "false" }),
        )
        .map_err(|error| {
            format!(
                "cannot update {} in {}: {error}",
                kind.key(),
                path.display()
            )
        })?;
    write_file(path, &file)
}

/// Serialize one value as a YAML block without a document marker.
pub fn to_yaml<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let text =
        serde_yaml_ng::to_string(value).map_err(|error| format!("cannot encode YAML: {error}"))?;
    Ok(text)
}

/// Find a schedule by name in effective sections.
#[must_use]
pub fn schedule<'a>(sections: &'a ResidentSections, name: &str) -> Option<&'a Schedule> {
    sections.schedules.iter().find(|value| value.name == name)
}

/// Find a watcher by name in effective sections.
#[must_use]
pub fn watcher<'a>(sections: &'a ResidentSections, name: &str) -> Option<&'a Watcher> {
    sections.watchers.iter().find(|value| value.name == name)
}
