//! Interactive application state, input handling and child command dispatch.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use impresario_core::platforms::Platform;
use impresario_core::probe;
use impresario_core::settings::{self, SettingValue};
use impresario_core::workspace;

use crate::cli;
use crate::definitions::{AppEntry, DefinitionSource, GroupInfo};
use crate::exec::ExecEvent;
use crate::jobs::{self, JobRecord, JobStatus, JobStore};
use crate::report::Status;
use crate::resident::status::{self as resident_status, DumpEntry, ResidentStatus};

use super::{Action, Context, Focus, Screen};

const REFRESH: Duration = Duration::from_millis(250);
const RESIDENT_REFRESH: Duration = Duration::from_secs(2);
const HISTORY_FALLBACK: usize = 1000;
const ACTIVITY_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum LineStatus {
    #[default]
    Running,
    Passed,
    Failed,
    Skipped,
    Aborted,
    Unverifiable,
    /// A non-status line carrying failure evidence (probe/reason/observed).
    Detail,
}

#[derive(Debug, Clone)]
pub(super) struct TreeLine {
    pub indent: usize,
    pub text: String,
    pub status: LineStatus,
}

#[derive(Debug, Clone, Default)]
pub(super) struct TestRow {
    pub indent: usize,
    pub label: String,
    pub kind: TestKind,
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub name: String,
    /// Platform audience of a stage row, from `tests --json`.
    pub platforms: Vec<String>,
}

/// One stage row of the theatre coverage matrix.
#[derive(Debug, Clone, Default)]
pub(super) struct CoverageRow {
    pub stage: String,
    pub theatres: Vec<String>,
    pub implementations: usize,
    pub platforms: BTreeSet<String>,
    /// Configured devices whose platform has no implementation of this stage.
    pub missing: Vec<String>,
}

/// One configured device's plan-time probe support.
#[derive(Debug, Clone)]
pub(super) struct ProbeRow {
    pub device: String,
    pub platform: String,
    pub showing: bool,
    pub player: bool,
    pub drm: bool,
    pub strongest: String,
    pub properties: usize,
    pub providers: Vec<String>,
    pub sources: Vec<ProbeSourceRow>,
    /// Set when the configured platform cannot be resolved.
    pub error: Option<String>,
}

/// Modal state for adding, editing or removing an actor group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GroupEditorKind {
    Add,
    Edit,
    Remove,
}

#[derive(Debug, Clone)]
pub(super) struct GroupEditor {
    pub kind: GroupEditorKind,
    pub name: String,
    pub members: String,
    pub members_focus: bool,
    pub source: DefinitionSource,
}

/// Shared shape of the definition forms (the Groups form predates it and stays
/// on its own state to avoid changing its key handling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FormKind {
    Add,
    Edit,
    Remove,
}

impl FormKind {
    fn verb(self, add: &'static str, edit: &'static str) -> &'static str {
        match self {
            Self::Add => add,
            Self::Edit => edit,
            Self::Remove => "remove",
        }
    }
}

/// Modal state for the Devices screen.
#[derive(Debug, Clone)]
pub(super) struct DeviceEditor {
    pub kind: FormKind,
    pub name: String,
    pub device_type: String,
    pub ip: String,
    pub port: String,
    pub field: usize,
    pub source: DefinitionSource,
}

/// Modal state for the Apps screen.
#[derive(Debug, Clone)]
pub(super) struct AppEditor {
    pub kind: FormKind,
    pub name: String,
    pub description: String,
    pub platforms: String,
    pub packages: String,
    pub launch_activities: String,
    pub launch_params: String,
    pub clear_data: bool,
    pub field: usize,
    pub source: DefinitionSource,
}

/// Modal state for the Config screen.
#[derive(Debug, Clone)]
pub(super) struct ConfigEditor {
    pub kind: FormKind,
    pub key: String,
    pub value: String,
    pub field: usize,
    pub source: DefinitionSource,
}

/// The definition form open on the current screen.
#[derive(Debug, Clone)]
pub(super) enum DefinitionEditor {
    Device(DeviceEditor),
    App(AppEditor),
    Config(ConfigEditor),
}

impl DefinitionEditor {
    pub(super) fn kind(&self) -> FormKind {
        match self {
            Self::Device(editor) => editor.kind,
            Self::App(editor) => editor.kind,
            Self::Config(editor) => editor.kind,
        }
    }
}

/// One evidence source in a device's probe matrix.
#[derive(Debug, Clone)]
pub(super) struct ProbeSourceRow {
    pub source: String,
    pub class: String,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum TestKind {
    Scene,
    Piece,
    Theatre,
    Stage,
    #[default]
    Play,
    Act,
}

#[derive(Debug, Clone)]
pub(super) struct WorkspaceRow {
    pub path: PathBuf,
    pub name: String,
    pub current: bool,
    pub missing: bool,
}

#[derive(Debug, Clone)]
pub(super) struct DeviceRow {
    pub name: String,
    pub platform: String,
    pub connection: String,
    pub device_type: String,
    pub ip: String,
    pub port: Option<u16>,
    pub source: DefinitionSource,
}

#[derive(Debug, Clone)]
pub(super) struct AppRow {
    pub name: String,
    pub description: String,
    pub platforms: String,
    pub source: DefinitionSource,
}

#[derive(Debug, Clone)]
pub(super) struct ConfigRow {
    pub key: String,
    pub value: String,
    pub source: String,
}

#[derive(Debug)]
pub(super) struct History {
    entries: Vec<String>,
    index: Option<usize>,
    limit: usize,
}

impl History {
    fn load(path: &std::path::Path, limit: usize) -> Self {
        let mut entries = Vec::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let line = line.trim_end_matches('\r');
                if !line.is_empty() {
                    entries.push(line.to_string());
                }
            }
        }
        if entries.len() > limit {
            entries = entries.split_off(entries.len().saturating_sub(limit));
        }
        Self {
            entries,
            index: None,
            limit,
        }
    }

    fn add(&mut self, entry: &str) {
        if entry.trim().is_empty() {
            return;
        }
        if self.entries.last().is_some_and(|last| last == entry) {
            self.index = None;
            return;
        }
        self.entries.push(entry.to_string());
        if self.entries.len() > self.limit {
            let excess = self.entries.len().saturating_sub(self.limit);
            self.entries.drain(..excess);
        }
        self.index = None;
    }

    pub(super) fn previous(&mut self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let next = match self.index {
            Some(index) => index.saturating_sub(1),
            None => self.entries.len().saturating_sub(1),
        };
        self.index = Some(next);
        self.entries.get(next).cloned()
    }

    pub(super) fn next(&mut self) -> Option<String> {
        let index = self.index?;
        if index + 1 >= self.entries.len() {
            self.index = None;
            return None;
        }
        let next = index + 1;
        self.index = Some(next);
        self.entries.get(next).cloned()
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Prompt {
    pub(super) chars: Vec<char>,
    pub(super) cursor: usize,
    pub(super) completions: Vec<String>,
    pub(super) completion_index: usize,
    pub(super) completion_visible: bool,
    pub(super) completion_context: Vec<String>,
    accepted: bool,
}

impl Prompt {
    pub(super) fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub(super) fn set_text(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
        self.accepted = false;
        self.refresh_completion();
    }

    pub(super) fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.completions.clear();
        self.completion_index = 0;
        self.completion_visible = false;
        self.accepted = false;
    }

    pub(super) fn insert_char(&mut self, character: char) {
        let index = self.cursor.min(self.chars.len());
        self.chars.insert(index, character);
        self.cursor = index.saturating_add(1);
        self.accepted = false;
        self.refresh_completion();
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        let index = self.cursor.min(self.chars.len());
        let inserted: Vec<char> = text.chars().collect();
        let count = inserted.len();
        for (offset, character) in inserted.into_iter().enumerate() {
            self.chars.insert(index.saturating_add(offset), character);
        }
        self.cursor = index.saturating_add(count);
        self.accepted = false;
        self.refresh_completion();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let index = self.cursor.saturating_sub(1);
        if index < self.chars.len() {
            self.chars.remove(index);
            self.cursor = index;
        }
        self.accepted = false;
        self.refresh_completion();
    }

    pub(super) fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
        self.accepted = false;
        self.refresh_completion();
    }

    pub(super) fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn right(&mut self) {
        self.cursor = self.cursor.saturating_add(1).min(self.chars.len());
    }

    pub(super) fn home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    pub(super) fn kill_to_end(&mut self) {
        self.chars.truncate(self.cursor);
        self.refresh_completion();
    }

    pub(super) fn kill_to_start(&mut self) {
        if self.cursor <= self.chars.len() {
            self.chars.drain(..self.cursor);
            self.cursor = 0;
        }
        self.refresh_completion();
    }

    pub(super) fn kill_word(&mut self) {
        let mut index = self.cursor.min(self.chars.len());
        while index > 0
            && self
                .chars
                .get(index.saturating_sub(1))
                .is_some_and(|character| character.is_whitespace())
        {
            index = index.saturating_sub(1);
        }
        while index > 0
            && self
                .chars
                .get(index.saturating_sub(1))
                .is_some_and(|character| !character.is_whitespace())
        {
            index = index.saturating_sub(1);
        }
        if index < self.cursor && self.cursor <= self.chars.len() {
            self.chars.drain(index..self.cursor);
            self.cursor = index;
        }
        self.refresh_completion();
    }

    pub(super) fn move_line(&mut self, delta: i32) {
        let mut line = 0usize;
        let mut column = 0usize;
        for (index, character) in self.chars.iter().enumerate() {
            if index >= self.cursor {
                break;
            }
            if *character == '\n' {
                line = line.saturating_add(1);
                column = 0;
            } else {
                column = column.saturating_add(1);
            }
        }
        if delta < 0 {
            if line == 0 {
                self.home();
                return;
            }
            line = line.saturating_sub(1);
        } else {
            let lines = self
                .chars
                .iter()
                .filter(|character| **character == '\n')
                .count();
            if line >= lines {
                self.end();
                return;
            }
            line = line.saturating_add(1);
        }
        let mut target = 0usize;
        let mut current_line = 0usize;
        for (index, character) in self.chars.iter().enumerate() {
            if current_line == line && column == 0 {
                target = index;
                break;
            }
            if current_line == line && column > 0 {
                column = column.saturating_sub(1);
            }
            if *character == '\n' {
                current_line = current_line.saturating_add(1);
            }
            target = index.saturating_add(1);
        }
        self.cursor = target.min(self.chars.len());
    }

    pub(super) fn select_completion(&mut self) {
        let Some(candidate) = self.completions.get(self.completion_index).cloned() else {
            return;
        };
        self.set_text(&candidate);
        self.completion_visible = false;
        self.accepted = true;
        self.cursor = self.chars.len();
    }

    pub(super) fn refresh_completion(&mut self) {
        if self.accepted {
            return;
        }
        let text = self.text();
        self.completions = CompletionEngine::complete(&text, &self.completion_context);
        self.completion_index = self
            .completion_index
            .min(self.completions.len().saturating_sub(1));
        self.completion_visible =
            !self.completions.is_empty() && text.starts_with('/') && !text.contains('\n');
    }

    pub(super) fn next_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        self.completion_index = (self.completion_index + 1) % self.completions.len();
        self.completion_visible = true;
    }

    pub(super) fn previous_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        self.completion_index = self
            .completion_index
            .checked_sub(1)
            .unwrap_or_else(|| self.completions.len().saturating_sub(1));
        self.completion_visible = true;
    }
}

pub(super) struct ReverseSearch {
    pub(super) query: String,
    original: String,
    matches: Vec<String>,
    pub(super) index: usize,
}

impl ReverseSearch {
    fn new(history: &History) -> Self {
        let mut search = Self {
            query: String::new(),
            original: String::new(),
            matches: Vec::new(),
            index: 0,
        };
        search.update(history);
        search
    }

    pub(super) fn update(&mut self, history: &History) {
        let query = self.query.to_lowercase();
        self.matches = history
            .entries
            .iter()
            .rev()
            .filter(|entry| entry.to_lowercase().contains(&query))
            .cloned()
            .collect();
        self.index = self.index.min(self.matches.len().saturating_sub(1));
    }

    pub(super) fn current(&self) -> Option<&String> {
        self.matches.get(self.index)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct TuiState {
    schema_version: u32,
    screen: String,
    selected_job: Option<String>,
    selected_test: Option<String>,
    last_run: Option<Vec<String>>,
    log_follow: bool,
    filter: String,
    device_filter: Option<String>,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            screen: "home".to_string(),
            selected_job: None,
            selected_test: None,
            last_run: None,
            log_follow: false,
            filter: String::new(),
            device_filter: None,
        }
    }
}

pub(super) struct App {
    pub ctx: Context,
    pub screen: Screen,
    pub focus: Focus,
    pub prompt: Prompt,
    pub history: History,
    pub search: Option<ReverseSearch>,
    pub workspaces: Vec<WorkspaceRow>,
    pub landing_index: usize,
    pub menu_index: usize,
    pub jobs: Vec<JobRecord>,
    pub job_index: usize,
    pub follow: Option<String>,
    pub follow_notified: bool,
    pub events: Vec<ExecEvent>,
    pub tree: Vec<TreeLine>,
    pub logs: VecDeque<String>,
    pub current_command: String,
    pub follow_started: Option<Instant>,
    pub tests: Vec<TestRow>,
    pub test_index: usize,
    pub test_marks: BTreeSet<usize>,
    pub test_error: Option<String>,
    pub devices: Vec<DeviceRow>,
    pub device_index: usize,
    pub apps: Vec<AppRow>,
    pub app_index: usize,
    pub config_rows: Vec<ConfigRow>,
    pub config_index: usize,
    pub form: Option<DefinitionEditor>,
    pub resident: Option<ResidentStatus>,
    pub resident_dumps: Vec<DumpEntry>,
    pub resident_error: Option<String>,
    pub resident_scroll: u16,
    pub coverage: Vec<CoverageRow>,
    pub coverage_error: Option<String>,
    pub coverage_scroll: u16,
    pub probe: Vec<ProbeRow>,
    pub probe_scroll: u16,
    pub groups: Vec<GroupInfo>,
    pub group_index: usize,
    pub group_scroll: u16,
    pub group_error: Option<String>,
    pub group_editor: Option<GroupEditor>,
    pub activity: VecDeque<String>,
    pub overlay: Option<String>,
    pub should_quit: bool,
    pub ctrl_c_at: Option<Instant>,
    pub last_run_tokens: Option<Vec<String>>,
    pub filter: String,
    pub filter_input: Option<String>,
    pub device_filter: Option<String>,
    pub log_follow: bool,
    pub first_run: bool,
    pub first_run_prompt: bool,
    store: JobStore,
    last_refresh: Instant,
    resident_refreshed: Instant,
    history_path: PathBuf,
    history_limit: usize,
    state_path: PathBuf,
}

impl App {
    /// Build empty state without reading the workspace or spawning the CLI.
    ///
    /// `new` fills this in; tests use it to render deterministic frames.
    pub(super) fn empty(ctx: Context) -> Self {
        let store = JobStore::new(&ctx.home);
        let history_limit = history_limit(&ctx);
        let history_path = ctx.home.history_path();
        let state_path = ctx.home.state_path();
        let first_run = !ctx.config_path.exists();
        Self {
            screen: Screen::Landing,
            focus: Focus::Prompt,
            prompt: Prompt::default(),
            history: History::load(&history_path, history_limit),
            search: None,
            workspaces: Vec::new(),
            landing_index: 0,
            menu_index: 0,
            jobs: store.list(),
            job_index: 0,
            follow: None,
            follow_notified: false,
            events: Vec::new(),
            tree: Vec::new(),
            logs: VecDeque::new(),
            current_command: String::new(),
            follow_started: None,
            tests: Vec::new(),
            test_index: 0,
            test_marks: BTreeSet::new(),
            test_error: None,
            devices: Vec::new(),
            device_index: 0,
            apps: Vec::new(),
            app_index: 0,
            config_rows: Vec::new(),
            config_index: 0,
            form: None,
            resident: None,
            resident_dumps: Vec::new(),
            resident_error: None,
            resident_scroll: 0,
            coverage: Vec::new(),
            coverage_error: None,
            coverage_scroll: 0,
            probe: Vec::new(),
            probe_scroll: 0,
            groups: Vec::new(),
            group_index: 0,
            group_scroll: 0,
            group_error: None,
            group_editor: None,
            activity: VecDeque::new(),
            overlay: None,
            should_quit: false,
            ctrl_c_at: None,
            last_run_tokens: None,
            filter: String::new(),
            filter_input: None,
            device_filter: None,
            log_follow: false,
            first_run,
            first_run_prompt: false,
            store,
            last_refresh: Instant::now(),
            resident_refreshed: Instant::now(),
            history_path,
            history_limit,
            state_path,
            ctx,
        }
    }

    pub(super) fn new(ctx: Context) -> Self {
        let mut app = Self::empty(ctx);
        app.load_devices();
        app.load_groups();
        app.load_apps();
        app.load_config();
        app.load_resident();
        app.load_tests();
        app.rebuild_coverage();
        app.refresh_completion_context();
        app.load_state();
        app.load_workspaces();
        app.screen = Screen::Landing;
        app.focus = Focus::Content;
        app.overlay = None;
        app.push_activity("Welcome. Choose a workspace to begin.".to_string());
        app
    }

    /// Read the remembered workspace list and put the active workspace first.
    pub(super) fn load_workspaces(&mut self) {
        self.workspaces.clear();
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut paths = vec![self.ctx.base.clone()];
        let global = settings::SettingsFile::load(&self.ctx.home.config_path());
        if let Some(SettingValue::List(values)) = global.values.get("workspaces") {
            for value in values {
                let path = PathBuf::from(value);
                if !seen.iter().any(|seen| same_path(seen, &path))
                    && !same_path(&self.ctx.base, &path)
                {
                    paths.push(path.clone());
                    seen.push(path);
                }
            }
        }
        for path in paths {
            let missing = !path.is_dir();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .map_or_else(|| path.display().to_string(), str::to_string);
            let current = same_path(&path, &self.ctx.base);
            self.workspaces.push(WorkspaceRow {
                path,
                name,
                current,
                missing,
            });
        }
        self.landing_index = self.landing_index.min(self.landing_len().saturating_sub(1));
    }

    pub(super) fn landing_len(&self) -> usize {
        self.workspaces.len() + 3
    }

    pub(super) fn move_landing(&mut self, delta: i32) {
        let len = self.landing_len();
        if len == 0 {
            return;
        }
        if delta < 0 {
            self.landing_index = self.landing_index.saturating_sub(1);
        } else {
            self.landing_index = (self.landing_index + 1).min(len - 1);
        }
    }

    pub(super) fn move_menu(&mut self, delta: i32) {
        const MENU_LEN: usize = 13;
        if delta < 0 {
            self.menu_index = self.menu_index.saturating_sub(1);
        } else {
            self.menu_index = (self.menu_index + 1).min(MENU_LEN - 1);
        }
    }

    pub(super) fn activate_landing(&mut self) -> Action {
        let index = self.landing_index;
        if let Some(workspace) = self.workspaces.get(index) {
            let path = workspace.path.clone();
            if !path.is_dir() {
                self.push_activity(format!("workspace is missing: {}", path.display()));
                return Action::None;
            }
            self.enter_workspace(path);
            return Action::None;
        }
        match index.saturating_sub(self.workspaces.len()) {
            0 => {
                self.add_current_workspace();
                Action::None
            }
            1 => {
                self.run_doctor();
                Action::None
            }
            _ => Action::Quit,
        }
    }

    pub(super) fn activate_landing_action(&mut self, action: usize) {
        let index = self.workspaces.len().saturating_add(action);
        if index < self.landing_len() {
            self.landing_index = index;
            let _ = self.activate_landing();
        }
    }

    pub(super) fn activate_menu(&mut self) -> Action {
        match self.menu_index {
            0 => self.set_screen(Screen::Home),
            1 => self.set_screen(Screen::Runs),
            2 => self.set_screen(Screen::Resident),
            3 => self.set_screen(Screen::Tests),
            4 => self.set_screen(Screen::Coverage),
            5 => self.set_screen(Screen::Probe),
            6 => self.set_screen(Screen::Groups),
            7 => self.set_screen(Screen::Devices),
            8 => self.set_screen(Screen::Apps),
            9 => self.set_screen(Screen::Config),
            10 => self.run_doctor(),
            11 => {
                self.screen = Screen::Landing;
                self.focus = Focus::Content;
                self.overlay = None;
            }
            _ => return Action::Quit,
        }
        Action::None
    }

    fn enter_workspace(&mut self, path: PathBuf) {
        if !same_path(&path, &self.ctx.base) {
            self.ctx.base = path.clone();
            self.ctx.config_path = path.join("impresario.config");
            self.ctx.devices_path = path.join("impresario.devices.config");
            self.store = JobStore::new(&self.ctx.home);
            self.jobs = self.store.list();
            self.job_index = 0;
            self.test_index = 0;
            self.tests.clear();
            self.test_error = None;
            self.devices.clear();
            self.apps.clear();
            self.config_rows.clear();
            self.probe.clear();
            self.probe_scroll = 0;
            self.groups.clear();
            self.group_index = 0;
            self.group_scroll = 0;
            self.group_error = None;
            self.group_editor = None;
            self.follow = None;
            self.tree.clear();
            self.logs.clear();
            self.load_devices();
            self.load_groups();
            self.load_apps();
            self.load_config();
            self.load_resident();
            self.load_tests();
            self.rebuild_coverage();
            self.refresh_completion_context();
            let path_text = path.display().to_string();
            let _ = self.run_child(&[
                "workspace".to_string(),
                "use".to_string(),
                path_text.clone(),
            ]);
            let _ = self.run_child(&["workspace".to_string(), "add".to_string(), path_text]);
        }
        self.screen = Screen::Menu;
        self.focus = Focus::Content;
        self.overlay = None;
        self.first_run = !self.ctx.config_path.exists();
        self.first_run_prompt = self.first_run;
        if self.first_run {
            self.overlay = Some(first_run_text(&self.ctx));
        }
        self.push_activity(format!("Opened workspace {}", self.ctx.base.display()));
        self.load_workspaces();
    }

    fn add_current_workspace(&mut self) {
        let path = self.ctx.base.display().to_string();
        match self.run_child(&["workspace".to_string(), "add".to_string(), path.clone()]) {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("already listed") {
                    self.push_activity(format!("Workspace already remembered: {path}"));
                } else {
                    self.push_activity(format!("Added workspace {path}"));
                }
                self.load_workspaces();
            }
            Ok(output) => {
                let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
                self.push_activity(if message.is_empty() {
                    "workspace add failed".to_string()
                } else {
                    message
                });
            }
            Err(message) => self.push_activity(message),
        }
    }

    fn run_doctor(&mut self) {
        match self.run_child(&["doctor".to_string()]) {
            Ok(output) => {
                let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if !stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                if text.is_empty() {
                    text = "doctor produced no output".to_string();
                }
                self.overlay = Some(format!("Doctor\n{text}"));
            }
            Err(message) => self.push_activity(message),
        }
    }

    fn refresh_completion_context(&mut self) {
        let mut values = BTreeSet::new();
        for device in &self.devices {
            values.insert(format!("/run --device {}", device.name));
        }
        for group in &self.groups {
            values.insert(format!("/run --device @{}", group.name));
        }
        for app in &self.apps {
            values.insert(format!("/run --app {}", app.name));
        }
        for row in &self.tests {
            match row.kind {
                TestKind::Play => {
                    values.insert(format!("/run --play {}", row.name));
                }
                TestKind::Act => {
                    values.insert(format!("/run --act {}", row.name));
                }
                TestKind::Stage => {
                    values.insert(format!("/tests where {}", row.name));
                }
                TestKind::Scene | TestKind::Piece | TestKind::Theatre => {}
            }
        }
        for tag in self
            .tests
            .iter()
            .filter(|row| row.kind == TestKind::Act)
            .flat_map(|row| {
                row.label
                    .split_once('[')
                    .and_then(|(_, rest)| rest.split_once(']'))
                    .map(|(tags, _)| tags.split(',').map(str::trim).map(str::to_string))
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
            })
        {
            if !tag.is_empty() {
                values.insert(format!("/run --tag {tag}"));
            }
        }
        for job in &self.jobs {
            values.insert(format!("/attach {}", job.id));
            values.insert(format!("/logs {}", job.id));
            values.insert(format!("/report {}", job.id));
        }
        self.prompt.completion_context = values.into_iter().collect();
        self.prompt.refresh_completion();
    }

    fn load_state(&mut self) {
        let text = match std::fs::read_to_string(&self.state_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.push_activity(format!(
                    "state: cannot read {}: {error}",
                    self.state_path.display()
                ));
                return;
            }
        };
        let state: TuiState = match serde_json::from_str(&text) {
            Ok(state) => state,
            Err(error) => {
                self.push_activity(format!(
                    "state: cannot parse {}: {error}; using defaults",
                    self.state_path.display()
                ));
                return;
            }
        };
        self.screen = match state.screen.as_str() {
            "landing" => Screen::Landing,
            "menu" => Screen::Menu,
            "runs" => Screen::Runs,
            "tests" => Screen::Tests,
            "coverage" => Screen::Coverage,
            "probe" => Screen::Probe,
            "groups" => Screen::Groups,
            "devices" => Screen::Devices,
            "apps" => Screen::Apps,
            "config" => Screen::Config,
            "resident" => Screen::Resident,
            _ => Screen::Home,
        };
        if let Some(id) = state.selected_job {
            if let Some(index) = self.jobs.iter().position(|record| record.id == id) {
                self.job_index = index;
            }
        }
        if let Some(name) = state.selected_test {
            if let Some(index) = self.tests.iter().position(|row| {
                row.name == name && matches!(row.kind, TestKind::Play | TestKind::Act)
            }) {
                self.test_index = index;
            }
        }
        self.last_run_tokens = state.last_run.filter(|tokens| {
            tokens
                .first()
                .is_some_and(|token| token == "run" || token == "impresario")
        });
        self.log_follow = state.log_follow;
        self.filter = state.filter;
        self.device_filter = state.device_filter;
        self.clamp_selection();
    }

    pub(super) fn save_state(&self) -> Result<(), String> {
        let state = TuiState {
            schema_version: 1,
            screen: self.screen.label().to_string(),
            selected_job: self.selected_job().map(|record| record.id.clone()),
            selected_test: self
                .selected_test()
                .filter(|row| matches!(row.kind, TestKind::Play | TestKind::Act))
                .map(|row| row.name.clone()),
            last_run: self.last_run_tokens.clone(),
            log_follow: self.log_follow,
            filter: self.filter.clone(),
            device_filter: self.device_filter.clone(),
        };
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&state)
            .map_err(|error| format!("cannot serialize TUI state: {error}"))?;
        std::fs::write(&self.state_path, format!("{text}\n"))
            .map_err(|error| format!("cannot write {}: {error}", self.state_path.display()))
    }

    fn selected_job_id(&self) -> Option<&str> {
        self.jobs
            .get(self.job_index)
            .map(|record| record.id.as_str())
    }

    pub(super) fn tick(&mut self) {
        if self.last_refresh.elapsed() < REFRESH {
            return;
        }
        self.last_refresh = Instant::now();
        let selected = self.selected_job_id().map(str::to_string);
        self.jobs = self.store.list();
        if let Some(id) = selected {
            if let Some(index) = self.jobs.iter().position(|record| record.id == id) {
                self.job_index = index;
            } else {
                self.job_index = self.job_index.min(self.jobs.len().saturating_sub(1));
            }
        } else {
            self.job_index = self.job_index.min(self.jobs.len().saturating_sub(1));
        }
        self.clamp_selection();
        if let Some(id) = self.follow.clone() {
            self.refresh_follow(&id);
        }
        if self.screen == Screen::Resident && self.resident_refreshed.elapsed() >= RESIDENT_REFRESH
        {
            self.load_resident();
        }
    }

    fn refresh_follow(&mut self, id: &str) {
        let Some(record) = self.store.read(id).ok().flatten() else {
            self.follow = None;
            return;
        };
        if self.follow_started.is_none() && !record.status.terminal() {
            self.follow_started = Some(Instant::now());
        }
        if record.status.terminal() && !self.follow_notified {
            self.follow_notified = true;
            self.push_activity(format!(
                "Job {id} finished: {} (exit code {}).",
                record.status.label(),
                record.exit_code.unwrap_or(0)
            ));
        }
        self.events = jobs::read_events(&self.store.events_path(id));
        self.rebuild_tree(&record);
    }

    fn active_filter(&self) -> Option<&str> {
        self.filter_input
            .as_deref()
            .or_else(|| (!self.filter.is_empty()).then_some(self.filter.as_str()))
    }

    fn job_matches_filter(&self, record: &JobRecord) -> bool {
        let Some(filter) = self.active_filter() else {
            return true;
        };
        let filter = filter.to_lowercase();
        let mut haystack = format!(
            "{} {} {} {}",
            record.id,
            record.status.label(),
            record.plays.join(" "),
            record.devices.join(" ")
        );
        if let Some(message) = &record.message {
            haystack.push(' ');
            haystack.push_str(message);
        }
        haystack.to_lowercase().contains(&filter)
    }

    pub(super) fn visible_job_indices(&self) -> Vec<usize> {
        self.jobs
            .iter()
            .enumerate()
            .filter(|(_, record)| self.job_matches_filter(record))
            .map(|(index, _)| index)
            .collect()
    }

    pub(super) fn line_matches_filter(&self, line: &str) -> bool {
        self.active_filter()
            .is_none_or(|filter| line.to_lowercase().contains(&filter.to_lowercase()))
    }

    fn clamp_selection(&mut self) {
        let visible = self.visible_job_indices();
        if visible.is_empty() {
            self.job_index = 0;
        } else if !visible.contains(&self.job_index) {
            if let Some(first) = visible.first() {
                self.job_index = *first;
            }
        }
    }

    fn rebuild_tree(&mut self, record: &JobRecord) {
        self.tree.clear();
        self.logs.clear();
        self.current_command.clear();
        if self.events.is_empty() {
            self.tree.push(TreeLine {
                indent: 0,
                text: format!(
                    "{}  {}  [{}]",
                    record.id,
                    record.status.label(),
                    record.plays.join(", ")
                ),
                status: status_of_job(record.status),
            });
        }
        for event in &self.events {
            if let Some(filter) = &self.device_filter {
                if event_device(event).is_some_and(|device| device != filter) {
                    continue;
                }
            }
            match event {
                ExecEvent::RunStarted {
                    run_id,
                    devices,
                    acts,
                } => {
                    self.tree.push(TreeLine {
                        indent: 0,
                        text: format!("run {run_id}  {} device(s)  {acts} act(s)", devices.len()),
                        status: LineStatus::Running,
                    });
                    self.logs.push_back(format!(
                        "run {run_id}: {} device(s), {acts} act(s)",
                        devices.len()
                    ));
                }
                ExecEvent::DeviceStarted { device, platform } => {
                    self.tree.push(TreeLine {
                        indent: 1,
                        text: format!("{device} ({platform})"),
                        status: LineStatus::Running,
                    });
                }
                ExecEvent::ActStarted {
                    device,
                    play,
                    act,
                    attempt,
                    ..
                } => {
                    let suffix = if *attempt > 1 {
                        format!("  attempt {attempt}")
                    } else {
                        String::new()
                    };
                    self.tree.push(TreeLine {
                        indent: 2,
                        text: format!("{device} :: {play} :: {act}{suffix}"),
                        status: LineStatus::Running,
                    });
                }
                ExecEvent::ActFinished {
                    device,
                    play,
                    act,
                    status,
                    duration_ms,
                } => {
                    self.tree.push(TreeLine {
                        indent: 2,
                        text: format!("{device} :: {play} :: {act}  {status}  {}ms", duration_ms),
                        status: status_of_str(status),
                    });
                    if !status.eq_ignore_ascii_case("passed") {
                        self.logs.push_back(format!(
                            "{device} {play} {act}: {status} ({duration_ms} ms)"
                        ));
                    }
                }
                ExecEvent::StepStarted {
                    device,
                    act,
                    stage,
                    index,
                    label,
                    source,
                    ..
                } => {
                    self.current_command = format!("{source}  ({label})");
                    self.tree.push(TreeLine {
                        indent: 3,
                        text: format!("{stage} [{index}] {label}: {source}"),
                        status: LineStatus::Running,
                    });
                    self.logs.push_back(format!("{device} {act}: {source}"));
                }
                ExecEvent::StepFinished {
                    act,
                    stage,
                    index,
                    status,
                    duration_ms,
                    error,
                    details,
                    ..
                } => {
                    self.tree.push(TreeLine {
                        indent: 3,
                        text: format!("{stage} [{index}]  {status}  {duration_ms}ms"),
                        status: status_of_str(status),
                    });
                    for (key, value) in details {
                        self.tree.push(TreeLine {
                            indent: 4,
                            text: format!("{key}: {value}"),
                            status: LineStatus::Detail,
                        });
                    }
                    if let Some(error) = error {
                        self.logs
                            .push_back(format!("{act} {stage} [{index}]: {error}"));
                    }
                    if !status.eq_ignore_ascii_case("running") {
                        self.current_command.clear();
                    }
                }
                ExecEvent::LogLine {
                    device,
                    level,
                    source,
                    message,
                    ..
                } => {
                    self.logs
                        .push_back(format!("[{level}] {device} {source}: {message}"));
                }
                ExecEvent::DeviceFinished {
                    device,
                    status,
                    error,
                } => {
                    self.tree.push(TreeLine {
                        indent: 1,
                        text: format!("{device}  {status}"),
                        status: status_of_str(status),
                    });
                    if let Some(error) = error {
                        self.logs.push_back(format!("{device}: {error}"));
                    }
                }
                ExecEvent::RunFinished {
                    status,
                    exit_code,
                    duration_ms,
                } => {
                    self.tree.push(TreeLine {
                        indent: 0,
                        text: format!(
                            "run finished: {status} (exit {exit_code}, {duration_ms} ms)"
                        ),
                        status: status_of_str(status),
                    });
                }
            }
        }
        if let Some(filter) = self.active_filter().map(str::to_lowercase) {
            self.tree
                .retain(|line| line.text.to_lowercase().contains(&filter));
            self.logs
                .retain(|line| line.to_lowercase().contains(&filter));
        }
        while self.tree.len() > 800 {
            self.tree.remove(0);
        }
        while self.logs.len() > 300 {
            self.logs.pop_front();
        }
    }

    pub(super) fn elapsed_label(&self) -> String {
        let record = self.jobs.get(self.job_index);
        match record {
            Some(record) => match record.duration_ms {
                Some(millis) => super::format_duration(millis),
                None if record.status == JobStatus::Running => self
                    .follow_started
                    .map(|started| super::format_duration(started.elapsed().as_millis() as u64))
                    .unwrap_or_else(|| "running".to_string()),
                None => "-".to_string(),
            },
            None => "-".to_string(),
        }
    }

    pub(super) fn selected_job(&self) -> Option<&JobRecord> {
        self.jobs.get(self.job_index)
    }

    pub(super) fn selected_test(&self) -> Option<&TestRow> {
        self.tests.get(self.test_index)
    }

    pub(super) fn load_devices(&mut self) {
        self.devices.clear();
        match crate::definitions::device_entries(&self.ctx.home, &self.ctx.devices_path) {
            Ok(entries) => {
                for entry in entries {
                    let connection = entry.device.effective_transport().as_str().to_string();
                    let platform = entry.device.platform.as_str().to_string();
                    self.devices.push(DeviceRow {
                        name: entry.device.name,
                        platform,
                        connection,
                        device_type: entry.device.device_type.as_str().to_string(),
                        ip: entry.device.ip,
                        port: entry.device.connection.port,
                        source: entry.source,
                    });
                }
                self.device_index = self.device_index.min(self.devices.len().saturating_sub(1));
            }
            Err(message) => {
                self.push_activity(format!("devices: {message}"));
                self.device_index = 0;
            }
        }
    }

    pub(super) fn load_groups(&mut self) {
        self.groups.clear();
        self.group_error = None;
        match crate::definitions::group_rows(&self.ctx.home, &self.ctx.devices_path) {
            Ok(groups) => {
                self.groups = groups;
                self.group_index = self.group_index.min(self.groups.len().saturating_sub(1));
                self.group_scroll = self.group_scroll_for_index();
            }
            Err(message) => {
                self.group_error = Some(message);
                self.group_index = 0;
                self.group_scroll = 0;
            }
        }
        self.refresh_completion_context();
    }

    pub(super) fn load_apps(&mut self) {
        self.apps.clear();
        let apps_path = self.ctx.base.join("impresario.apps.config");
        match crate::definitions::app_entries(&self.ctx.home, &apps_path) {
            Ok(entries) => {
                for entry in entries {
                    self.apps.push(AppRow {
                        name: entry.app.name,
                        description: entry.app.description.unwrap_or_default(),
                        platforms: entry
                            .app
                            .platforms
                            .keys()
                            .map(|platform| platform.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                        source: entry.source,
                    });
                }
                self.app_index = self.app_index.min(self.apps.len().saturating_sub(1));
            }
            Err(message) => {
                self.push_activity(format!("apps: {message}"));
                self.app_index = 0;
            }
        }
    }

    pub(super) fn load_config(&mut self) {
        self.config_rows.clear();
        let outcome = cli::load_effective_config(&self.ctx.base, &self.ctx.config_path);
        let Some(config) = outcome.config else {
            self.push_activity(format!(
                "config: {} error(s)",
                outcome.diagnostics.errors().count()
            ));
            return;
        };
        for (key, value, source) in effective_settings(&self.ctx) {
            self.config_rows.push(ConfigRow {
                key: key.to_string(),
                value,
                source: source.to_string(),
            });
        }
        self.config_rows.push(ConfigRow {
            key: "config.path".to_string(),
            value: self.ctx.config_path.display().to_string(),
            source: "workspace".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "project".to_string(),
            value: config.project,
            source: "typed".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "plays_dir".to_string(),
            value: config.plays_dir.display().to_string(),
            source: "typed".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "scenes_dir".to_string(),
            value: config.scenes_dir.display().to_string(),
            source: "typed".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "theatres_dir".to_string(),
            value: config.theatres_dir.display().to_string(),
            source: "typed".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "report_output".to_string(),
            value: config.report_output.display().to_string(),
            source: "typed".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "parallel_devices".to_string(),
            value: config.parallel_devices.to_string(),
            source: "typed".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "retries".to_string(),
            value: config.retries.to_string(),
            source: "typed".to_string(),
        });
        self.config_rows.push(ConfigRow {
            key: "reporters".to_string(),
            value: config
                .reporters
                .iter()
                .map(|reporter| reporter.as_str())
                .collect::<Vec<_>>()
                .join(","),
            source: "typed".to_string(),
        });
        self.config_index = self
            .config_index
            .min(self.config_rows.len().saturating_sub(1));
    }

    pub(super) fn load_resident(&mut self) {
        match resident_status::collect(&self.ctx.home, &self.ctx.config_path) {
            Ok(status) => {
                self.resident = Some(status);
                self.resident_error = None;
            }
            Err(message) => {
                self.resident = None;
                self.resident_error = Some(message);
            }
        }
        match resident_status::dumps(&self.ctx.base, None) {
            Ok(dumps) => self.resident_dumps = dumps,
            Err(message) => {
                self.resident_dumps.clear();
                if self.resident_error.is_none() {
                    self.resident_error = Some(message);
                }
            }
        }
        self.resident_refreshed = Instant::now();
    }

    /// Refresh the theatre coverage matrix from the same `tests --json`
    /// explorer data the Tests screen uses.
    pub(super) fn load_coverage(&mut self) {
        self.load_tests();
        self.rebuild_coverage();
    }

    /// Rebuild the plan-time probe matrix for the configured devices.
    pub(super) fn load_probe(&mut self) {
        self.load_devices();
        self.probe = self.devices.iter().map(probe_row).collect();
    }

    pub(super) fn select_first_group(&mut self) {
        self.group_index = 0;
        self.group_scroll = 0;
    }

    pub(super) fn select_last_group(&mut self) {
        self.group_index = self.groups.len().saturating_sub(1);
        self.group_scroll = self.group_scroll_for_index();
    }

    fn group_scroll_for_index(&self) -> u16 {
        if self.group_index == 0 {
            return 0;
        }
        let lines = 3usize.saturating_add(self.group_index.saturating_mul(2));
        u16::try_from(lines).unwrap_or(u16::MAX)
    }

    pub(super) fn open_group_editor(&mut self, add: bool) {
        if add {
            self.group_scroll = 0;
            self.group_editor = Some(GroupEditor {
                kind: GroupEditorKind::Add,
                name: String::new(),
                members: String::new(),
                members_focus: false,
                source: DefinitionSource::Workspace,
            });
            return;
        }
        let Some(group) = self.groups.get(self.group_index) else {
            self.push_activity("No group selected; press `a` to add one.".to_string());
            return;
        };
        self.group_scroll = 0;
        self.group_editor = Some(GroupEditor {
            kind: GroupEditorKind::Edit,
            name: group.name.clone(),
            members: group.members.join(", "),
            members_focus: true,
            source: group.source,
        });
    }

    pub(super) fn confirm_remove_group(&mut self) {
        let Some(group) = self.groups.get(self.group_index) else {
            self.push_activity("No group selected.".to_string());
            return;
        };
        self.group_scroll = 0;
        self.group_editor = Some(GroupEditor {
            kind: GroupEditorKind::Remove,
            name: group.name.clone(),
            members: String::new(),
            members_focus: false,
            source: group.source,
        });
    }

    pub(super) fn handle_group_editor_key(&mut self, key: KeyEvent) -> Action {
        let Some(editor) = self.group_editor.as_ref() else {
            return Action::None;
        };
        let kind = editor.kind;
        if kind == GroupEditorKind::Remove {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                    if let Some(editor) = self.group_editor.take() {
                        self.run_group_editor(editor);
                    }
                }
                KeyCode::Esc | KeyCode::Char('n' | 'N') => self.group_editor = None,
                _ => {}
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Esc => self.group_editor = None,
            KeyCode::Enter => {
                if let Some(editor) = self.group_editor.take() {
                    self.run_group_editor(editor);
                }
            }
            KeyCode::Tab | KeyCode::BackTab if kind == GroupEditorKind::Add => {
                if let Some(editor) = self.group_editor.as_mut() {
                    editor.members_focus = !editor.members_focus;
                }
            }
            KeyCode::Backspace => {
                if let Some(editor) = self.group_editor.as_mut() {
                    let field = if editor.members_focus {
                        &mut editor.members
                    } else {
                        &mut editor.name
                    };
                    field.pop();
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(editor) = self.group_editor.as_mut() {
                    let field = if editor.members_focus {
                        &mut editor.members
                    } else {
                        &mut editor.name
                    };
                    field.push(character);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn run_group_editor(&mut self, editor: GroupEditor) {
        let name = editor.name.trim().to_string();
        if name.is_empty() {
            self.push_activity("A group name is required.".to_string());
            self.group_editor = Some(editor);
            return;
        }
        let action = match editor.kind {
            GroupEditorKind::Add => "add",
            GroupEditorKind::Edit => "set",
            GroupEditorKind::Remove => "remove",
        };
        let members = if editor.kind == GroupEditorKind::Remove {
            Vec::new()
        } else {
            split_group_members(&editor.members)
        };
        let args = group_command(action, &name, &members, editor.source);
        self.run_and_report(&args);
        self.load_groups();
        if let Some(index) = self.groups.iter().position(|group| group.name == name) {
            self.group_index = index;
            self.group_scroll = self.group_scroll_for_index();
        }
    }

    pub(super) fn open_device_form(&mut self, add: bool) {
        if add {
            self.form = Some(DefinitionEditor::Device(DeviceEditor {
                kind: FormKind::Add,
                name: String::new(),
                device_type: String::new(),
                ip: String::new(),
                port: String::new(),
                field: 0,
                source: DefinitionSource::Workspace,
            }));
            return;
        }
        let Some(row) = self.devices.get(self.device_index) else {
            self.push_activity("No device selected; press `a` to add one.".to_string());
            return;
        };
        self.form = Some(DefinitionEditor::Device(DeviceEditor {
            kind: FormKind::Edit,
            name: row.name.clone(),
            device_type: row.device_type.clone(),
            ip: row.ip.clone(),
            port: device_display_port(row),
            field: 1,
            source: row.source,
        }));
    }

    pub(super) fn confirm_remove_device(&mut self) {
        let Some(row) = self.devices.get(self.device_index) else {
            self.push_activity("No device selected.".to_string());
            return;
        };
        self.form = Some(DefinitionEditor::Device(DeviceEditor {
            kind: FormKind::Remove,
            name: row.name.clone(),
            device_type: row.device_type.clone(),
            ip: row.ip.clone(),
            port: device_display_port(row),
            field: 0,
            source: row.source,
        }));
    }

    pub(super) fn open_app_form(&mut self, add: bool) {
        if add {
            self.form = Some(DefinitionEditor::App(AppEditor {
                kind: FormKind::Add,
                name: String::new(),
                description: String::new(),
                platforms: String::new(),
                packages: String::new(),
                launch_activities: String::new(),
                launch_params: String::new(),
                clear_data: false,
                field: 0,
                source: DefinitionSource::Workspace,
            }));
            return;
        }
        let Some(row) = self.apps.get(self.app_index) else {
            self.push_activity("No app selected; press `a` to add one.".to_string());
            return;
        };
        let apps_path = self.ctx.base.join("impresario.apps.config");
        let entry = crate::definitions::app_entries(&self.ctx.home, &apps_path)
            .ok()
            .and_then(|entries| entries.into_iter().find(|entry| entry.app.name == row.name));
        let Some(entry) = entry else {
            self.push_activity(format!(
                "app '{}' is no longer in the definitions.",
                row.name
            ));
            return;
        };
        self.form = Some(DefinitionEditor::App(app_editor_from(
            FormKind::Edit,
            &entry,
        )));
    }

    pub(super) fn confirm_remove_app(&mut self) {
        let Some(row) = self.apps.get(self.app_index) else {
            self.push_activity("No app selected.".to_string());
            return;
        };
        self.form = Some(DefinitionEditor::App(AppEditor {
            kind: FormKind::Remove,
            name: row.name.clone(),
            description: String::new(),
            platforms: String::new(),
            packages: String::new(),
            launch_activities: String::new(),
            launch_params: String::new(),
            clear_data: false,
            field: 0,
            source: row.source,
        }));
    }

    /// Enter on a Config row: open a value form for a schema setting, or the
    /// file for derived (`typed`) rows such as relative directories.
    pub(super) fn edit_config_setting(&mut self) -> Action {
        let Some(row) = self.config_rows.get(self.config_index).cloned() else {
            self.push_activity("No setting selected; press `a` to add one.".to_string());
            return Action::None;
        };
        if row.source == "typed" {
            return Action::OpenEditor(self.ctx.config_path.clone());
        }
        if settings::definition(&row.key).is_none() {
            return Action::OpenEditor(self.ctx.config_path.clone());
        }
        let source = config_target_source(&row.key, config_row_source(&row));
        self.form = Some(DefinitionEditor::Config(ConfigEditor {
            kind: FormKind::Edit,
            key: row.key,
            value: row.value,
            field: 0,
            source,
        }));
        Action::None
    }

    pub(super) fn open_config_add(&mut self) {
        self.form = Some(DefinitionEditor::Config(ConfigEditor {
            kind: FormKind::Add,
            key: String::new(),
            value: String::new(),
            field: 0,
            source: DefinitionSource::Workspace,
        }));
    }

    pub(super) fn confirm_unset_config(&mut self) {
        let Some(row) = self.config_rows.get(self.config_index) else {
            self.push_activity("No setting selected.".to_string());
            return;
        };
        if row.source == "typed" || settings::definition(&row.key).is_none() {
            let key = row.key.clone();
            self.push_activity(format!(
                "`{key}` is derived; edit the file with `o` instead."
            ));
            return;
        }
        self.form = Some(DefinitionEditor::Config(ConfigEditor {
            kind: FormKind::Remove,
            key: row.key.clone(),
            value: String::new(),
            field: 0,
            source: config_target_source(&row.key, config_row_source(row)),
        }));
    }

    pub(super) fn handle_form_key(&mut self, key: KeyEvent) -> Action {
        let kind = match self.form.as_ref() {
            Some(form) => form.kind(),
            None => return Action::None,
        };
        match key.code {
            KeyCode::Esc => {
                self.form = None;
                return Action::None;
            }
            KeyCode::Enter => {
                self.submit_form();
                return Action::None;
            }
            KeyCode::Char('y' | 'Y') if kind == FormKind::Remove => {
                self.submit_form();
                return Action::None;
            }
            KeyCode::Char('n' | 'N') if kind == FormKind::Remove => {
                self.form = None;
                return Action::None;
            }
            KeyCode::Tab => {
                self.form_cycle_field(false);
                return Action::None;
            }
            KeyCode::BackTab => {
                self.form_cycle_field(true);
                return Action::None;
            }
            KeyCode::Backspace => {
                if let Some(field) = self.form_field_mut() {
                    field.pop();
                }
                return Action::None;
            }
            _ => {}
        }
        if key.code == KeyCode::Char(' ') && self.form_clear_data_focused() {
            if let Some(DefinitionEditor::App(editor)) = self.form.as_mut() {
                editor.clear_data = !editor.clear_data;
            }
            return Action::None;
        }
        if let KeyCode::Char(character) = key.code {
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                if let Some(field) = self.form_field_mut() {
                    field.push(character);
                }
            }
        }
        Action::None
    }

    fn form_field_mut(&mut self) -> Option<&mut String> {
        match self.form.as_mut()? {
            DefinitionEditor::Device(editor) => match editor.field {
                0 if editor.kind == FormKind::Add => Some(&mut editor.name),
                1 => Some(&mut editor.device_type),
                2 => Some(&mut editor.ip),
                3 => Some(&mut editor.port),
                _ => None,
            },
            DefinitionEditor::App(editor) => match editor.field {
                0 if editor.kind == FormKind::Add => Some(&mut editor.name),
                1 => Some(&mut editor.description),
                2 => Some(&mut editor.platforms),
                3 => Some(&mut editor.packages),
                4 => Some(&mut editor.launch_activities),
                5 => Some(&mut editor.launch_params),
                _ => None,
            },
            DefinitionEditor::Config(editor) => match (editor.kind, editor.field) {
                (FormKind::Add, 0) => Some(&mut editor.key),
                (FormKind::Add | FormKind::Edit, 1) | (FormKind::Edit, 0) => {
                    Some(&mut editor.value)
                }
                _ => None,
            },
        }
    }

    fn form_clear_data_focused(&self) -> bool {
        matches!(
            &self.form,
            Some(DefinitionEditor::App(editor))
                if editor.kind != FormKind::Remove && editor.field == 6
        )
    }

    fn form_cycle_field(&mut self, backwards: bool) {
        if let Some(form) = self.form.as_mut() {
            match form {
                DefinitionEditor::Device(editor) => {
                    let fields: &[usize] = if editor.kind == FormKind::Add {
                        &[0, 1, 2, 3]
                    } else {
                        &[1, 2, 3]
                    };
                    cycle_field(&mut editor.field, fields, backwards);
                }
                DefinitionEditor::App(editor) => {
                    let fields: &[usize] = if editor.kind == FormKind::Add {
                        &[0, 1, 2, 3, 4, 5, 6]
                    } else {
                        &[1, 2, 3, 4, 5, 6]
                    };
                    cycle_field(&mut editor.field, fields, backwards);
                }
                DefinitionEditor::Config(editor) => {
                    let fields: &[usize] = match editor.kind {
                        FormKind::Add => &[0, 1],
                        FormKind::Edit => &[0],
                        FormKind::Remove => &[],
                    };
                    cycle_field(&mut editor.field, fields, backwards);
                }
            }
        }
    }

    fn submit_form(&mut self) {
        let Some(form) = self.form.take() else {
            return;
        };
        match form {
            DefinitionEditor::Device(editor) => {
                if let Err(message) = validate_device_form(&editor) {
                    self.push_activity(message);
                    self.form = Some(DefinitionEditor::Device(editor));
                    return;
                }
                let args = device_form_command(&editor);
                if self.run_and_report_status(&args) {
                    let name = editor.name.trim().to_string();
                    let added = editor.kind == FormKind::Add;
                    self.load_devices();
                    self.load_groups();
                    if let Some(index) = self.devices.iter().position(|row| row.name == name) {
                        self.device_index = index;
                    } else if added && name.is_empty() {
                        self.device_index = self.devices.len().saturating_sub(1);
                    }
                } else {
                    self.form = Some(DefinitionEditor::Device(editor));
                }
            }
            DefinitionEditor::App(editor) => {
                if let Err(message) = validate_app_form(&editor) {
                    self.push_activity(message);
                    self.form = Some(DefinitionEditor::App(editor));
                    return;
                }
                let args = app_form_command(&editor);
                if self.run_and_report_status(&args) {
                    let name = editor.name.trim().to_string();
                    self.load_apps();
                    if let Some(index) = self.apps.iter().position(|row| row.name == name) {
                        self.app_index = index;
                    }
                } else {
                    self.form = Some(DefinitionEditor::App(editor));
                }
            }
            DefinitionEditor::Config(editor) => {
                if editor.kind == FormKind::Add && editor.key.trim().is_empty() {
                    self.push_activity("A setting key is required.".to_string());
                    self.form = Some(DefinitionEditor::Config(editor));
                    return;
                }
                let key = editor.key.trim().to_string();
                let args = config_form_command(&editor);
                if self.run_and_report_status(&args) {
                    self.load_config();
                    if let Some(index) = self.config_rows.iter().position(|row| row.key == key) {
                        self.config_index = index;
                    }
                } else {
                    self.form = Some(DefinitionEditor::Config(editor));
                }
            }
        }
    }

    fn rebuild_coverage(&mut self) {
        self.coverage.clear();
        self.coverage_error = self.test_error.clone();
        if self.test_error.is_some() {
            return;
        }

        let mut stages: BTreeMap<String, (BTreeSet<String>, Vec<String>, usize)> = BTreeMap::new();
        let mut theatre = None;
        for row in &self.tests {
            match row.kind {
                TestKind::Theatre => theatre = Some(row.name.clone()),
                TestKind::Stage => {
                    let entry = stages.entry(row.name.clone()).or_default();
                    entry.0.extend(row.platforms.iter().cloned());
                    entry.2 += 1;
                    if let Some(name) = &theatre {
                        entry.1.push(name.clone());
                    }
                }
                _ => {}
            }
        }
        for (_, theatres, _) in stages.values_mut() {
            theatres.dedup();
        }

        for (stage, (platforms, theatres, implementations)) in stages {
            let missing = self
                .devices
                .iter()
                .filter(|device| !platforms.contains(device_platform_slug(&device.platform)))
                .map(|device| format!("{} ({})", device.name, device.platform))
                .collect();
            self.coverage.push(CoverageRow {
                stage,
                theatres,
                implementations,
                platforms,
                missing,
            });
        }
    }

    /// Number of scrollable lines in the Coverage body. Mirrors
    /// `render::draw_coverage`; keep the two in sync.
    pub(super) fn coverage_max_scroll(&self) -> u16 {
        let mut lines = 2; // summary + blank
        if self.coverage_error.is_some() || self.coverage.is_empty() {
            lines += 1;
        } else {
            lines += 2; // legend + header
            lines += self.coverage.len() * 2;
            lines += self
                .coverage
                .iter()
                .filter(|row| !row.missing.is_empty())
                .count();
        }
        u16::try_from(lines.saturating_sub(1)).unwrap_or(u16::MAX)
    }

    /// Number of scrollable lines in the Probe body. Mirrors
    /// `render::draw_probe`; keep the two in sync.
    pub(super) fn probe_max_scroll(&self) -> u16 {
        let mut lines = 2; // summary + blank
        if self.probe.is_empty() {
            lines += 1;
        } else {
            lines += 1; // header
            for row in &self.probe {
                lines += 1;
                if row.error.is_none() {
                    lines += row.sources.len();
                }
            }
        }
        u16::try_from(lines.saturating_sub(1)).unwrap_or(u16::MAX)
    }

    /// Number of scrollable lines in the Resident body.
    ///
    /// Mirrors the line construction in `render::draw_resident`; keep the two
    /// in step when adding a section there.
    pub(super) fn resident_max_scroll(&self) -> u16 {
        let mut lines = 0usize;
        if self.resident_error.is_some() {
            lines += 1;
        }
        if let Some(status) = &self.resident {
            lines += 3; // service, facts, blank
            if status.restart_required {
                lines += 1;
            }
            if status.heartbeat_stale {
                lines += 1;
            }
            lines += 1; // schedules heading
            if status.schedules.is_empty() {
                lines += 1;
            } else {
                lines += 1 + status.schedules.len(); // column header + rows
            }
            lines += 1; // blank
            lines += 1; // watchers heading
            if status.watchers.is_empty() {
                lines += 1;
            } else {
                lines += status.watchers.len() * 2; // summary + detail
            }
            lines += 1; // blank
            lines += 1; // dumps heading
            if self.resident_dumps.is_empty() {
                lines += 1;
            } else {
                lines += self.resident_dumps.len().min(8);
            }
        } else if self.resident_error.is_none() {
            lines += 1;
        }
        u16::try_from(lines.saturating_sub(1)).unwrap_or(u16::MAX)
    }

    pub(super) fn load_tests(&mut self) {
        self.tests.clear();
        self.test_error = None;
        let mut command = match child_command(&self.ctx) {
            Ok(command) => command,
            Err(error) => {
                self.test_error = Some(error);
                return;
            }
        };
        command.args(["tests", "--json"]);
        let output = match command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(output) => output,
            Err(error) => {
                self.test_error = Some(format!("cannot run tests explorer: {error}"));
                return;
            }
        };
        if !output.status.success() {
            self.test_error = Some(format!(
                "tests explorer failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
            return;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                self.test_error = Some(format!("cannot parse tests explorer JSON: {error}"));
                return;
            }
        };
        self.tests = rows_from_tree(&value);
    }

    pub(super) fn reload_screen(&mut self) {
        match self.screen {
            Screen::Tests => self.load_tests(),
            Screen::Coverage => self.load_coverage(),
            Screen::Probe => self.load_probe(),
            Screen::Groups => self.load_groups(),
            Screen::Devices => self.load_devices(),
            Screen::Apps => self.load_apps(),
            Screen::Config => self.load_config(),
            Screen::Resident => self.load_resident(),
            Screen::Landing | Screen::Menu | Screen::Home | Screen::Runs => {}
        }
    }

    pub(super) fn select_next_job(&mut self) {
        let visible = self.visible_job_indices();
        let Some(position) = visible.iter().position(|index| *index == self.job_index) else {
            if let Some(first) = visible.first() {
                self.job_index = *first;
            }
            return;
        };
        if let Some(next) = visible.get(position.saturating_add(1)) {
            self.job_index = *next;
        }
    }

    pub(super) fn select_previous_job(&mut self) {
        let visible = self.visible_job_indices();
        let Some(position) = visible.iter().position(|index| *index == self.job_index) else {
            if let Some(first) = visible.first() {
                self.job_index = *first;
            }
            return;
        };
        if let Some(previous) = position.checked_sub(1).and_then(|index| visible.get(index)) {
            self.job_index = *previous;
        }
    }

    pub(super) fn select_next_test(&mut self) {
        if self.tests.is_empty() {
            return;
        }
        self.test_index = (self.test_index + 1).min(self.tests.len().saturating_sub(1));
    }

    pub(super) fn select_previous_test(&mut self) {
        self.test_index = self.test_index.saturating_sub(1);
    }

    pub(super) fn follow_selected(&mut self) {
        let Some(record) = self.selected_job() else {
            self.push_activity("No job selected.".to_string());
            return;
        };
        let id = record.id.clone();
        self.follow = Some(id.clone());
        self.follow_started = None;
        self.follow_notified = false;
        self.refresh_follow(&id);
        self.screen = Screen::Runs;
        self.focus = Focus::Content;
        self.push_activity(format!("Following job {id}."));
    }

    pub(super) fn stop_selected(&mut self) {
        let id = match &self.follow {
            Some(id) => id.clone(),
            None => match self.selected_job() {
                Some(record) => record.id.clone(),
                None => {
                    self.push_activity("No job selected.".to_string());
                    return;
                }
            },
        };
        match self.store.request_stop(&id) {
            Ok(true) => {
                self.push_activity(format!("Stop requested for job {id}."));
            }
            Ok(false) => {
                self.push_activity(format!("Job {id} is already finished."));
            }
            Err(error) => self.push_activity(format!("stop: {error}")),
        }
    }

    pub(super) fn remove_selected(&mut self) {
        let Some(record) = self.selected_job() else {
            return;
        };
        let id = record.id.clone();
        match self.store.remove(&id) {
            Ok(()) => {
                if self.follow.as_deref() == Some(id.as_str()) {
                    self.follow = None;
                }
                self.push_activity(format!("Removed job {id}."));
                self.jobs = self.store.list();
                self.job_index = self.job_index.min(self.jobs.len().saturating_sub(1));
            }
            Err(error) => self.push_activity(format!("rm: {error}")),
        }
    }

    pub(super) fn detach(&mut self) {
        if let Some(id) = self.follow.take() {
            self.follow_started = None;
            self.follow_notified = false;
            self.events.clear();
            self.tree.clear();
            self.current_command.clear();
            self.logs.clear();
            self.push_activity(format!("Detached {id} · reattach with /attach {id}."));
        } else if let Some(record) = self.selected_job() {
            self.push_activity(format!(
                "Job {} is already running in the background; press Enter to view it.",
                record.id
            ));
        } else {
            self.push_activity("No job selected.".to_string());
        }
        self.focus = Focus::Prompt;
    }

    pub(super) fn toggle_log_follow(&mut self) {
        self.log_follow = !self.log_follow;
        self.push_activity(if self.log_follow {
            "Log tail resumed.".to_string()
        } else {
            "Log tail paused; press f to follow new lines again.".to_string()
        });
    }

    pub(super) fn cycle_device_filter(&mut self) {
        let mut names: Vec<String> = match &self.follow {
            Some(id) => self
                .store
                .read(id)
                .ok()
                .flatten()
                .map(|record| record.devices)
                .unwrap_or_default(),
            None => self
                .selected_job()
                .map(|record| record.devices.clone())
                .unwrap_or_default(),
        };
        if names.is_empty() {
            names = self
                .devices
                .iter()
                .map(|device| device.name.clone())
                .collect();
        }
        names.sort();
        names.dedup();
        if names.is_empty() {
            self.push_activity("No devices to filter.".to_string());
            return;
        }
        let next = match &self.device_filter {
            None => names.first().cloned(),
            Some(current) => {
                let position = names.iter().position(|name| name == current);
                position
                    .and_then(|index| names.get(index.saturating_add(1)))
                    .cloned()
            }
        };
        self.device_filter = next.clone();
        if let Some(id) = self.follow.clone() {
            self.refresh_follow(&id);
        }
        match next {
            Some(name) => self.push_activity(format!(
                "Device filter: {name} (t cycles, / sets a text filter)."
            )),
            None => self.push_activity("Device filter cleared.".to_string()),
        }
    }

    pub(super) fn begin_filter(&mut self) {
        self.filter_input = Some(self.filter.clone());
        self.focus = Focus::Content;
    }

    pub(super) fn apply_filter(&mut self) {
        if let Some(input) = self.filter_input.take() {
            self.filter = input.trim().to_string();
            self.clamp_selection();
            self.push_activity(if self.filter.is_empty() {
                "Filter cleared.".to_string()
            } else {
                format!("Filter: {}", self.filter)
            });
        }
    }

    pub(super) fn cancel_filter(&mut self) {
        self.filter_input = None;
    }

    pub(super) fn create_first_workspace(&mut self) {
        let root = self.ctx.base.display().to_string();
        let args = vec![
            "workspace".to_string(),
            "new".to_string(),
            root,
            "--example".to_string(),
        ];
        match self.run_child(&args) {
            Ok(output) => {
                let text = format!(
                    "{}{}",
                    trim_output(&output.stdout),
                    trim_output(&output.stderr)
                );
                self.push_activity(text);
                if output.status.success() {
                    self.first_run = false;
                    self.first_run_prompt = false;
                    self.overlay = None;
                    self.load_devices();
                    self.load_apps();
                    self.load_config();
                    self.load_tests();
                    self.refresh_completion_context();
                }
            }
            Err(message) => self.push_activity(message),
        }
    }

    pub(super) fn open_report(&mut self, screenshot: bool) -> Action {
        let Some(record) = self.selected_job() else {
            self.push_activity("No job selected.".to_string());
            return Action::None;
        };
        let id = record.id.clone();
        if screenshot {
            match self.screenshot_path(&id) {
                Some(path) => Action::OpenExternally(path),
                None => {
                    self.push_activity(format!(
                        "Job {id} has no screenshot (the report has no failure artifact)."
                    ));
                    Action::None
                }
            }
        } else {
            let report = self.store.dir(&id).join("report.html");
            if report.is_file() {
                Action::OpenExternally(report)
            } else {
                let json = self.store.report_path(&id);
                if json.is_file() {
                    Action::OpenExternally(json)
                } else {
                    self.push_activity(format!("Job {id} has no report yet."));
                    Action::None
                }
            }
        }
    }

    fn screenshot_path(&self, id: &str) -> Option<PathBuf> {
        let report = self.store.read_report(id).ok()?;
        for device in &report.devices {
            for play in &device.plays {
                for act in &play.acts {
                    for attempt in &act.attempts {
                        if let Some(failure) = &attempt.failure {
                            if let Some(path) = &failure.screenshot {
                                let candidate = PathBuf::from(path);
                                if candidate.is_absolute() {
                                    return Some(candidate);
                                }
                                if let Some(parent) = self.store.report_path(id).parent() {
                                    let joined = parent.join(&candidate);
                                    if joined.is_file() {
                                        return Some(joined);
                                    }
                                }
                                return Some(self.ctx.base.join(candidate));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    pub(super) fn rerun(&mut self, failed_only: bool) {
        let Some(record) = self.selected_job().cloned() else {
            self.push_activity("No job selected.".to_string());
            return;
        };
        let mut tokens = if let Some(last) = &self.last_run_tokens {
            last.clone()
        } else {
            match self.tokens_from_record(&record, failed_only) {
                Ok(tokens) => tokens,
                Err(message) => {
                    self.push_activity(message);
                    return;
                }
            }
        };
        if failed_only {
            if let Some(tokens_from_report) = self.failed_tokens(&record) {
                tokens = tokens_from_report;
            }
        }
        self.submit_run(tokens);
    }

    fn tokens_from_record(
        &self,
        record: &JobRecord,
        _failed_only: bool,
    ) -> Result<Vec<String>, String> {
        let outcome = cli::load_effective_config(&self.ctx.base, &self.ctx.config_path);
        let Some(config) = outcome.config else {
            return Err("Cannot rerun: the workspace config has errors.".to_string());
        };
        let loaded = workspace::load_plays(&self.ctx.base.join(&config.plays_dir));
        let mut tokens = Vec::new();
        for name in &record.plays {
            let path = loaded
                .workspace
                .plays
                .iter()
                .find(|play| &play.name == name)
                .map(|play| play.path.display().to_string())
                .unwrap_or_else(|| name.clone());
            tokens.push("--play".to_string());
            tokens.push(path);
        }
        for device in &record.devices {
            tokens.push("--device".to_string());
            tokens.push(device.clone());
        }
        if tokens.is_empty() {
            return Err(
                "Cannot rerun: the job has no recorded plays or devices; run it from the prompt."
                    .to_string(),
            );
        }
        Ok(tokens)
    }

    fn failed_tokens(&mut self, record: &JobRecord) -> Option<Vec<String>> {
        let report = self.store.read_report(&record.id).ok()?;
        let mut plays = BTreeSet::new();
        let mut acts = BTreeSet::new();
        for device in &report.devices {
            for play in &device.plays {
                for act in &play.acts {
                    if act.status == Status::Failed {
                        plays.insert(play.file.clone());
                        acts.insert(act.name.clone());
                    }
                }
            }
        }
        if plays.is_empty() && acts.is_empty() {
            self.push_activity(format!(
                "Job {} has no failed acts; nothing to retry.",
                record.id
            ));
            return None;
        }
        let mut tokens = Vec::new();
        for play in plays {
            tokens.push("--play".to_string());
            tokens.push(play);
        }
        for act in acts {
            tokens.push("--act".to_string());
            tokens.push(act);
        }
        for device in &record.devices {
            tokens.push("--device".to_string());
            tokens.push(device.clone());
        }
        Some(tokens)
    }

    pub(super) fn submit_run(&mut self, tokens: Vec<String>) {
        let mut filtered = Vec::new();
        let mut dry_run = false;
        for token in tokens {
            match token.as_str() {
                "run" | "--detach" | "--no-tui" | "--plain" | "--no-color" => {}
                "--dry-run" => {
                    dry_run = true;
                    filtered.push(token);
                }
                _ => filtered.push(token),
            }
        }
        let mut args = vec!["run".to_string()];
        args.extend(filtered);
        if !dry_run {
            args.push("--detach".to_string());
        }
        let before: BTreeSet<String> = self.store.list().into_iter().map(|r| r.id).collect();
        match self.run_child(&args) {
            Ok(output) => {
                let stdout = trim_output(&output.stdout);
                let stderr = trim_output(&output.stderr);
                if dry_run {
                    let plan = [stdout, stderr]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !plan.is_empty() {
                        self.overlay = Some(format!("Run plan\n{plan}"));
                    }
                    return;
                }
                if !stdout.is_empty() {
                    self.push_activity(stdout);
                }
                if !stderr.is_empty() {
                    self.push_activity(stderr);
                }
                if !output.status.success() {
                    return;
                }
                self.last_run_tokens = Some(args);
                let after = self.store.list();
                if let Some(new) = after
                    .into_iter()
                    .find(|record| !before.contains(&record.id))
                {
                    self.follow = Some(new.id.clone());
                    self.follow_started = None;
                    self.follow_notified = false;
                    self.refresh_follow(&new.id);
                    self.screen = Screen::Runs;
                    self.focus = Focus::Content;
                    self.jobs = self.store.list();
                    if let Some(index) = self.jobs.iter().position(|r| r.id == new.id) {
                        self.job_index = index;
                    }
                }
            }
            Err(message) => self.push_activity(message),
        }
    }

    pub(super) fn run_selected_test(&mut self) {
        let Some(row) = self.selected_test().cloned() else {
            return;
        };
        match row.kind {
            TestKind::Play | TestKind::Act => {}
            _ => {
                self.push_activity(
                    "Select a play or act to run; scenes and stages are building blocks."
                        .to_string(),
                );
                return;
            }
        }
        let Some(file) = row.file else {
            self.push_activity("The selected row has no file path.".to_string());
            return;
        };
        let mut tokens = vec!["--play".to_string(), file.display().to_string()];
        if row.kind == TestKind::Act {
            tokens.push("--act".to_string());
            tokens.push(row.name.clone());
        }
        for index in &self.test_marks {
            if let Some(marked) = self.tests.get(*index) {
                if let (Some(marked_file), TestKind::Play | TestKind::Act) =
                    (marked.file.as_ref(), marked.kind)
                {
                    tokens.push("--play".to_string());
                    tokens.push(marked_file.display().to_string());
                    if marked.kind == TestKind::Act {
                        tokens.push("--act".to_string());
                        tokens.push(marked.name.clone());
                    }
                }
            }
        }
        self.submit_run(tokens);
    }

    pub(super) fn dry_run_selected_test(&mut self) {
        let Some(row) = self.selected_test().cloned() else {
            return;
        };
        let Some(file) = row.file else {
            self.push_activity("The selected row has no file path.".to_string());
            return;
        };
        let mut args = vec![
            "tests".to_string(),
            "--dry-run".to_string(),
            "--play".to_string(),
            file.display().to_string(),
        ];
        if row.kind == TestKind::Act {
            args.push("--act".to_string());
            args.push(row.name);
        }
        match self.run_child(&args) {
            Ok(output) => {
                let text = format!(
                    "{}{}",
                    trim_output(&output.stdout),
                    trim_output(&output.stderr)
                );
                if text.is_empty() {
                    self.push_activity("The dry run produced no output.".to_string());
                } else {
                    self.overlay = Some(format!("Run plan\n{text}"));
                }
            }
            Err(message) => self.push_activity(message),
        }
    }

    pub(super) fn open_selected_test(&mut self) -> Action {
        self.selected_test()
            .and_then(|row| row.file.clone())
            .map_or(Action::None, Action::OpenEditor)
    }

    pub(super) fn execute_prompt(&mut self) -> Action {
        if let Some(search) = self.search.take() {
            if let Some(current) = search.current().cloned() {
                self.prompt.set_text(&current);
            }
            return Action::None;
        }
        let line = self.prompt.text();
        let line = line.trim().to_string();
        if line.is_empty() {
            return Action::None;
        }
        self.prompt.clear();
        self.history.add(&line);
        self.save_history();
        if line.starts_with('/') {
            return self.execute_slash(&line);
        }
        self.execute_cli_line(&line)
    }

    fn execute_slash(&mut self, line: &str) -> Action {
        let trimmed = line.trim_start_matches('/').trim();
        let (command, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((command, rest)) => (command, rest.trim()),
            None => (trimmed, ""),
        };
        match command {
            "quit" | "q" => {
                self.should_quit = true;
                Action::Quit
            }
            "help" | "?" => {
                self.overlay = Some(help_text());
                Action::None
            }
            "clear" => {
                self.activity.clear();
                Action::None
            }
            "home" | "menu" => {
                self.screen = if command == "home" {
                    Screen::Home
                } else {
                    Screen::Menu
                };
                self.focus = Focus::Content;
                Action::None
            }
            "workspaces" | "landing" => {
                self.screen = Screen::Landing;
                self.focus = Focus::Content;
                self.load_workspaces();
                Action::None
            }
            "runs" | "jobs" => {
                self.screen = Screen::Runs;
                self.focus = Focus::Content;
                self.jobs = self.store.list();
                Action::None
            }
            "tests" => {
                self.screen = Screen::Tests;
                self.focus = Focus::Content;
                if rest.is_empty() {
                    self.load_tests();
                    Action::None
                } else {
                    let mut args = vec!["tests".to_string()];
                    args.extend(split_args(rest));
                    self.run_and_report(&args);
                    self.load_tests();
                    Action::None
                }
            }
            "coverage" => {
                self.set_screen(Screen::Coverage);
                Action::None
            }
            "probe" => {
                self.set_screen(Screen::Probe);
                Action::None
            }
            "groups" => {
                self.set_screen(Screen::Groups);
                Action::None
            }
            "devices" => {
                self.screen = Screen::Devices;
                self.focus = Focus::Content;
                self.load_devices();
                Action::None
            }
            "apps" => {
                self.screen = Screen::Apps;
                self.focus = Focus::Content;
                self.load_apps();
                Action::None
            }
            "config" => {
                self.screen = Screen::Config;
                self.focus = Focus::Content;
                self.load_config();
                Action::None
            }
            "resident" | "service" => {
                self.set_screen(Screen::Resident);
                Action::None
            }
            "run" => {
                let tokens = split_args(rest);
                self.submit_run(tokens);
                Action::None
            }
            "attach" => {
                if let Some(id) = rest.split_whitespace().next() {
                    self.follow = Some(id.to_string());
                    self.follow_started = None;
                    self.follow_notified = false;
                    self.refresh_follow(id);
                    self.screen = Screen::Runs;
                    self.focus = Focus::Content;
                } else {
                    self.push_activity("Usage: /attach <job-id>".to_string());
                }
                Action::None
            }
            "open" => {
                if rest.is_empty() {
                    self.push_activity("Usage: /open <path>".to_string());
                    Action::None
                } else {
                    let path = PathBuf::from(rest);
                    let path = if path.is_absolute() {
                        path
                    } else {
                        self.ctx.base.join(path)
                    };
                    Action::OpenExternally(path)
                }
            }
            "fmt" => {
                let mut args = vec!["fmt".to_string()];
                args.extend(split_args(rest));
                self.run_and_report(&args);
                if rest.is_empty() {
                    self.load_tests();
                    self.refresh_completion_context();
                }
                Action::None
            }
            "validate" => {
                let mut args = vec!["validate".to_string()];
                args.extend(split_args(rest));
                self.run_and_report(&args);
                Action::None
            }
            "plan" => {
                let mut args = vec!["run".to_string(), "--dry-run".to_string()];
                args.extend(split_args(rest));
                self.run_and_report(&args);
                Action::None
            }
            "doctor" => {
                let mut args = vec!["doctor".to_string()];
                args.extend(split_args(rest));
                self.run_and_report(&args);
                Action::None
            }
            "profile" | "stop" | "rm" | "logs" | "report" => {
                let mut args = vec![command.to_string()];
                args.extend(split_args(rest));
                self.run_and_report(&args);
                Action::None
            }
            "history" => {
                let text = self
                    .history
                    .entries
                    .iter()
                    .rev()
                    .take(40)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                self.push_activity(text);
                Action::None
            }
            other => {
                self.push_activity(format!(
                    "Unknown slash command '/{other}'. Press ? for help."
                ));
                Action::None
            }
        }
    }

    fn execute_cli_line(&mut self, line: &str) -> Action {
        let mut tokens = split_args(line);
        if tokens
            .first()
            .is_some_and(|token| token == "impresario" || token.ends_with("impresario.exe"))
        {
            tokens.remove(0);
        }
        if tokens.is_empty() {
            return Action::None;
        }
        if tokens.first().is_some_and(|token| token == "wait") {
            self.push_activity(
                "`wait` is not available in the TUI; use `impresario wait <id>` in a terminal."
                    .to_string(),
            );
            return Action::None;
        }
        if tokens
            .first()
            .is_some_and(|token| matches!(token.as_str(), "lsp" | "mcp" | "mock-agent" | "daemon"))
        {
            self.push_activity(
                "That command runs a server and is not available inside the TUI.".to_string(),
            );
            return Action::None;
        }
        if tokens.first().is_some_and(|token| token == "run") {
            self.submit_run(tokens);
            return Action::None;
        }
        self.run_and_report(&tokens);
        if tokens.first().is_some_and(|token| token == "tests") {
            self.load_tests();
        }
        Action::None
    }

    fn run_and_report(&mut self, args: &[String]) {
        self.run_and_report_status(args);
    }

    fn run_and_report_status(&mut self, args: &[String]) -> bool {
        match self.run_child(args) {
            Ok(output) => {
                let mut text = trim_output(&output.stdout);
                let stderr = trim_output(&output.stderr);
                if !stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                if text.is_empty() {
                    text = format!("command exited with {}", output.status);
                }
                self.push_activity(text);
                output.status.success()
            }
            Err(message) => {
                self.push_activity(message);
                false
            }
        }
    }

    fn run_child(&self, args: &[String]) -> Result<Output, String> {
        let mut command = child_command(&self.ctx)?;
        command.args(args);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("cannot start impresario: {error}"))
    }

    pub(super) fn push_activity(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        for line in text.lines() {
            self.activity.push_back(line.to_string());
        }
        while self.activity.len() > ACTIVITY_LIMIT {
            self.activity.pop_front();
        }
    }

    fn save_history(&mut self) {
        if let Some(parent) = self.history_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = format!("{}\n", self.history.entries.join("\n"));
        if std::fs::write(&self.history_path, text).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if let Ok(metadata) = std::fs::metadata(&self.history_path) {
                    let mut permissions = metadata.permissions();
                    permissions.set_mode(0o600);
                    let _ = std::fs::set_permissions(&self.history_path, permissions);
                }
            }
        }
        self.history.limit = self.history_limit;
    }

    pub(super) fn start_reverse_search(&mut self) {
        let mut search = ReverseSearch::new(&self.history);
        search.original = self.prompt.text();
        self.search = Some(search);
    }

    pub(super) fn reverse_search_input(&mut self, character: char) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.query.push(character);
        search.index = 0;
        search.update(&self.history);
    }

    pub(super) fn reverse_search_backspace(&mut self) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let _ = search.query.pop();
        search.index = 0;
        search.update(&self.history);
    }

    pub(super) fn reverse_search_next(&mut self) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if !search.matches.is_empty() {
            search.index = (search.index + 1) % search.matches.len();
        }
    }

    pub(super) fn reverse_search_cancel(&mut self) {
        if let Some(search) = self.search.take() {
            self.prompt.set_text(&search.original);
        }
    }

    pub(super) fn accept_reverse_search(&mut self) {
        if let Some(search) = self.search.take() {
            if let Some(current) = search.current().cloned() {
                self.prompt.set_text(&current);
            } else {
                self.prompt.set_text(&search.original);
            }
        }
    }

    pub(super) fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.focus = Focus::Content;
        self.form = None;
        if screen == Screen::Resident {
            self.resident_scroll = 0;
        }
        if screen == Screen::Coverage {
            self.coverage_scroll = 0;
        }
        if screen == Screen::Probe {
            self.probe_scroll = 0;
        }
        if screen == Screen::Groups {
            self.group_index = 0;
            self.group_scroll = 0;
            self.group_editor = None;
        }
        if screen == Screen::Devices {
            self.device_index = 0;
        }
        if screen == Screen::Apps {
            self.app_index = 0;
        }
        if screen == Screen::Config {
            self.config_index = 0;
        }
        self.reload_screen();
    }

    pub(super) fn scroll(&mut self, delta: i32) {
        match self.screen {
            Screen::Landing => {
                self.move_landing(delta);
            }
            Screen::Menu => {
                self.move_menu(delta);
            }
            Screen::Runs => {
                if delta < 0 {
                    self.select_previous_job();
                } else {
                    self.select_next_job();
                }
            }
            Screen::Tests => {
                if delta < 0 {
                    self.select_previous_test();
                } else {
                    self.select_next_test();
                }
            }
            Screen::Resident => {
                let max = self.resident_max_scroll();
                if delta < 0 {
                    self.resident_scroll = self.resident_scroll.min(max).saturating_sub(1);
                } else {
                    self.resident_scroll = self.resident_scroll.saturating_add(1).min(max);
                }
            }
            Screen::Coverage => {
                let max = self.coverage_max_scroll();
                if delta < 0 {
                    self.coverage_scroll = self.coverage_scroll.min(max).saturating_sub(1);
                } else {
                    self.coverage_scroll = self.coverage_scroll.saturating_add(1).min(max);
                }
            }
            Screen::Probe => {
                let max = self.probe_max_scroll();
                if delta < 0 {
                    self.probe_scroll = self.probe_scroll.min(max).saturating_sub(1);
                } else {
                    self.probe_scroll = self.probe_scroll.saturating_add(1).min(max);
                }
            }
            Screen::Groups => {
                let last = self.groups.len().saturating_sub(1);
                if delta < 0 {
                    self.group_index = self.group_index.saturating_sub(1);
                } else {
                    self.group_index = self.group_index.saturating_add(1).min(last);
                }
                self.group_scroll = self.group_scroll_for_index();
            }
            Screen::Devices => move_index(&mut self.device_index, self.devices.len(), delta),
            Screen::Apps => move_index(&mut self.app_index, self.apps.len(), delta),
            Screen::Config => move_index(&mut self.config_index, self.config_rows.len(), delta),
            Screen::Home => {}
        }
    }
}

fn move_index(index: &mut usize, len: usize, delta: i32) {
    if len == 0 {
        *index = 0;
        return;
    }
    if delta < 0 {
        *index = index.saturating_sub(1);
    } else {
        *index = index.saturating_add(1).min(len.saturating_sub(1));
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => {
            #[cfg(windows)]
            {
                left.to_string_lossy()
                    .eq_ignore_ascii_case(&right.to_string_lossy())
            }
            #[cfg(not(windows))]
            {
                false
            }
        }
    }
}

fn child_command(ctx: &Context) -> Result<Command, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the impresario executable: {error}"))?;
    let mut command = Command::new(executable);
    command
        .arg("--workspace")
        .arg(&ctx.base)
        .arg("--config")
        .arg(&ctx.config_path)
        .arg("--devices-config")
        .arg(&ctx.devices_path)
        .arg("--no-tui")
        .arg("--plain")
        .arg("--no-color")
        .env("IMPRESARIO_NO_PATH_INSTALL", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

fn trim_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_end();
    let mut result: String = text.chars().take(20_000).collect();
    if text.chars().count() > 20_000 {
        result.push_str("\n... output truncated ...");
    }
    result
}

fn history_limit(ctx: &Context) -> usize {
    // The typed config does not carry `ui.*`; read the workspace setting file
    // directly so a deliberately small history is honoured.
    let file = settings::SettingsFile::load(&ctx.config_path);
    match file.values.get("ui.history_size") {
        Some(SettingValue::Int(value)) if *value > 0 => {
            usize::try_from(*value).unwrap_or(HISTORY_FALLBACK)
        }
        _ => HISTORY_FALLBACK,
    }
}

fn effective_settings(ctx: &Context) -> Vec<(String, String, String)> {
    let global = match impresario_core::home::Home::resolve() {
        Ok(home) => settings::SettingsFile::load(&home.config_path()),
        Err(_) => settings::SettingsFile::empty("<no home>"),
    };
    let workspace = settings::SettingsFile::load(&ctx.config_path);
    let effective = settings::EffectiveSettings::resolve(&global, &workspace);
    effective
        .iter()
        .map(|(key, value, source)| (key.to_string(), setting_value(value), source.to_string()))
        .collect()
}

fn setting_value(value: &SettingValue) -> String {
    match value {
        SettingValue::Text(text) | SettingValue::Enum(text) => text.clone(),
        SettingValue::Bool(boolean) => boolean.to_string(),
        SettingValue::Int(number) => number.to_string(),
        SettingValue::List(items) => items.join(","),
    }
}

fn event_device(event: &ExecEvent) -> Option<&str> {
    match event {
        ExecEvent::RunStarted { .. } | ExecEvent::RunFinished { .. } => None,
        ExecEvent::DeviceStarted { device, .. }
        | ExecEvent::ActStarted { device, .. }
        | ExecEvent::ActFinished { device, .. }
        | ExecEvent::StepStarted { device, .. }
        | ExecEvent::StepFinished { device, .. }
        | ExecEvent::LogLine { device, .. }
        | ExecEvent::DeviceFinished { device, .. } => Some(device),
    }
}

pub(super) fn first_run_text(ctx: &Context) -> String {
    [
        "Welcome to impresario".to_string(),
        String::new(),
        format!("workspace  {}", ctx.base.display()),
        format!("global home {}", ctx.home.root().display()),
        String::new(),
        "No workspace config was found yet. Create the workspace skeleton now?".to_string(),
        String::new(),
        "  [y] create impresario.config, plays/, scenes/, theatres/ and an example play"
            .to_string(),
        "  [n] or Esc skip; you can create it later with:".to_string(),
        "      impresario workspace new . --example".to_string(),
        String::new(),
        "After creating it, use /doctor to check devices and /tests to explore the DSL."
            .to_string(),
        "Press ? at any time for keys and slash commands.".to_string(),
    ]
    .join("\n")
}

fn status_of_job(status: JobStatus) -> LineStatus {
    match status {
        JobStatus::Passed => LineStatus::Passed,
        JobStatus::Failed | JobStatus::Stopped => LineStatus::Failed,
        JobStatus::Error | JobStatus::Interrupted => LineStatus::Failed,
        JobStatus::Queued | JobStatus::Running => LineStatus::Running,
    }
}

fn status_of_str(status: &str) -> LineStatus {
    if status.eq_ignore_ascii_case("passed") {
        LineStatus::Passed
    } else if status.eq_ignore_ascii_case("skipped") {
        LineStatus::Skipped
    } else if status.eq_ignore_ascii_case("aborted") {
        LineStatus::Aborted
    } else if status.eq_ignore_ascii_case("unverifiable") {
        LineStatus::Unverifiable
    } else if status.eq_ignore_ascii_case("failed") || status.eq_ignore_ascii_case("error") {
        LineStatus::Failed
    } else {
        LineStatus::Running
    }
}

/// Build one probe support row from a configured device.
pub(super) fn probe_row(device: &DeviceRow) -> ProbeRow {
    match device_platform_slug(&device.platform).parse::<Platform>() {
        Ok(platform) => {
            let support = probe::support(platform);
            ProbeRow {
                device: device.name.clone(),
                platform: platform.as_str().to_string(),
                showing: support.showing,
                player: support.player,
                drm: support.drm,
                strongest: support.strongest_class().as_str().to_string(),
                properties: support.properties.len(),
                providers: support
                    .providers
                    .iter()
                    .map(|provider| provider.as_str().to_string())
                    .collect(),
                sources: support
                    .sources
                    .iter()
                    .map(|source| ProbeSourceRow {
                        source: source.source.as_str().to_string(),
                        class: source.source.evidence_class().as_str().to_string(),
                        available: source.available,
                        reason: source.reason.clone(),
                    })
                    .collect(),
                error: None,
            }
        }
        Err(()) => ProbeRow {
            device: device.name.clone(),
            platform: device.platform.clone(),
            showing: false,
            player: false,
            drm: false,
            strongest: String::new(),
            properties: 0,
            providers: Vec::new(),
            sources: Vec::new(),
            error: Some(format!("unknown platform '{}'", device.platform)),
        },
    }
}

/// Normalize a configured device type (`webOS`, `tizenOS`, `iOS`, `tvOS`)
/// to the lowercase platform slug used by stage audiences.
fn device_platform_slug(platform: &str) -> &str {
    match platform {
        "webOS" => "webos",
        "tizenOS" => "tizen",
        "iOS" => "ios",
        "tvOS" => "tvos",
        other => other,
    }
}

fn rows_from_tree(value: &serde_json::Value) -> Vec<TestRow> {
    let mut rows = Vec::new();
    if let Some(scenes) = value.get("scenes").and_then(serde_json::Value::as_array) {
        for scene in scenes {
            let name = string_field(scene, "name").unwrap_or_default();
            rows.push(TestRow {
                indent: 0,
                label: format!("scene {name}"),
                kind: TestKind::Scene,
                file: string_field(scene, "file").map(PathBuf::from),
                line: scene
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .map(|n| n as usize),
                name: name.clone(),
                platforms: Vec::new(),
            });
            if let Some(pieces) = scene.get("pieces").and_then(serde_json::Value::as_array) {
                for piece in pieces {
                    let piece_name = string_field(piece, "name").unwrap_or_default();
                    rows.push(TestRow {
                        indent: 1,
                        label: format!("piece {piece_name}"),
                        kind: TestKind::Piece,
                        file: string_field(piece, "file").map(PathBuf::from),
                        line: piece
                            .get("line")
                            .and_then(serde_json::Value::as_u64)
                            .map(|n| n as usize),
                        name: piece_name,
                        platforms: Vec::new(),
                    });
                }
            }
        }
    }
    if let Some(theatres) = value.get("theatres").and_then(serde_json::Value::as_array) {
        for theatre in theatres {
            let name = string_field(theatre, "name").unwrap_or_default();
            rows.push(TestRow {
                indent: 0,
                label: format!("theatre {name}"),
                kind: TestKind::Theatre,
                file: string_field(theatre, "file").map(PathBuf::from),
                line: theatre
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .map(|n| n as usize),
                name: name.clone(),
                platforms: Vec::new(),
            });
            if let Some(stages) = theatre.get("stages").and_then(serde_json::Value::as_array) {
                for stage in stages {
                    let stage_name = string_field(stage, "name").unwrap_or_default();
                    rows.push(TestRow {
                        indent: 1,
                        label: format!("stage {stage_name}"),
                        kind: TestKind::Stage,
                        file: string_field(stage, "file").map(PathBuf::from),
                        line: stage
                            .get("line")
                            .and_then(serde_json::Value::as_u64)
                            .map(|n| n as usize),
                        name: stage_name,
                        platforms: string_array(stage, "platforms"),
                    });
                }
            }
        }
    }
    if let Some(plays) = value.get("plays").and_then(serde_json::Value::as_array) {
        for play in plays {
            let name = string_field(play, "name").unwrap_or_default();
            let file = string_field(play, "file").map(PathBuf::from);
            rows.push(TestRow {
                indent: 0,
                label: format!("play {name}"),
                kind: TestKind::Play,
                file: file.clone(),
                line: play
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .map(|n| n as usize),
                name: name.clone(),
                platforms: Vec::new(),
            });
            if let Some(acts) = play.get("acts").and_then(serde_json::Value::as_array) {
                for act in acts {
                    let act_name = string_field(act, "name").unwrap_or_default();
                    let tags = string_array(act, "tags");
                    let tags = if tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", tags.join(","))
                    };
                    rows.push(TestRow {
                        indent: 1,
                        label: format!("act {act_name}{tags}"),
                        kind: TestKind::Act,
                        file: file.clone(),
                        line: act
                            .get("line")
                            .and_then(serde_json::Value::as_u64)
                            .map(|n| n as usize),
                        name: act_name,
                        platforms: Vec::new(),
                    });
                }
            }
        }
    }
    rows
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn string_array(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) struct CompletionEngine;

impl CompletionEngine {
    pub(super) fn complete(text: &str, context: &[String]) -> Vec<String> {
        let query = text.trim_start();
        if !query.starts_with('/') {
            return Vec::new();
        }
        let mut unique = BTreeSet::new();
        for candidate in candidates() {
            unique.insert(candidate.to_string());
        }
        for candidate in context {
            unique.insert(candidate.clone());
        }
        let mut scored: Vec<(i32, String)> = unique
            .into_iter()
            .filter_map(|candidate| fuzzy_score(&candidate, query).map(|score| (score, candidate)))
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.len().cmp(&right.1.len()))
                .then_with(|| left.1.cmp(&right.1))
        });
        scored
            .into_iter()
            .take(12)
            .map(|(_, value)| value)
            .collect()
    }
}

fn candidates() -> Vec<&'static str> {
    vec![
        "/run",
        "/run --detach",
        "/run --dry-run",
        "/run --only-failed",
        "/run --tag ",
        "/run --exclude-tag ",
        "/run --act ",
        "/run --play ",
        "/run --device ",
        "/run --platform ",
        "/run --var ",
        "/run --arg ",
        "/run --profile ",
        "/tests",
        "/tests --json",
        "/tests --tag ",
        "/tests --exclude-tag ",
        "/tests --act ",
        "/tests --play ",
        "/tests --platform ",
        "/tests new scene ",
        "/tests new theatre ",
        "/tests new play ",
        "/tests show ",
        "/tests where ",
        "/tests grep ",
        "/tests rename ",
        "/tests open ",
        "/tests --dry-run",
        "/coverage",
        "/probe",
        "/groups",
        "/devices",
        "/apps",
        "/config",
        "/resident",
        "/doctor",
        "/doctor --json",
        "/fmt",
        "/validate",
        "/plan",
        "/plan --tag ",
        "/plan --device ",
        "/jobs",
        "/profile",
        "/attach ",
        "/logs ",
        "/report ",
        "/stop ",
        "/home",
        "/menu",
        "/workspaces",
        "/landing",
        "/runs",
        "/help",
        "/clear",
        "/quit",
    ]
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i32> {
    let candidate_lower = candidate.to_lowercase();
    let query_lower = query.to_lowercase();
    let mut score = 0i32;
    let mut position = 0usize;
    let mut previous: Option<usize> = None;
    for (query_index, needle) in query_lower.chars().enumerate() {
        let rest = candidate_lower.get(position..)?;
        let found = rest.find(needle)?;
        let index = position.saturating_add(found);
        if index == 0
            || candidate_lower
                .get(..index)
                .is_some_and(|prefix| prefix.ends_with(' '))
        {
            score += 12;
        }
        if previous.is_some_and(|previous| previous + 1 == index) {
            score += 8;
        }
        if query_index == 0 {
            score += 10;
        }
        score += 2;
        previous = Some(index);
        position = index.saturating_add(needle.len_utf8());
    }
    Some(score.saturating_sub(candidate_lower.len() as i32 / 4))
}

fn split_group_members(text: &str) -> Vec<String> {
    text.split(|character: char| character == ',' || character.is_whitespace())
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Comma/newline-separated form values; spaces stay inside a value (a launch
/// parameter or a path may contain them).
fn split_form_list(text: &str) -> Vec<String> {
    text.split(|character: char| character == ',' || character == '\n')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn push_scope(args: &mut Vec<String>, source: DefinitionSource) {
    if source == DefinitionSource::Workspace {
        args.push("--workspace".to_string());
    }
}

fn push_pair(args: &mut Vec<String>, flag: &str, value: &str) {
    args.push(flag.to_string());
    args.push(value.to_string());
}

fn cycle_field(field: &mut usize, fields: &[usize], backwards: bool) {
    if fields.is_empty() {
        return;
    }
    let position = fields
        .iter()
        .position(|candidate| candidate == field)
        .unwrap_or(0);
    let next = if backwards {
        (position + fields.len() - 1) % fields.len()
    } else {
        (position + 1) % fields.len()
    };
    *field = fields[next];
}

fn display_port(port: Option<u16>) -> String {
    port.filter(|port| *port != 5555)
        .map(|port| port.to_string())
        .unwrap_or_default()
}

fn device_display_port(row: &DeviceRow) -> String {
    if row.port.is_some() && matches!(row.platform.as_str(), "android" | "androidtv") {
        display_port(row.port)
    } else {
        String::new()
    }
}

fn form_port(text: &str) -> Option<u16> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        text.parse().ok()
    }
}

fn device_form_command(editor: &DeviceEditor) -> Vec<String> {
    let verb = editor.kind.verb("add", "edit");
    let mut args = vec!["device".to_string(), verb.to_string()];
    if editor.kind == FormKind::Remove {
        args.push(editor.name.trim().to_string());
    } else {
        push_pair(&mut args, "--type", editor.device_type.trim());
        push_pair(&mut args, "--ip", editor.ip.trim());
        if !editor.name.trim().is_empty() {
            push_pair(&mut args, "--name", editor.name.trim());
        }
        if let Some(port) = form_port(&editor.port) {
            push_pair(&mut args, "--port", &port.to_string());
        }
    }
    push_scope(&mut args, editor.source);
    args
}

fn validate_device_form(editor: &DeviceEditor) -> Result<(), String> {
    if editor.kind == FormKind::Remove {
        return Ok(());
    }
    if editor.device_type.trim().is_empty() {
        return Err(
            "A device type is required (web, webOS, tizenOS, android, androidtv, iOS or tvOS)."
                .to_string(),
        );
    }
    if editor.ip.trim().is_empty() {
        return Err("An IP address is required.".to_string());
    }
    if !editor.port.trim().is_empty() {
        match form_port(&editor.port) {
            Some(port) if port > 0 => {}
            _ => return Err("The port must be a number between 1 and 65535.".to_string()),
        }
    }
    Ok(())
}

fn app_editor_from(kind: FormKind, entry: &AppEntry) -> AppEditor {
    let app = &entry.app;
    let platforms = app
        .platforms
        .iter()
        .filter_map(|(platform, spec)| {
            if *platform == Platform::Web {
                spec.url
                    .as_ref()
                    .map(|url| format!("{}={url}", platform.as_str()))
            } else {
                spec.id
                    .as_ref()
                    .map(|id| format!("{}={id}", platform.as_str()))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let packages = app
        .platforms
        .iter()
        .filter_map(|(platform, spec)| {
            spec.package
                .as_ref()
                .map(|package| format!("{}={package}", platform.as_str()))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let launch_activities = app
        .platforms
        .iter()
        .filter_map(|(platform, spec)| {
            spec.launch_activity
                .as_ref()
                .map(|activity| format!("{}={activity}", platform.as_str()))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let launch_params = app
        .options
        .launch_params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    AppEditor {
        kind,
        name: app.name.clone(),
        description: app.description.clone().unwrap_or_default(),
        platforms,
        packages,
        launch_activities,
        launch_params,
        clear_data: app.options.clear_data,
        field: if kind == FormKind::Add { 0 } else { 1 },
        source: entry.source,
    }
}

fn app_form_command(editor: &AppEditor) -> Vec<String> {
    let verb = editor.kind.verb("add", "edit");
    let mut args = vec!["app".to_string(), verb.to_string()];
    if editor.kind == FormKind::Remove {
        args.push(editor.name.trim().to_string());
    } else {
        push_pair(&mut args, "--name", editor.name.trim());
        if !editor.description.trim().is_empty() {
            push_pair(&mut args, "--description", editor.description.trim());
        }
        for spec in split_form_list(&editor.platforms) {
            push_pair(&mut args, "--platform", &spec);
        }
        for spec in split_form_list(&editor.packages) {
            push_pair(&mut args, "--package", &spec);
        }
        for spec in split_form_list(&editor.launch_activities) {
            push_pair(&mut args, "--launch-activity", &spec);
        }
        for spec in split_form_list(&editor.launch_params) {
            push_pair(&mut args, "--launch-param", &spec);
        }
        if editor.clear_data {
            args.push("--clear-data".to_string());
        }
    }
    push_scope(&mut args, editor.source);
    args
}

fn validate_app_form(editor: &AppEditor) -> Result<(), String> {
    if editor.kind == FormKind::Remove {
        return Ok(());
    }
    if editor.name.trim().is_empty() {
        return Err("An app name is required.".to_string());
    }
    if editor.platforms.trim().is_empty() {
        return Err(
            "At least one platform is required, for example android=com.acme.shop or web=https://shop.example."
                .to_string(),
        );
    }
    Ok(())
}

fn config_row_source(row: &ConfigRow) -> DefinitionSource {
    if row.source.starts_with("global") {
        DefinitionSource::Global
    } else {
        DefinitionSource::Workspace
    }
}

/// A global-only setting cannot be written to the workspace layer, even when
/// its winning value is a built-in default.
fn config_target_source(key: &str, preferred: DefinitionSource) -> DefinitionSource {
    match settings::definition(key) {
        Some(def) if def.scope == settings::Scope::GlobalOnly => DefinitionSource::Global,
        _ => preferred,
    }
}

fn config_form_command(editor: &ConfigEditor) -> Vec<String> {
    let key = editor.key.trim();
    let mut args = match editor.kind {
        FormKind::Add | FormKind::Edit => vec![
            "config".to_string(),
            "set".to_string(),
            key.to_string(),
            editor.value.clone(),
        ],
        FormKind::Remove => vec!["config".to_string(), "unset".to_string(), key.to_string()],
    };
    let source = config_target_source(key, editor.source);
    args.push(
        if source == DefinitionSource::Global {
            "--global"
        } else {
            "--workspace"
        }
        .to_string(),
    );
    args
}

fn group_command(
    action: &str,
    name: &str,
    members: &[String],
    source: DefinitionSource,
) -> Vec<String> {
    let mut args = vec!["group".to_string(), action.to_string(), name.to_string()];
    args.extend(members.iter().cloned());
    push_scope(&mut args, source);
    args
}

/// Split a command line with single/double quotes and backslash escapes.
/// Newlines are treated as arg separators; malformed quotes fail loudly.
pub(super) fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut in_arg = false;
    for character in line.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            in_arg = true;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            in_arg = true;
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            in_arg = true;
            continue;
        }
        if character.is_whitespace() {
            if in_arg {
                args.push(std::mem::take(&mut current));
                in_arg = false;
            }
        } else {
            current.push(character);
            in_arg = true;
        }
    }
    if escaped {
        current.push('\\');
    }
    if in_arg {
        args.push(current);
    }
    args
}

pub(super) fn help_text() -> String {
    [
        "impresario interactive",
        "",
        "Workspaces · Up/Down, Enter opens; a add, d doctor, q quit.",
        "Workspace menu · Up/Down, Enter opens, Esc goes back.",
        "  Overview · Runs · Resident · Tests · Coverage · Probe · Groups · Devices · Apps · Config",
        "  Doctor · Switch workspace returns to the landing screen.",
        "Content · arrows move; Esc menu; Tab prompt.",
        "Prompt · type a CLI command; /run --tag smoke is a shortcut.",
        "  Tab completes · Ctrl-R search · Up/Down history · Alt+Enter newline",
        "  Ctrl-C clears (twice quits) · Ctrl-Q quits · Ctrl-Z suspends",
        "",
        "Runs · Enter follow · b detach · x stop · f logs · t device · / filter",
        "  r rerun · F rerun failed · o report · p screenshot · X remove",
        "Tests · Enter edit · Space mark · r run · d dry-run · j/k move",
        "Coverage / Probe · j/k scroll · g/G top/bottom · r reload",
        "Groups · j/k move · a add · Enter/e edit · x remove · r reload",
        "Devices / Apps / Config · j/k · a add · e edit/set · x remove/unset · o file · r reload",
        "Resident · j/k scroll · g/G top/bottom · r reload",
        "",
        "Commands · /run /jobs /devices /apps /config /resident /coverage /probe /tests /open /doctor",
        "  /groups /fmt /validate /plan /attach /logs /stop /report /profile /quit",
        "Scriptable · run --detach · jobs --json · wait · report",
        "  /menu /workspaces",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definitions::ResolvedDevice;

    #[test]
    fn fuzzy_completion_prefers_prefixes() {
        let values = CompletionEngine::complete("/run --on", &[]);
        assert!(values.iter().any(|value| value.contains("only-failed")));
    }

    #[test]
    fn completion_includes_workspace_names() {
        let context = vec!["/run --device Pixel 8".to_string()];
        let values = CompletionEngine::complete("/run --dev", &context);
        assert!(values.iter().any(|value| value == "/run --device Pixel 8"));
    }

    #[test]
    fn split_args_handles_quotes() {
        assert_eq!(
            split_args("run --play \"my play.play\" --tag 'smoke test'"),
            vec!["run", "--play", "my play.play", "--tag", "smoke test"]
        );
    }

    #[test]
    fn prompt_editing_keeps_unicode_boundaries() {
        let mut prompt = Prompt::default();
        prompt.insert_text("héllo");
        prompt.left();
        prompt.backspace();
        assert_eq!(prompt.text(), "hélo");
    }

    #[test]
    fn load_apps_reads_the_workspace_layer() {
        let dir = tempfile::tempdir().expect("temp workspace");
        std::fs::write(
            dir.path().join("impresario.apps.config"),
            "apps:\n  - name: shop\n    description: Storefront\n    platforms:\n      web: { url: 'https://shop.example' }\n",
        )
        .expect("apps file");
        let context = Context::new(
            impresario_core::home::Home::at(dir.path().join("home")),
            dir.path().to_path_buf(),
            dir.path().join("impresario.config"),
            dir.path().join("impresario.devices.config"),
            false,
        );
        let mut app = App::empty(context);
        app.load_apps();
        assert_eq!(app.apps.len(), 1);
        assert_eq!(app.apps.first().map(|row| row.name.as_str()), Some("shop"));
    }

    #[test]
    fn group_command_targets_the_selected_layer() {
        assert_eq!(
            group_command(
                "add",
                "phones",
                &["phone".to_string()],
                DefinitionSource::Workspace
            ),
            vec!["group", "add", "phones", "phone", "--workspace"]
        );
        assert_eq!(
            group_command(
                "set",
                "phones",
                &["phone".to_string()],
                DefinitionSource::Global
            ),
            vec!["group", "set", "phones", "phone"]
        );
        assert_eq!(
            group_command("remove", "phones", &[], DefinitionSource::Global),
            vec!["group", "remove", "phones"]
        );
    }

    #[test]
    fn split_group_members_accepts_commas_and_spaces() {
        assert_eq!(
            split_group_members("phone, @tv  web1"),
            vec!["phone", "@tv", "web1"]
        );
        assert!(split_group_members("  ,  ").is_empty());
    }

    #[test]
    fn definition_form_keys_edit_fields_and_skip_locked_names() {
        let context = Context::new(
            impresario_core::home::Home::at("demo-home"),
            PathBuf::from("demo"),
            PathBuf::from("demo/impresario.config"),
            PathBuf::from("demo/impresario.devices.config"),
            false,
        );
        let mut app = App::empty(context);
        app.form = Some(DefinitionEditor::Device(DeviceEditor {
            kind: FormKind::Add,
            name: String::new(),
            device_type: String::new(),
            ip: String::new(),
            port: String::new(),
            field: 0,
            source: DefinitionSource::Workspace,
        }));
        app.handle_form_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        app.handle_form_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.form.as_ref().map(DefinitionEditor::kind),
            Some(FormKind::Add)
        );
        let Some(DefinitionEditor::Device(editor)) = &app.form else {
            panic!("device form");
        };
        assert_eq!(editor.name, "p");
        assert_eq!(editor.field, 1);

        app.form = Some(DefinitionEditor::Device(DeviceEditor {
            kind: FormKind::Edit,
            name: "phone".to_string(),
            device_type: "android".to_string(),
            ip: "10.0.0.5".to_string(),
            port: String::new(),
            field: 1,
            source: DefinitionSource::Workspace,
        }));
        app.handle_form_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        let Some(DefinitionEditor::Device(editor)) = &app.form else {
            panic!("device form");
        };
        assert_eq!(editor.field, 3, "edit mode never focuses the locked name");
        app.handle_form_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let Some(DefinitionEditor::Device(editor)) = &app.form else {
            panic!("device form");
        };
        assert_eq!(editor.field, 1);
    }

    #[test]
    fn app_form_toggles_clear_data_with_space() {
        let context = Context::new(
            impresario_core::home::Home::at("demo-home"),
            PathBuf::from("demo"),
            PathBuf::from("demo/impresario.config"),
            PathBuf::from("demo/impresario.devices.config"),
            false,
        );
        let mut app = App::empty(context);
        app.form = Some(DefinitionEditor::App(AppEditor {
            kind: FormKind::Add,
            name: "shop".to_string(),
            description: String::new(),
            platforms: "web=https://shop.example".to_string(),
            packages: String::new(),
            launch_activities: String::new(),
            launch_params: String::new(),
            clear_data: false,
            field: 6,
            source: DefinitionSource::Workspace,
        }));
        app.handle_form_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.handle_form_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let Some(DefinitionEditor::App(editor)) = &app.form else {
            panic!("app form");
        };
        assert!(!editor.clear_data);
        assert_eq!(editor.platforms, "web=https://shop.example");
    }

    #[test]
    fn device_form_command_carries_scope_and_port() {
        let editor = DeviceEditor {
            kind: FormKind::Add,
            name: "phone".to_string(),
            device_type: "android".to_string(),
            ip: "10.0.0.5".to_string(),
            port: "38103".to_string(),
            field: 0,
            source: DefinitionSource::Workspace,
        };
        assert_eq!(
            device_form_command(&editor),
            vec![
                "device",
                "add",
                "--type",
                "android",
                "--ip",
                "10.0.0.5",
                "--name",
                "phone",
                "--port",
                "38103",
                "--workspace"
            ]
        );
        let edit = DeviceEditor {
            kind: FormKind::Edit,
            source: DefinitionSource::Global,
            ..editor.clone()
        };
        assert_eq!(
            device_form_command(&edit),
            vec![
                "device", "edit", "--type", "android", "--ip", "10.0.0.5", "--name", "phone",
                "--port", "38103"
            ]
        );
        let remove = DeviceEditor {
            kind: FormKind::Remove,
            port: String::new(),
            ..edit
        };
        assert_eq!(
            device_form_command(&remove),
            vec!["device", "remove", "phone"]
        );
    }

    #[test]
    fn app_form_command_round_trips_every_option() {
        let editor = AppEditor {
            kind: FormKind::Add,
            name: "shop".to_string(),
            description: "Storefront".to_string(),
            platforms: "web=https://shop.example, android=com.acme.shop".to_string(),
            packages: "android=dist/app.apk".to_string(),
            launch_activities: "android=com.acme.MainActivity".to_string(),
            launch_params: "env=staging, locale=en-US".to_string(),
            clear_data: true,
            field: 0,
            source: DefinitionSource::Workspace,
        };
        assert_eq!(
            app_form_command(&editor),
            vec![
                "app",
                "add",
                "--name",
                "shop",
                "--description",
                "Storefront",
                "--platform",
                "web=https://shop.example",
                "--platform",
                "android=com.acme.shop",
                "--package",
                "android=dist/app.apk",
                "--launch-activity",
                "android=com.acme.MainActivity",
                "--launch-param",
                "env=staging",
                "--launch-param",
                "locale=en-US",
                "--clear-data",
                "--workspace",
            ]
        );
    }

    #[test]
    fn app_editor_prefills_platforms_and_options() {
        use impresario_core::apps::{AppConfig, AppOptions, AppPlatform};
        let app = AppConfig {
            name: "shop".to_string(),
            description: Some("Storefront".to_string()),
            platforms: BTreeMap::from([
                (
                    Platform::Web,
                    AppPlatform {
                        url: Some("https://shop.example".to_string()),
                        ..AppPlatform::default()
                    },
                ),
                (
                    Platform::Android,
                    AppPlatform {
                        id: Some("com.acme.shop".to_string()),
                        package: Some("dist/app.apk".to_string()),
                        launch_activity: Some("com.acme.MainActivity".to_string()),
                        ..AppPlatform::default()
                    },
                ),
            ]),
            options: AppOptions {
                clear_data: true,
                launch_params: BTreeMap::from([("env".to_string(), "staging".to_string())]),
            },
        };
        let editor = app_editor_from(
            FormKind::Edit,
            &AppEntry {
                source: DefinitionSource::Global,
                app,
            },
        );
        assert_eq!(editor.field, 1);
        assert_eq!(editor.source, DefinitionSource::Global);
        assert_eq!(
            editor.platforms,
            "web=https://shop.example, android=com.acme.shop"
        );
        assert_eq!(editor.packages, "android=dist/app.apk");
        assert_eq!(editor.launch_activities, "android=com.acme.MainActivity");
        assert_eq!(editor.launch_params, "env=staging");
        assert!(editor.clear_data);
    }

    #[test]
    fn config_form_command_targets_the_right_layer() {
        let workspace = ConfigEditor {
            kind: FormKind::Edit,
            key: "timeout_default_ms".to_string(),
            value: "5000".to_string(),
            field: 0,
            source: DefinitionSource::Workspace,
        };
        assert_eq!(
            config_form_command(&workspace),
            vec!["config", "set", "timeout_default_ms", "5000", "--workspace"]
        );
        let global_only = ConfigEditor {
            kind: FormKind::Add,
            key: "workspace".to_string(),
            value: "/work".to_string(),
            field: 0,
            source: DefinitionSource::Workspace,
        };
        assert_eq!(
            config_form_command(&global_only),
            vec!["config", "set", "workspace", "/work", "--global"]
        );
        let unset = ConfigEditor {
            kind: FormKind::Remove,
            key: "ui.theme".to_string(),
            value: String::new(),
            field: 0,
            source: DefinitionSource::Global,
        };
        assert_eq!(
            config_form_command(&unset),
            vec!["config", "unset", "ui.theme", "--global"]
        );
    }

    #[test]
    fn split_form_list_keeps_spaces_inside_values() {
        assert_eq!(
            split_form_list("env=staging, path=/tmp/my app\n locale=en-US"),
            vec!["env=staging", "path=/tmp/my app", "locale=en-US"]
        );
        assert!(split_form_list(" , \n").is_empty());
    }

    #[test]
    fn device_entries_merge_layers_and_track_the_winner() {
        let dir = tempfile::tempdir().expect("temp workspace");
        let home = impresario_core::home::Home::at(dir.path().join("home"));
        std::fs::create_dir_all(home.root()).expect("home");
        std::fs::write(
            home.devices_path(),
            "devices:\n  - name: phone\n    type: android\n    ip: 10.0.0.5\n    port: 38103\n  - name: web1\n    type: web\n    ip: 127.0.0.1\n",
        )
        .expect("global devices");
        let workspace_path = dir.path().join("impresario.devices.config");
        std::fs::write(
            &workspace_path,
            "devices:\n  - name: phone\n    type: android\n    ip: 10.0.0.6\n",
        )
        .expect("workspace devices");
        let entries =
            crate::definitions::device_entries(&home, &workspace_path).expect("device entries");
        assert_eq!(entries.len(), 2);
        let phone = entries
            .iter()
            .find(|entry| entry.device.name == "phone")
            .expect("phone");
        assert_eq!(phone.source, DefinitionSource::Workspace);
        assert_eq!(phone.device.ip, "10.0.0.6");
        assert_eq!(phone.device.connection.port, None);
        let web = entries
            .iter()
            .find(|entry| entry.device.name == "web1")
            .expect("web1");
        assert_eq!(web.source, DefinitionSource::Global);
    }

    #[test]
    fn load_groups_reads_both_layers_and_resolves_nesting() {
        let dir = tempfile::tempdir().expect("temp workspace");
        let home = impresario_core::home::Home::at(dir.path().join("home"));
        std::fs::create_dir_all(home.root()).expect("home");
        std::fs::write(
            home.devices_path(),
            "devices:\n  - name: phone\n    type: android\n    ip: 10.0.0.5\ngroups:\n  phones:\n    - phone\n",
        )
        .expect("global devices");
        std::fs::write(
            dir.path().join("impresario.devices.config"),
            "devices:\n  - name: web1\n    type: web\n    ip: 127.0.0.1\ngroups:\n  screens:\n    - '@phones'\n    - web1\n",
        )
        .expect("workspace devices");
        let context = Context::new(
            home,
            dir.path().to_path_buf(),
            dir.path().join("impresario.config"),
            dir.path().join("impresario.devices.config"),
            false,
        );
        let mut app = App::empty(context);
        app.load_groups();
        assert_eq!(app.groups.len(), 2, "{:?}", app.group_error);
        let phones = app
            .groups
            .iter()
            .find(|group| group.name == "phones")
            .expect("phones");
        assert_eq!(phones.source, DefinitionSource::Global);
        assert_eq!(
            phones.devices,
            vec![ResolvedDevice {
                name: "phone".to_string(),
                platform: "android".to_string(),
            }]
        );
        let screens = app
            .groups
            .iter()
            .find(|group| group.name == "screens")
            .expect("screens");
        assert_eq!(screens.source, DefinitionSource::Workspace);
        assert_eq!(screens.devices.len(), 2, "{:?}", screens.error);
        assert!(screens.error.is_none());
        assert_eq!(screens.members, vec!["@phones", "web1"]);
    }

    #[test]
    fn group_selection_clamps_and_follows_the_last_row() {
        let context = Context::new(
            impresario_core::home::Home::at("demo-home"),
            PathBuf::from("demo"),
            PathBuf::from("demo/impresario.config"),
            PathBuf::from("demo/impresario.devices.config"),
            false,
        );
        let mut app = App::empty(context);
        app.groups = vec![
            GroupInfo {
                name: "a".to_string(),
                members: Vec::new(),
                devices: Vec::new(),
                source: DefinitionSource::Global,
                error: None,
            },
            GroupInfo {
                name: "b".to_string(),
                members: Vec::new(),
                devices: Vec::new(),
                source: DefinitionSource::Workspace,
                error: None,
            },
        ];
        app.screen = Screen::Groups;
        app.select_last_group();
        assert_eq!(app.group_index, 1);
        app.scroll(1);
        assert_eq!(app.group_index, 1, "selection must not pass the last row");
        app.select_first_group();
        app.scroll(-1);
        assert_eq!(app.group_index, 0, "selection must not go below zero");
        app.scroll(1);
        assert_eq!(app.group_index, 1, "selection must follow a down scroll");
    }

    #[test]
    fn rows_from_tree_keeps_file_and_kind() {
        let value = serde_json::json!({
            "scenes": [],
            "theatres": [],
            "plays": [{
                "name": "Login",
                "file": "plays/login.play",
                "line": 1,
                "acts": [{"name": "Pass", "line": 4, "tags": ["smoke"]}]
            }]
        });
        let rows = rows_from_tree(&value);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.get(1).map(|row| row.kind), Some(TestKind::Act));
        assert_eq!(
            rows.get(1)
                .and_then(|row| row.file.as_ref())
                .map(|p| p.display().to_string()),
            Some("plays/login.play".to_string())
        );
    }

    #[test]
    fn coverage_groups_stage_implementations_and_marks_missing_devices() {
        let context = Context::new(
            impresario_core::home::Home::at("demo-home"),
            PathBuf::from("demo"),
            PathBuf::from("demo/impresario.config"),
            PathBuf::from("demo/impresario.devices.config"),
            false,
        );
        let mut app = App::empty(context);
        app.devices = vec![
            DeviceRow {
                name: "web1".to_string(),
                platform: "web".to_string(),
                connection: "launch".to_string(),
                device_type: "web".to_string(),
                ip: String::new(),
                port: None,
                source: DefinitionSource::Global,
            },
            DeviceRow {
                name: "phone".to_string(),
                platform: "android".to_string(),
                connection: "adb".to_string(),
                device_type: "android".to_string(),
                ip: String::new(),
                port: None,
                source: DefinitionSource::Global,
            },
            DeviceRow {
                name: "living".to_string(),
                platform: "webOS".to_string(),
                connection: "cdp".to_string(),
                device_type: "webOS".to_string(),
                ip: String::new(),
                port: None,
                source: DefinitionSource::Global,
            },
        ];
        app.tests = vec![
            TestRow {
                indent: 0,
                label: "theatre Auth".to_string(),
                kind: TestKind::Theatre,
                name: "Auth".to_string(),
                ..TestRow::default()
            },
            TestRow {
                indent: 1,
                label: "stage Login".to_string(),
                kind: TestKind::Stage,
                name: "Login".to_string(),
                platforms: vec![
                    "web".to_string(),
                    "android".to_string(),
                    "webos".to_string(),
                ],
                ..TestRow::default()
            },
            TestRow {
                indent: 0,
                label: "theatre Playback".to_string(),
                kind: TestKind::Theatre,
                name: "Playback".to_string(),
                ..TestRow::default()
            },
            TestRow {
                indent: 1,
                label: "stage PlaybackStarted".to_string(),
                kind: TestKind::Stage,
                name: "PlaybackStarted".to_string(),
                platforms: vec!["web".to_string()],
                ..TestRow::default()
            },
        ];
        app.test_error = None;

        app.rebuild_coverage();

        assert_eq!(app.coverage.len(), 2);
        let login = &app.coverage[0];
        assert_eq!(login.stage, "Login");
        assert_eq!(login.theatres, vec!["Auth".to_string()]);
        assert_eq!(login.implementations, 1);
        assert!(login.missing.is_empty());
        let playback = &app.coverage[1];
        assert_eq!(playback.stage, "PlaybackStarted");
        assert_eq!(
            playback.missing,
            vec!["phone (android)".to_string(), "living (webOS)".to_string()]
        );
    }

    #[test]
    fn step_finished_adds_redacted_probe_evidence_lines() {
        let context = Context::new(
            impresario_core::home::Home::at("demo-home"),
            PathBuf::from("demo"),
            PathBuf::from("demo/impresario.config"),
            PathBuf::from("demo/impresario.devices.config"),
            false,
        );
        let mut app = App::empty(context);
        app.events = vec![ExecEvent::StepFinished {
            device: "web1".to_string(),
            play: "Playback".to_string(),
            act: "Check".to_string(),
            stage: "Playing".to_string(),
            index: 2,
            status: "unverifiable".to_string(),
            duration_ms: 12,
            error: Some("probe unverifiable: no system evidence".to_string()),
            details: BTreeMap::from([
                ("probe".to_string(), "showing=unverifiable".to_string()),
                ("reason".to_string(), "no system evidence".to_string()),
                ("observed.playing".to_string(), "false".to_string()),
            ]),
        }];
        let record = JobRecord {
            schema_version: 1,
            id: "job-1".to_string(),
            status: JobStatus::Running,
            detached: true,
            created_at: "2026-11-09T12:00:00Z".to_string(),
            started_at: Some("2026-11-09T12:00:01Z".to_string()),
            finished_at: None,
            duration_ms: None,
            exit_code: None,
            workspace: "demo".to_string(),
            devices: vec!["web1".to_string()],
            plays: vec!["Playback".to_string()],
            label: "1 play(s) x 1 device(s)".to_string(),
            pid: Some(1),
            queue_position: None,
            run_dir: "demo/reports/job-1".to_string(),
            message: None,
        };
        app.rebuild_tree(&record);

        assert!(app.tree.iter().any(|line| {
            line.status == LineStatus::Unverifiable && line.text.contains("unverifiable")
        }));
        let details: Vec<&str> = app
            .tree
            .iter()
            .filter(|line| line.status == LineStatus::Detail)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            details,
            vec![
                "observed.playing: false",
                "probe: showing=unverifiable",
                "reason: no system evidence",
            ]
        );
        assert!(app
            .logs
            .iter()
            .any(|line| line.contains("no system evidence")));
    }

    #[test]
    fn probe_rows_follow_the_platform_matrix() {
        let web = probe_row(&DeviceRow {
            name: "web1".to_string(),
            platform: "web".to_string(),
            connection: "launch".to_string(),
            device_type: "web".to_string(),
            ip: String::new(),
            port: None,
            source: DefinitionSource::Global,
        });
        assert_eq!(web.error, None);
        assert!(web.showing && web.player && web.drm);
        assert_eq!(web.strongest, "authoritative");
        assert!(web.properties > 0);
        assert!(web.providers.contains(&"Widevine".to_string()));
        assert!(web
            .sources
            .iter()
            .any(|source| source.source == "dom" && source.available));
        let luna = web
            .sources
            .iter()
            .find(|source| source.source == "luna_media")
            .expect("luna_media row");
        assert!(!luna.available);
        assert!(luna
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("webOS")));

        let apple = probe_row(&DeviceRow {
            name: "living".to_string(),
            platform: "tvOS".to_string(),
            connection: "apple".to_string(),
            device_type: "tvOS".to_string(),
            ip: String::new(),
            port: None,
            source: DefinitionSource::Global,
        });
        assert_eq!(apple.providers, vec!["FairPlay".to_string()]);
        assert!(apple
            .sources
            .iter()
            .any(|source| source.source == "dvt_sysmon" && !source.available));

        let unknown = probe_row(&DeviceRow {
            name: "x".to_string(),
            platform: "bogus".to_string(),
            connection: "adb".to_string(),
            device_type: "bogus".to_string(),
            ip: String::new(),
            port: None,
            source: DefinitionSource::Global,
        });
        assert!(unknown.error.is_some());
        assert!(unknown.sources.is_empty());
    }
}
