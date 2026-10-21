//! Interactive application state, input handling and child command dispatch.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use impresario_core::apps;
use impresario_core::config;
use impresario_core::settings::{self, SettingValue};
use impresario_core::workspace;

use crate::cli;
use crate::exec::ExecEvent;
use crate::jobs::{self, JobRecord, JobStatus, JobStore};
use crate::report::Status;

use super::{Action, Context, Focus, Screen};

const REFRESH: Duration = Duration::from_millis(250);
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
pub(super) struct DeviceRow {
    pub name: String,
    pub platform: String,
    pub connection: String,
}

#[derive(Debug, Clone)]
pub(super) struct AppRow {
    pub name: String,
    pub description: String,
    pub platforms: String,
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
    pub apps: Vec<AppRow>,
    pub config_rows: Vec<ConfigRow>,
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
    store: JobStore,
    last_refresh: Instant,
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
        let mut app = Self {
            screen: Screen::Home,
            focus: Focus::Prompt,
            prompt: Prompt::default(),
            history: History::load(&history_path, history_limit),
            search: None,
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
            apps: Vec::new(),
            config_rows: Vec::new(),
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
            store,
            last_refresh: Instant::now(),
            history_path,
            history_limit,
            state_path,
            ctx,
        };
        if first_run {
            app.overlay = Some(first_run_text(&app.ctx));
        }
        app
    }

    pub(super) fn new(ctx: Context) -> Self {
        let mut app = Self::empty(ctx);
        app.load_devices();
        app.load_apps();
        app.load_config();
        app.load_tests();
        app.refresh_completion_context();
        app.load_state();
        app.push_activity("Welcome. Type a command, or /help for keys and examples.".to_string());
        app
    }

    fn refresh_completion_context(&mut self) {
        let mut values = BTreeSet::new();
        for device in &self.devices {
            values.insert(format!("/run --device {}", device.name));
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
            "runs" => Screen::Runs,
            "tests" => Screen::Tests,
            "devices" => Screen::Devices,
            "apps" => Screen::Apps,
            "config" => Screen::Config,
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
                    ..
                } => {
                    self.tree.push(TreeLine {
                        indent: 3,
                        text: format!("{stage} [{index}]  {status}  {duration_ms}ms"),
                        status: status_of_str(status),
                    });
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
        let outcome = config::load_devices(&self.ctx.devices_path);
        if let Some(devices) = outcome.config {
            for device in devices.devices {
                let connection = device.effective_transport().as_str().to_string();
                let platform = device.platform.as_str().to_string();
                self.devices.push(DeviceRow {
                    name: device.name,
                    platform,
                    connection,
                });
            }
        }
        if outcome.diagnostics.has_errors() {
            self.push_activity(format!(
                "devices: {} error(s) in {}",
                outcome.diagnostics.errors().count(),
                self.ctx.devices_path.display()
            ));
        }
    }

    pub(super) fn load_apps(&mut self) {
        self.apps.clear();
        let outcome = apps::load_apps_merged(&self.ctx.base);
        if let Some(apps) = outcome.config {
            for app in apps.apps {
                self.apps.push(AppRow {
                    name: app.name,
                    description: app.description.unwrap_or_default(),
                    platforms: app
                        .platforms
                        .keys()
                        .map(|platform| platform.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                });
            }
        }
        if outcome.diagnostics.has_errors() {
            self.push_activity(format!(
                "apps: {} error(s)",
                outcome.diagnostics.errors().count()
            ));
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
            Screen::Devices => self.load_devices(),
            Screen::Apps => self.load_apps(),
            Screen::Config => self.load_config(),
            Screen::Home | Screen::Runs => {}
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
            self.push_activity(format!(
                "Job {id} continues in the background; reattach with /attach {id}."
            ));
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
                "Device filter: {name} (Tab cycles, Shift-Tab clears)."
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
                if !stdout.is_empty() {
                    self.push_activity(stdout);
                }
                if !stderr.is_empty() {
                    self.push_activity(stderr);
                }
                if !output.status.success() {
                    return;
                }
                if dry_run {
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
                self.push_activity(text);
            }
            Err(message) => self.push_activity(message),
        }
    }

    pub(super) fn open_selected_test(&mut self) -> Action {
        self.selected_test()
            .and_then(|row| row.file.clone())
            .map_or(Action::None, Action::OpenEditor)
    }

    pub(super) fn open_config(&mut self) -> Action {
        Action::OpenEditor(self.ctx.config_path.clone())
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
            "home" => {
                self.screen = Screen::Home;
                self.focus = Focus::Prompt;
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
                    "Unknown slash command '/{other}'. Press F1 for help."
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
            }
            Err(message) => self.push_activity(message),
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
        self.focus = Focus::Prompt;
        self.reload_screen();
    }

    pub(super) fn scroll(&mut self, delta: i32) {
        match self.screen {
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
            Screen::Home | Screen::Devices | Screen::Apps | Screen::Config => {}
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
        .arg("--no-color");
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

fn first_run_text(ctx: &Context) -> String {
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
        "Press F1 at any time for keys and slash commands.".to_string(),
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
    } else if status.eq_ignore_ascii_case("failed") || status.eq_ignore_ascii_case("error") {
        LineStatus::Failed
    } else {
        LineStatus::Running
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
        "/devices",
        "/apps",
        "/config",
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
        "Prompt: type a CLI command; /run --tag smoke is a shortcut. Tab completes.",
        "        Ctrl-R reverse search, Up/Down history, Alt+Enter newline.",
        "        Ctrl-C clears the prompt, twice quits. Ctrl-Q quits. Ctrl-Z suspends.",
        "",
        "Screens: F2 home, F3 runs, F4 tests, F5 devices, F6 apps, F7 config.",
        "Focus:  Tab switches to content in most screens; Esc returns to the prompt.",
        "",
        "Runs:   Enter follow, b background/detach, x stop, f pause/resume log tail,",
        "        Tab cycle device, / filter, ? help, r rerun, F rerun failed,",
        "        o report, p screenshot, X or Delete remove a finished job.",
        "Tests:  Enter edit, Space mark, r run selected/marked, d dry-run matrix.",
        "        Selectors live in the prompt: /tests --tag smoke --platform android.",
        "Devices/Apps/Config: j/k scroll, Enter open the relevant file.",
        "",
        "Slash:  /run /jobs /devices /apps /config /tests /open /doctor /fmt",
        "        /validate /plan /attach /logs /stop /report /profile /help /quit",
        "",
        "Everything is scriptable outside the TUI: impresario run --detach,",
        "impresario jobs --json, impresario wait <id>, impresario report <id>.",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
