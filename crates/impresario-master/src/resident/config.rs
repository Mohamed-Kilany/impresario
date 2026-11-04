//! Resident configuration discovery and lossless editing helpers.

use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use impresario_core::home::Home;
use impresario_core::resident::{ResidentSections, Schedule, Watcher};
use yaml_edit::path::YamlPath as _;
use yaml_edit::{YamlFile, YamlNode, YamlValue};

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

fn write_source(path: &Path, source: &str) -> Result<(), String> {
    // Validate the complete prospective file before replacing it: a hand-edited
    // schedule or watcher must never leave the workspace unparsable.
    ResidentSections::from_yaml(source)
        .map_err(|error| format!("refusing to write {}: {error}", path.display()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, source)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn write_file(path: &Path, file: &YamlFile) -> Result<(), String> {
    write_source(path, &file.to_string())
}

fn sections_before(source: &str, path: &Path) -> Result<ResidentSections, String> {
    if source.trim().is_empty() {
        return Ok(ResidentSections::default());
    }
    ResidentSections::from_yaml(source).map_err(|error| format!("{}: {error}", path.display()))
}

fn node_span(node: &YamlNode) -> Option<(usize, usize)> {
    let range = match node {
        YamlNode::Scalar(value) => value.byte_range(),
        YamlNode::Mapping(value) => value.byte_range(),
        YamlNode::Sequence(value) => value.byte_range(),
        YamlNode::Alias(_) | YamlNode::TaggedNode(_) => return None,
    };
    Some((range.start as usize, range.end as usize))
}

fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |index| index + 1)
}

fn line_indent(source: &str, at: usize) -> usize {
    source[line_start(source, at)..at]
        .chars()
        .take_while(|character| *character == ' ' || *character == '\t')
        .count()
}

fn sequence_dash_indent(source: &str, sequence: &yaml_edit::Sequence) -> usize {
    let Some(first) = sequence.first() else {
        return 2;
    };
    let Some((start, _)) = node_span(&first) else {
        return 2;
    };
    let prefix = &source[line_start(source, start)..start];
    if let Some(dash) = prefix.rfind('-') {
        return prefix[..dash].chars().count();
    }
    // A bare `-` may put the first mapping key on the following line.
    let Some(dash) = source[..start].rfind('-') else {
        return 2;
    };
    let between = &source[dash + 1..start];
    if between
        .chars()
        .all(|character| matches!(character, ' ' | '\t' | '\r' | '\n'))
    {
        return source[line_start(source, dash)..dash].chars().count();
    }
    2
}

/// Render a block mapping as one sequence item at `indent`.
fn block_item(yaml: &str, indent: usize) -> String {
    let yaml = yaml.trim_end_matches(['\r', '\n']);
    let mut lines = yaml.lines();
    let first = lines.next().unwrap_or_default();
    let mut output = " ".repeat(indent);
    output.push_str("- ");
    output.push_str(first);
    let continuation = " ".repeat(indent + 2);
    for line in lines {
        output.push('\n');
        if !line.is_empty() {
            output.push_str(&continuation);
            output.push_str(line);
        }
    }
    output
}

/// Render a block mapping as the value after an existing sequence dash.
fn block_value(yaml: &str, indent: usize) -> String {
    let yaml = yaml.trim_end_matches(['\r', '\n']);
    let mut lines = yaml.lines();
    let first = lines.next().unwrap_or_default();
    let mut output = String::from(first);
    let continuation = " ".repeat(indent);
    for line in lines {
        output.push('\n');
        if !line.is_empty() {
            output.push_str(&continuation);
            output.push_str(line);
        }
    }
    output
}

/// JSON is a flow-style YAML subset, so it can be inserted into a flow
/// sequence without losing the surrounding document.
fn flow_mapping(yaml: &str, path: &Path) -> Result<yaml_edit::Mapping, String> {
    let value = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(yaml)
        .map_err(|error| format!("{}: invalid generated YAML: {error}", path.display()))?;
    let json = serde_json::to_string(&value)
        .map_err(|error| format!("{}: cannot encode flow YAML: {error}", path.display()))?;
    let file = YamlFile::from_str(&json)
        .map_err(|error| format!("{}: cannot encode flow YAML: {error}", path.display()))?;
    file.document()
        .and_then(|document| document.as_mapping())
        .ok_or_else(|| format!("{}: generated YAML is not a mapping", path.display()))
}

/// Append a schedule or watcher while preserving the rest of the YAML file.
pub fn append(path: &Path, kind: Kind, yaml: &str) -> Result<(), String> {
    let source = source_or_empty(path)?;
    let file = parse_file(&source, path)?;
    let mapping = file.document().and_then(|document| document.as_mapping());
    let sequence = mapping
        .as_ref()
        .and_then(|mapping| mapping.get_sequence(kind.key()));
    match sequence {
        Some(sequence) if sequence.is_empty() => {
            let range = sequence.byte_range();
            let mut replace_start = range.start as usize;
            while replace_start > 0 && matches!(source.as_bytes()[replace_start - 1], b' ' | b'\t')
            {
                replace_start -= 1;
            }
            let indent = line_indent(&source, replace_start) + 2;
            let item = block_item(yaml, indent);
            let mut result = String::with_capacity(source.len() + item.len() + 1);
            result.push_str(&source[..replace_start]);
            result.push('\n');
            result.push_str(&item);
            result.push_str(&source[range.end as usize..]);
            write_source(path, &result)
        }
        Some(sequence) if sequence.is_flow_style() => {
            let value = flow_mapping(yaml, path)?;
            sequence.push(value);
            write_file(path, &file)
        }
        Some(sequence) => {
            let end = sequence.byte_range().end as usize;
            let indent = sequence_dash_indent(&source, &sequence);
            let item = block_item(yaml, indent);
            let mut result = String::with_capacity(source.len() + item.len() + 2);
            result.push_str(&source[..end]);
            if !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&item);
            let rest = &source[end..];
            if !rest.is_empty() && !rest.starts_with('\n') && !rest.starts_with('\r') {
                result.push('\n');
            }
            result.push_str(rest);
            if source.ends_with('\n') && !result.ends_with('\n') {
                result.push('\n');
            }
            write_source(path, &result)
        }
        None => {
            if mapping
                .as_ref()
                .is_some_and(|mapping| mapping.contains_key(kind.key()))
            {
                return Err(format!(
                    "{} in {} is not a sequence",
                    kind.key(),
                    path.display()
                ));
            }
            let mut result = source;
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(kind.key());
            result.push_str(":\n");
            result.push_str(&block_item(yaml, 2));
            result.push('\n');
            write_source(path, &result)
        }
    }
}

/// Replace one item by name, preserving surrounding comments.
pub fn replace(path: &Path, kind: Kind, name: &str, yaml: &str) -> Result<(), String> {
    let source = source_or_empty(path)?;
    let sections = sections_before(&source, path)?;
    let index = kind
        .find(&sections, name)
        .ok_or_else(|| format!("no {} named '{name}' in {}", kind.key(), path.display()))?;
    let file = parse_file(&source, path)?;
    let mapping = file.document().and_then(|document| document.as_mapping());
    let sequence = mapping
        .as_ref()
        .and_then(|mapping| mapping.get_sequence(kind.key()))
        .ok_or_else(|| format!("{} in {} is not a sequence", kind.key(), path.display()))?;
    if sequence.is_flow_style() {
        let value = flow_mapping(yaml, path)?;
        if !sequence.set(index, value) {
            return Err(format!(
                "cannot replace {}[{}] in {}",
                kind.key(),
                index,
                path.display()
            ));
        }
        return write_file(path, &file);
    }
    let item = sequence.get(index).ok_or_else(|| {
        format!(
            "cannot replace {}[{}] in {}",
            kind.key(),
            index,
            path.display()
        )
    })?;
    let Some((start, end)) = node_span(&item) else {
        return Err(format!(
            "cannot replace {}[{}] in {}",
            kind.key(),
            index,
            path.display()
        ));
    };
    let value = block_value(yaml, sequence_dash_indent(&source, &sequence) + 2);
    let mut result = String::with_capacity(source.len() + value.len());
    result.push_str(&source[..start]);
    result.push_str(&value);
    result.push_str(&source[end..]);
    write_source(path, &result)
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
