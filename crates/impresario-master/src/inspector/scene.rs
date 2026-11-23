//! Scene panel backend: listing, preview, commit and undo.
//!
//! Every write goes through `impresario_core::scene_edit`, so the inspector
//! inherits the same byte-preserving edits, revision checks and atomic writes
//! as the CLI. Previews are kept in memory by opaque id; committing one
//! re-opens the file and refuses a stale base revision.

use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::MutexGuard;

use impresario_core::ast::SceneFile;
use impresario_core::platforms::Platform;
use impresario_core::scene_edit::{self, SceneDocument, SceneEdit, SceneEditError};
use impresario_inspector_core::snapshot::Snapshot;
use impresario_inspector_core::{resolve_column, ColumnResolution};
use impresario_locator::parse as parse_locator;
use serde::Serialize;
use serde_json::{json, Value};

use super::{ApiError, Shared};

/// One locator cell written by a previewed edit, remembered so the commit can
/// re-resolve it against the live snapshot (TASK-INSPECTOR 9.4, verify after
/// commit).
pub(crate) struct VerifyTarget {
    pub row: String,
    pub column: String,
    pub locator: String,
}

/// One previewed edit waiting for a commit.
pub(crate) struct StoredEdit {
    pub edit: SceneEdit,
    pub created_ms: u64,
    pub targets: Vec<VerifyTarget>,
}

/// The inverse information kept after a commit.
pub(crate) struct UndoRecord {
    pub previous_source: String,
    pub previous_revision: String,
    pub committed_revision: String,
}

/// One inspector-originated mutation for the audit log.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditEntry {
    pub at: String,
    pub action: String,
    pub detail: Value,
}

pub(crate) fn now_ms() -> u64 {
    crate::time_fmt::now_millis()
}

pub(crate) fn record_audit(shared: &Shared, action: &str, detail: Value) {
    let entry = AuditEntry {
        at: crate::time_fmt::rfc3339_utc(now_ms()),
        action: action.to_string(),
        detail,
    };
    if let Ok(mut audit) = shared.audit.lock() {
        audit.push(entry);
        let len = audit.len();
        if len > 200 {
            audit.drain(..len - 200);
        }
    }
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The directory scene paths are relative to.
#[must_use]
pub(crate) fn scene_root(shared: &Shared) -> PathBuf {
    let configured = &shared.config.scenes_dir;
    if configured.is_absolute() {
        configured.clone()
    } else {
        shared.base.join(configured)
    }
}

/// List `.scene` files under the configured scene directory.
pub(crate) fn list(shared: &Shared) -> Result<Value, ApiError> {
    let root = scene_root(shared);
    let mut scenes = Vec::new();
    if root.exists() {
        walk(&root, &root, &mut scenes)?;
    }
    scenes.sort_by(|left, right| {
        let left = left.get("path").and_then(Value::as_str).unwrap_or_default();
        let right = right
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        left.cmp(right)
    });
    Ok(json!({
        "root": display_path(shared, &root),
        "scenes": scenes,
    }))
}

fn walk(root: &Path, directory: &Path, out: &mut Vec<Value>) -> Result<(), ApiError> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        ApiError::internal(format!(
            "cannot read scene directory {}: {error}",
            directory.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| ApiError::internal(error.to_string()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| ApiError::internal(error.to_string()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk(root, &path, out)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("scene") {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path.as_path());
        let name = relative
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        out.push(json!({
            "path": relative.to_string_lossy().replace('\\', "/"),
            "name": name,
            "bytes": entry.metadata().map(|meta| meta.len()).unwrap_or(0),
        }));
    }
    Ok(())
}

fn display_path(shared: &Shared, path: &Path) -> String {
    path.strip_prefix(&shared.base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Open a scene inside the workspace and remember its revision for the file
/// watcher (an external edit arrives as an SSE `scene` update).
pub(crate) fn open(shared: &Shared, path: &str) -> Result<SceneDocument, ApiError> {
    let doc = SceneDocument::open_in_workspace(&shared.base, path).map_err(scene_error)?;
    shared.watch_scene(doc.path(), Some(doc.revision()));
    Ok(doc)
}

/// A scene document plus its metadata for the UI.
pub(crate) fn document(shared: &Shared, path: &str) -> Result<Value, ApiError> {
    let doc = open(shared, path)?;
    Ok(document_json(shared, &doc))
}

fn document_json(shared: &Shared, doc: &SceneDocument) -> Value {
    json!({
        "path": display_path(shared, doc.path()),
        "revision": doc.revision(),
        "base_revision": doc.base_revision(),
        "source": doc.source(),
        "scene": doc.scene(),
        "diagnostics": scene_edit::diagnostics_summary(doc.diagnostics()),
        "has_errors": doc.diagnostics().has_errors(),
        "columns": columns_json(doc.scene()),
    })
}

fn columns_json(scene: &SceneFile) -> Vec<Value> {
    scene
        .columns
        .iter()
        .map(|column| {
            json!({
                "name": &column.name,
                "platform": column.platform.map(Platform::as_str),
            })
        })
        .collect()
}

/// Preview one scene operation. The edit is stored under `edit_id` for a
/// later [`commit`].
pub(crate) fn preview(shared: &Shared, body: &Value) -> Result<Value, ApiError> {
    let path = string_field(body, "path")?;
    let doc = open(shared, &path)?;
    if let Some(expected) = body.get("base_revision").and_then(Value::as_str) {
        if expected != doc.revision() {
            return Err(conflict_error(Some(expected.to_string()), &doc));
        }
    }
    let op = body
        .get("op")
        .and_then(Value::as_object)
        .ok_or_else(|| ApiError::bad_request("the request needs an 'op' object"))?;
    let kind = op
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("'op.kind' is required"))?;

    let platform = op
        .get("platform")
        .and_then(Value::as_str)
        .map(Platform::from_str)
        .transpose()
        .map_err(|_| ApiError::bad_request("'op.platform' is not a known platform"))?;
    let mut used_column: Option<String> = None;
    let mut target: Option<VerifyTarget> = None;

    let edit = match kind {
        "add_column" => {
            let platform = platform
                .ok_or_else(|| ApiError::bad_request("'op.platform' is required for add_column"))?;
            doc.add_column(platform)
        }
        "add_element" => {
            let column = resolve_column_field(&doc, op, platform)?;
            let seed = op
                .get("seed")
                .and_then(Value::as_str)
                .or_else(|| op.get("row").and_then(Value::as_str))
                .unwrap_or("Element");
            let row = op
                .get("row")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| doc.suggest_row_name(seed));
            let locator = string_in(op, "locator")?;
            used_column = Some(column.clone());
            target = Some(VerifyTarget {
                row: row.clone(),
                column: column.clone(),
                locator: locator.clone(),
            });
            doc.add_element(&row, &column, &locator)
        }
        "fill_cell" => {
            let row = string_in(op, "row")?;
            let column = resolve_column_field(&doc, op, platform)?;
            let locator = string_in(op, "locator")?;
            used_column = Some(column.clone());
            target = Some(VerifyTarget {
                row: row.clone(),
                column: column.clone(),
                locator: locator.clone(),
            });
            doc.fill_cell(&row, &column, &locator)
        }
        "replace_cell" => {
            let row = string_in(op, "row")?;
            let column = resolve_column_field(&doc, op, platform)?;
            let locator = string_in(op, "locator")?;
            let confirm = op.get("confirm").and_then(Value::as_bool).unwrap_or(false);
            used_column = Some(column.clone());
            target = Some(VerifyTarget {
                row: row.clone(),
                column: column.clone(),
                locator: locator.clone(),
            });
            doc.replace_cell(&row, &column, &locator, confirm)
        }
        "rename_row" => {
            let row = string_in(op, "row")?;
            let new_name = string_in(op, "new_name")?;
            doc.rename_row(&row, &new_name)
        }
        "remove_row" => {
            let row = string_in(op, "row")?;
            doc.remove_row(&row)
        }
        "validate" => {
            return Ok(json!({
                "ok": true,
                "document": document_json(shared, &doc),
            }))
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "unknown scene operation '{other}'"
            )))
        }
    }
    .map_err(scene_error)?;

    let edit_id = uuid::Uuid::new_v4().to_string();
    let revision = edit.revision().to_string();
    let base_revision = edit.base_revision().map(str::to_string);
    let diff = edit.diff().to_string();
    let source = edit.source().to_string();
    let scene = edit.scene().clone();
    let diagnostics = scene_edit::diagnostics_summary(edit.diagnostics());
    let has_errors = edit.diagnostics().has_errors();
    let created = now_ms();
    {
        let mut edits = lock(&shared.edits);
        edits.retain(|_, stored| created.saturating_sub(stored.created_ms) < 15 * 60 * 1000);
        edits.insert(
            edit_id.clone(),
            StoredEdit {
                edit,
                created_ms: created,
                targets: target.into_iter().collect(),
            },
        );
    }
    Ok(json!({
        "ok": true,
        "edit_id": edit_id,
        "path": display_path(shared, doc.path()),
        "base_revision": base_revision,
        "revision": revision,
        "column": used_column,
        "diff": diff,
        "source": source,
        "scene": scene,
        "columns": columns_json(&scene),
        "diagnostics": diagnostics,
        "has_errors": has_errors,
    }))
}

fn resolve_column_field(
    doc: &SceneDocument,
    op: &serde_json::Map<String, Value>,
    platform: Option<Platform>,
) -> Result<String, ApiError> {
    if let Some(column) = op.get("column").and_then(Value::as_str) {
        if !column.trim().is_empty() {
            return Ok(column.trim().to_string());
        }
    }
    let platform = platform.ok_or_else(|| {
        ApiError::bad_request("provide 'op.column' or 'op.platform' to choose a column")
    })?;
    match resolve_column(doc.scene(), platform) {
        ColumnResolution::Resolved { column, .. } => Ok(column),
        ColumnResolution::Ambiguous { candidates, hint } => Err(ApiError::new(
            422,
            "AMBIGUOUS_COLUMN",
            hint,
            Some(json!({ "candidates": candidates })),
        )),
        ColumnResolution::Missing {
            platform,
            available,
            suggested,
        } => Err(ApiError::new(
            422,
            "MISSING_COLUMN",
            format!(
                "the scene has no column for '{}'; add '{suggested}'",
                platform.as_str()
            ),
            Some(json!({
                "platform": platform.as_str(),
                "available": available,
                "suggested": suggested,
            })),
        )),
    }
}

/// Commit one previewed edit.
pub(crate) fn commit(shared: &Shared, body: &Value) -> Result<Value, ApiError> {
    let edit_id = string_field(body, "edit_id")?;
    let stored = lock(&shared.edits).remove(&edit_id);
    let Some(stored) = stored else {
        return Err(ApiError::not_found(format!(
            "unknown or expired edit '{edit_id}'"
        )));
    };
    let mut doc = SceneDocument::open(stored.edit.path()).map_err(scene_error)?;
    let previous_source = doc.source().to_string();
    let previous_revision = doc.revision().to_string();
    // Hold the watch lock across the write: the scanner must never sample the
    // file between the old and new revisions, or it would report the
    // inspector's own atomic rename as an external edit.
    let mut write = shared.scene_watch.begin_write();
    let commit = match doc.commit(&stored.edit) {
        Ok(commit) => {
            write.record(&commit.path, Some(&commit.revision));
            commit
        }
        Err(error) => {
            let disk = doc.disk_revision().ok().flatten();
            write.record(doc.path(), disk.as_deref());
            return Err(scene_error(error));
        }
    };
    drop(write);
    let path = commit.path.clone();
    let verification = verify_targets(shared.latest_snapshot().as_deref(), &stored.targets);
    lock(&shared.undo).insert(
        path.clone(),
        UndoRecord {
            previous_source,
            previous_revision,
            committed_revision: commit.revision.clone(),
        },
    );
    record_audit(
        shared,
        "scene.commit",
        json!({
            "path": display_path(shared, &path),
            "revision": &commit.revision,
            "verification": verification.get("status"),
        }),
    );
    Ok(json!({
        "ok": true,
        "path": display_path(shared, &path),
        "revision": commit.revision,
        "diff": stored.edit.diff(),
        "verification": verification,
    }))
}

/// Undo the last committed edit of one scene.
pub(crate) fn undo(shared: &Shared, body: &Value) -> Result<Value, ApiError> {
    let path = string_field(body, "path")?;
    let mut doc = open(shared, &path)?;
    let key = doc.path().to_path_buf();
    let record = lock(&shared.undo).remove(&key);
    let Some(record) = record else {
        return Err(ApiError::not_found(format!(
            "there is no inspector edit to undo for '{path}'"
        )));
    };
    let mut write = shared.scene_watch.begin_write();
    let commit = match doc.restore(&record.previous_source, Some(&record.committed_revision)) {
        Ok(commit) => {
            write.record(&key, Some(&commit.revision));
            commit
        }
        Err(error) => {
            let disk = doc.disk_revision().ok().flatten();
            write.record(&key, disk.as_deref());
            return Err(scene_error(error));
        }
    };
    drop(write);
    record_audit(
        shared,
        "scene.undo",
        json!({
            "path": display_path(shared, &key),
            "revision": &commit.revision,
            "restored_revision": record.previous_revision,
        }),
    );
    Ok(json!({
        "ok": true,
        "path": display_path(shared, &key),
        "revision": commit.revision,
    }))
}

/// Create a new scene skeleton and write it immediately.
pub(crate) fn create(shared: &Shared, body: &Value) -> Result<Value, ApiError> {
    let path = string_field(body, "path")?;
    let name = string_field(body, "name")?;
    let platforms = body
        .get("platforms")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(Platform::from_str)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(|_| ApiError::bad_request("'platforms' contains an unknown platform"))?
        .unwrap_or_default();
    let mut doc = SceneDocument::create_in_workspace(&shared.base, &path, &name, &platforms)
        .map_err(scene_error)?;
    let source = doc.source().to_string();
    let mut write = shared.scene_watch.begin_write();
    let commit = match doc.restore(&source, None) {
        Ok(commit) => {
            write.record(&commit.path, Some(&commit.revision));
            commit
        }
        Err(error) => {
            let disk = doc.disk_revision().ok().flatten();
            write.record(doc.path(), disk.as_deref());
            return Err(scene_error(error));
        }
    };
    drop(write);
    record_audit(
        shared,
        "scene.create",
        json!({
            "path": display_path(shared, &commit.path),
            "revision": &commit.revision,
        }),
    );
    Ok(json!({
        "ok": true,
        "path": display_path(shared, &commit.path),
        "revision": commit.revision,
        "source": source,
    }))
}

/// Re-resolve the locator cells written by a committed edit against the live
/// snapshot (TASK-INSPECTOR 9.4, "verify after commit").
///
/// Verification is advisory: the commit has already succeeded, so a missing
/// device or a locator that no longer matches returns a warning-shaped result
/// instead of failing the request. `not_applicable` is returned for edits that
/// write no locator cell (rename, remove, add column).
fn verify_targets(snapshot: Option<&Snapshot>, targets: &[VerifyTarget]) -> Value {
    if targets.is_empty() {
        return json!({
            "status": "not_applicable",
            "checks": [],
        });
    }
    let Some(snapshot) = snapshot else {
        return json!({
            "status": "unavailable",
            "reason": "no snapshot has been acquired; attach a device to verify the written locator",
            "checks": targets
                .iter()
                .map(|target| json!({
                    "row": &target.row,
                    "column": &target.column,
                    "locator": &target.locator,
                }))
                .collect::<Vec<_>>(),
        });
    };

    let mut checks = Vec::with_capacity(targets.len());
    let mut all_valid = true;
    let mut all_unique = true;
    for target in targets {
        let check = match parse_locator(&target.locator) {
            Ok(locator) => match snapshot.resolve(&locator) {
                Ok(matches) => {
                    let count = matches.len();
                    let unique = count == 1;
                    all_unique &= unique;
                    json!({
                        "row": &target.row,
                        "column": &target.column,
                        "locator": &target.locator,
                        "count": count,
                        "unique": unique,
                        "visible": matches.first().is_some_and(|entry| entry.visible),
                        "matches": matches,
                    })
                }
                Err(error) => {
                    all_valid = false;
                    json!({
                        "row": &target.row,
                        "column": &target.column,
                        "locator": &target.locator,
                        "error": error.to_string(),
                    })
                }
            },
            Err(error) => {
                all_valid = false;
                json!({
                    "row": &target.row,
                    "column": &target.column,
                    "locator": &target.locator,
                    "error": error.to_string(),
                })
            }
        };
        checks.push(check);
    }
    let status = if !all_valid {
        "invalid"
    } else if !all_unique {
        "warning"
    } else {
        "verified"
    };
    json!({
        "status": status,
        "rev": snapshot.rev,
        "checks": checks,
    })
}

fn string_field(value: &Value, name: &str) -> Result<String, ApiError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request(format!("'{name}' is required")))
}

fn string_in(map: &serde_json::Map<String, Value>, name: &str) -> Result<String, ApiError> {
    map.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request(format!("'{name}' is required")))
}

fn conflict_error(expected: Option<String>, doc: &SceneDocument) -> ApiError {
    ApiError::new(
        409,
        "SCENE_CONFLICT",
        "the scene changed on disk since it was read",
        Some(json!({
            "expected": expected,
            "actual": doc.revision(),
            "fresh_source": doc.source(),
        })),
    )
}

fn scene_error(error: SceneEditError) -> ApiError {
    let code = error.code();
    let status = match &error {
        SceneEditError::PathOutsideWorkspace { .. } => 403,
        SceneEditError::Conflict { .. }
        | SceneEditError::ConfirmationRequired { .. }
        | SceneEditError::AlreadyExists { .. } => 409,
        SceneEditError::Io { .. } => 500,
        _ => 422,
    };
    let details = match &error {
        SceneEditError::Conflict {
            expected,
            actual,
            fresh_source,
        } => Some(json!({
            "expected": expected,
            "actual": actual,
            "fresh_source": fresh_source,
        })),
        SceneEditError::ConfirmationRequired { diff } => Some(json!({ "diff": diff })),
        SceneEditError::UnknownColumn { available, .. } => Some(json!({ "available": available })),
        SceneEditError::UnknownRow { available, .. } => Some(json!({ "available": available })),
        SceneEditError::Invalid { diagnostics } => {
            Some(json!({ "diagnostics": scene_edit::diagnostics_summary(diagnostics) }))
        }
        _ => None,
    };
    ApiError::new(status, code, error.to_string(), details)
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_inspector_core::snapshot::{ScreenInfo, SnapshotNode};
    use impresario_locator::{Rect, Role};

    fn snapshot_with_play() -> Snapshot {
        let screen = ScreenInfo::logical((108.0, 240.0));
        let nodes = vec![
            SnapshotNode::new(
                1,
                0,
                Role::Generic,
                "Screen",
                Rect::new(0.0, 0.0, 108.0, 240.0),
            ),
            SnapshotNode::new(2, 1, Role::Button, "Play", Rect::new(8.0, 8.0, 100.0, 30.0))
                .with_test_id("play"),
        ];
        Snapshot::new("phone", "phone", Platform::Android, screen, nodes)
    }

    fn target(locator: &str) -> VerifyTarget {
        VerifyTarget {
            row: "Play".to_string(),
            column: "android".to_string(),
            locator: locator.to_string(),
        }
    }

    #[test]
    fn verification_reports_verified_warning_and_unavailable() {
        let empty = verify_targets(None, &[]);
        assert_eq!(empty["status"], "not_applicable");

        let unavailable = verify_targets(None, &[target("testid=play")]);
        assert_eq!(unavailable["status"], "unavailable");
        assert_eq!(unavailable["checks"][0]["locator"], "testid=play");

        let snapshot = snapshot_with_play();
        let verified = verify_targets(Some(&snapshot), &[target("testid=play")]);
        assert_eq!(verified["status"], "verified");
        assert_eq!(verified["rev"], 1);
        assert_eq!(verified["checks"][0]["count"], 1);
        assert_eq!(verified["checks"][0]["unique"], true);

        let missing = verify_targets(Some(&snapshot), &[target("testid=missing")]);
        assert_eq!(missing["status"], "warning");
        assert_eq!(missing["checks"][0]["count"], 0);

        let invalid = verify_targets(Some(&snapshot), &[target("role=unicorn")]);
        assert_eq!(invalid["status"], "invalid");
        assert!(invalid["checks"][0]["error"].is_string());
    }

    #[test]
    fn verification_warns_when_a_locator_is_ambiguous() {
        let screen = ScreenInfo::logical((108.0, 240.0));
        let nodes = vec![
            SnapshotNode::new(
                1,
                0,
                Role::Generic,
                "Screen",
                Rect::new(0.0, 0.0, 108.0, 240.0),
            ),
            SnapshotNode::new(2, 1, Role::Button, "Play", Rect::new(8.0, 8.0, 100.0, 30.0))
                .with_test_id("play"),
            SnapshotNode::new(
                3,
                1,
                Role::Button,
                "Play",
                Rect::new(8.0, 44.0, 100.0, 30.0),
            )
            .with_test_id("play"),
        ];
        let snapshot = Snapshot::new("phone", "phone", Platform::Android, screen, nodes);
        let result = verify_targets(Some(&snapshot), &[target("testid=play")]);
        assert_eq!(result["status"], "warning");
        assert_eq!(result["checks"][0]["count"], 2);
        assert_eq!(result["checks"][0]["unique"], false);
    }

    #[test]
    fn scene_errors_carry_stable_codes() {
        let error = scene_error(SceneEditError::MissingTable);
        assert_eq!(error.code, "SCENE_INVALID");
        assert_eq!(error.status, 422);
        let conflict = scene_error(SceneEditError::Conflict {
            expected: Some("a".to_string()),
            actual: Some("b".to_string()),
            fresh_source: None,
        });
        assert_eq!(conflict.status, 409);
        assert_eq!(conflict.code, "SCENE_CONFLICT");
    }
}
