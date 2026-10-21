//! Scripted PTY acceptance for the TUI (TASK-2 3.6).
//!
//! Each test drives the real `impresario` binary through ConPTY exactly like a
//! terminal: cursor queries are answered, keys are typed and the visible
//! screen is asserted after a tiny ANSI parser resolves cursor movement and
//! erases. No test depends on raw escape-sequence bytes from ratatui.

mod common;

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{write_workspace, TestAgent, PLAY_BASIC};
use impresario_clients::testing::MockAgentBuilder;
use impresario_core::home::Home;
use impresario_master::jobs::JobStore;
use portable_pty::{native_pty_system, Child, CommandBuilder, ExitStatus, MasterPty, PtySize};

const ROWS: u16 = 40;
const COLS: u16 = 110;
const READY: Duration = Duration::from_secs(20);
const EXIT: Duration = Duration::from_secs(15);

#[derive(Debug)]
struct Screen {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<char>>,
    row: usize,
    col: usize,
    state: State,
    csi: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

impl Screen {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![vec![' '; cols]; rows],
            row: 0,
            col: 0,
            state: State::Text,
            csi: String::new(),
        }
    }

    fn feed(&mut self, text: &str) {
        for character in text.chars() {
            self.step(character);
        }
    }

    fn step(&mut self, character: char) {
        match self.state {
            State::Text => match character {
                '\x1b' => self.state = State::Escape,
                '\r' => self.col = 0,
                '\n' => self.newline(),
                '\x08' => self.col = self.col.saturating_sub(1),
                '\t' => self.col = ((self.col / 8) + 1) * 8,
                character if !character.is_control() => self.put(character),
                _ => {}
            },
            State::Escape => match character {
                '[' => {
                    self.csi.clear();
                    self.state = State::Csi;
                }
                ']' => self.state = State::Osc,
                '(' | ')' => self.state = State::Escape,
                '7' => {}
                '8' => {}
                _ => self.state = State::Text,
            },
            State::Csi => {
                if ('@'..='~').contains(&character) {
                    self.csi_command(character);
                    self.state = State::Text;
                } else {
                    self.csi.push(character);
                }
            }
            State::Osc => match character {
                '\x07' => self.state = State::Text,
                '\x1b' => self.state = State::OscEscape,
                _ => {}
            },
            State::OscEscape => {
                self.state = if character == '\\' {
                    State::Text
                } else {
                    State::Osc
                };
            }
        }
    }

    fn csi_command(&mut self, final_byte: char) {
        let params = self.csi.trim_start_matches(['?', '>', '!']);
        let numbers: Vec<usize> = params
            .split(';')
            .map(|part| {
                part.split(':')
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0)
            })
            .collect();
        let first = numbers.first().copied().unwrap_or(0);
        let count = first.max(1);
        match final_byte {
            'H' | 'f' => {
                let row = numbers.first().copied().unwrap_or(1).max(1) - 1;
                let col = numbers.get(1).copied().unwrap_or(1).max(1) - 1;
                self.row = row.min(self.rows.saturating_sub(1));
                self.col = col.min(self.cols.saturating_sub(1));
            }
            'A' => self.row = self.row.saturating_sub(count),
            'B' => self.row = (self.row + count).min(self.rows.saturating_sub(1)),
            'C' => self.col = (self.col + count).min(self.cols.saturating_sub(1)),
            'D' => self.col = self.col.saturating_sub(count),
            'E' => {
                self.row = (self.row + count).min(self.rows.saturating_sub(1));
                self.col = 0;
            }
            'F' => {
                self.row = self.row.saturating_sub(count);
                self.col = 0;
            }
            'G' => self.col = first.saturating_sub(1).min(self.cols.saturating_sub(1)),
            'd' => self.row = first.saturating_sub(1).min(self.rows.saturating_sub(1)),
            'J' => match first {
                0 => {
                    self.clear_to_end();
                }
                1 => {
                    for row in 0..self.row {
                        self.clear_row(row);
                    }
                    self.clear_line_from(0);
                }
                _ => {
                    for row in 0..self.rows {
                        self.clear_row(row);
                    }
                }
            },
            'K' => match first {
                0 => self.clear_line_from(self.col),
                1 => self.clear_line_to(self.col),
                _ => self.clear_row(self.row),
            },
            'X' => {
                for offset in 0..count {
                    self.set(self.row, self.col + offset, ' ');
                }
            }
            _ => {}
        }
    }

    fn put(&mut self, character: char) {
        if self.col >= self.cols {
            self.newline();
            self.col = 0;
        }
        self.set(self.row, self.col, character);
        self.col += 1;
    }

    fn set(&mut self, row: usize, col: usize, character: char) {
        if let Some(line) = self.cells.get_mut(row) {
            if let Some(cell) = line.get_mut(col) {
                *cell = character;
            }
        }
    }

    fn newline(&mut self) {
        self.row += 1;
        if self.row >= self.rows {
            self.cells.remove(0);
            self.cells.push(vec![' '; self.cols]);
            self.row = self.rows.saturating_sub(1);
        }
    }

    fn clear_row(&mut self, row: usize) {
        if let Some(line) = self.cells.get_mut(row) {
            line.fill(' ');
        }
    }

    fn clear_line_from(&mut self, col: usize) {
        if let Some(line) = self.cells.get_mut(self.row) {
            for cell in line.iter_mut().skip(col) {
                *cell = ' ';
            }
        }
    }

    fn clear_line_to(&mut self, col: usize) {
        if let Some(line) = self.cells.get_mut(self.row) {
            for cell in line.iter_mut().take(col.saturating_add(1)) {
                *cell = ' ';
            }
        }
    }

    fn clear_to_end(&mut self) {
        self.clear_line_from(self.col);
        for row in self.row.saturating_add(1)..self.rows {
            self.clear_row(row);
        }
    }

    fn text(&self) -> String {
        self.cells
            .iter()
            .map(|line| line.iter().collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct PtySession {
    /// The master must stay alive: dropping it closes the ConPTY output pipe.
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    screen: Arc<Mutex<Screen>>,
    raw: Arc<Mutex<Vec<u8>>>,
    status: Option<ExitStatus>,
}

impl PtySession {
    fn start(home: &Path, workspace: &Path) -> Self {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");
        let mut command = CommandBuilder::new(assert_cmd::cargo::cargo_bin("impresario"));
        command.arg("--workspace");
        command.arg(workspace);
        command.env("IMPRESARIO_HOME", home);
        command.env("TERM", "xterm-256color");
        command.env("IMPRESARIO_ASCII", "1");
        command.env_remove("CI");
        command.cwd(workspace);
        let child = pair.slave.spawn_command(command).expect("spawn tui");
        drop(pair.slave);
        let master = pair.master;
        let mut reader = master.try_clone_reader().expect("pty reader");
        let writer = Arc::new(Mutex::new(master.take_writer().expect("pty writer")));
        let screen = Arc::new(Mutex::new(Screen::new(ROWS as usize, COLS as usize)));
        let raw = Arc::new(Mutex::new(Vec::new()));
        let read_screen = Arc::clone(&screen);
        let read_raw = Arc::clone(&raw);
        let read_writer = Arc::clone(&writer);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            let mut tail = String::new();
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                if let Ok(mut raw) = read_raw.lock() {
                    raw.extend_from_slice(&buffer[..count]);
                }
                let text = String::from_utf8_lossy(&buffer[..count]);
                {
                    let mut screen = read_screen.lock().expect("screen lock");
                    screen.feed(&text);
                }
                tail.push_str(&text);
                if tail.contains("\x1b[6n") {
                    // ConPTY asks the terminal for the cursor position and
                    // withholds output until it is answered.
                    if let Ok(mut writer) = read_writer.lock() {
                        let _ = writer.write_all(b"\x1b[1;1R");
                        let _ = writer.flush();
                    }
                    tail.clear();
                } else if tail.chars().count() > 64 {
                    let keep: Vec<char> = tail.chars().rev().take(8).collect();
                    tail = keep.into_iter().rev().collect();
                }
            }
        });
        Self {
            _master: master,
            child,
            writer,
            screen,
            raw,
            status: None,
        }
    }

    fn send(&self, bytes: &[u8]) {
        let mut writer = self.writer.lock().expect("writer lock");
        writer.write_all(bytes).expect("write to pty");
        writer.flush().expect("flush pty");
    }

    fn text(&self) -> String {
        self.screen.lock().expect("screen lock").text()
    }

    fn raw_tail(&self, bytes: usize) -> String {
        let raw = self.raw.lock().expect("raw lock");
        let start = raw.len().saturating_sub(bytes);
        String::from_utf8_lossy(&raw[start..])
            .chars()
            .flat_map(char::escape_debug)
            .collect()
    }

    fn wait_for(&mut self, needles: &[&str], timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let text = self.text();
            if needles.iter().all(|needle| text.contains(needle)) {
                return;
            }
            if Instant::now() >= deadline {
                let status = self.try_wait();
                panic!(
                    "timed out waiting for {needles:?}; child={status:?}; screen was:\n{text}\n--- raw tail ---\n{}",
                    self.raw_tail(4000)
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_gone(&self, needle: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !self.text().contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("'{needle}' stayed on screen:\n{}", self.text());
    }

    fn wait_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the TUI did not exit:\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn try_wait(&mut self) -> Option<ExitStatus> {
        if let Some(status) = &self.status {
            return Some(status.clone());
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.status = Some(status.clone());
                Some(status)
            }
            _ => None,
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.try_wait();
        }
    }
}

fn job_id(home: &Path) -> String {
    let store = JobStore::new(&Home::at(home));
    let deadline = Instant::now() + READY;
    loop {
        if let Some(record) = store.list().into_iter().next() {
            return record.id;
        }
        assert!(Instant::now() < deadline, "no job was submitted");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn cli(home: &Path, workspace: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(assert_cmd::cargo::cargo_bin("impresario"))
        .env("IMPRESARIO_HOME", home)
        .arg("--workspace")
        .arg(workspace)
        .arg("--no-tui")
        .arg("--plain")
        .arg("--no-color")
        .args(args)
        .output()
        .expect("cli runs")
}

#[test]
fn first_run_creates_the_workspace_skeleton() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut session = PtySession::start(home.path(), workspace.path());

    session.wait_for(&["Welcome to impresario"], READY);
    session.wait_for(&["[y] create impresario.config"], READY);
    session.send(b"y");

    let deadline = Instant::now() + READY;
    while !workspace.path().join("impresario.config").is_file() {
        assert!(
            Instant::now() < deadline,
            "the first-run scaffold was not created:\n{}",
            session.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(workspace.path().join("plays").is_dir());
    assert!(workspace.path().join("scenes").is_dir());
    assert!(
        std::fs::read_dir(workspace.path().join("plays"))
            .expect("plays")
            .next()
            .is_some(),
        "the example play was created"
    );
    session.wait_gone("Create the workspace skeleton now?", READY);

    session.send(b"\x11");
    let status = session.wait_exit(EXIT);
    assert!(status.success(), "tui exit {status:?}\n{}", session.text());
}

#[test]
fn slash_completion_and_ctrl_r_history() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(workspace.path(), "", &[], &[("basic.play", PLAY_BASIC)]);
    let mut session = PtySession::start(home.path(), workspace.path());

    session.wait_for(&["Dashboard"], READY);
    session.send(b"/he");
    session.wait_for(&["/help", "Tab accepts"], READY);
    session.send(b"\t");
    session.wait_for(&["\u{276f} /help"], READY);
    session.send(b"\r");
    session.wait_for(&["help \u{b7} Esc closes"], READY);
    session.send(b"\x1b");
    session.wait_gone("help \u{b7} Esc closes", READY);

    session.send(b"\x12");
    session.wait_for(&["(reverse-i-search)`"], READY);
    session.wait_for(&["/help"], READY);
    session.send(b"\r");
    session.wait_for(&["\u{276f} /help"], READY);

    session.send(b"\x11");
    let status = session.wait_exit(EXIT);
    assert!(status.success(), "tui exit {status:?}\n{}", session.text());
}

#[test]
fn run_view_detach_close_reattach_and_report() {
    let agent = TestAgent::start(MockAgentBuilder::new().delay_kind("run_action", 3_000));
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    write_workspace(
        workspace.path(),
        "",
        &[("web1".to_string(), agent.address.clone())],
        &[("basic.play", PLAY_BASIC)],
    );
    // The default 2 s step timeout would cut the deliberate delay short;
    // raise it so the job is comfortably running while the TUI detaches.
    let config_path = workspace.path().join("impresario.config");
    let config = std::fs::read_to_string(&config_path)
        .expect("read config")
        .replace("timeout_default_ms: 2000", "timeout_default_ms: 10000");
    std::fs::write(&config_path, config).expect("write config");
    let mut session = PtySession::start(home.path(), workspace.path());

    session.wait_for(&["Dashboard"], READY);
    session.send(b"/run\r");
    session.wait_for(&["Jobs (1)", "running"], READY);
    let id = job_id(home.path());
    assert!(
        session.text().contains(&id),
        "the run view shows the job id"
    );

    // Focus the list, then send the run to the background.
    session.send(b"\t");
    session.send(b"b");
    session.wait_for(&["continues in the background"], READY);
    session.wait_for(&["/attach"], READY);
    session.send(b"\x11");
    let status = session.wait_exit(EXIT);
    assert!(status.success(), "tui exit {status:?}\n{}", session.text());

    let record = JobStore::new(&Home::at(home.path()))
        .read(&id)
        .expect("job record")
        .expect("job exists");
    assert!(
        !record.status.terminal() || record.status == impresario_master::jobs::JobStatus::Passed,
        "closing the TUI did not interrupt the job: {:?}",
        record.status
    );

    let attached = cli(home.path(), workspace.path(), &["attach", &id]);
    assert!(
        attached.status.success(),
        "attach failed: {}",
        String::from_utf8_lossy(&attached.stderr)
    );
    let stdout = String::from_utf8_lossy(&attached.stdout);
    assert!(stdout.contains("Act: Pass"), "{stdout}");
    let stderr = String::from_utf8_lossy(&attached.stderr);
    assert!(stderr.contains("passed"), "{stderr}");

    let report = cli(home.path(), workspace.path(), &["report", &id, "--json"]);
    assert!(
        report.status.success(),
        "report failed: {}",
        String::from_utf8_lossy(&report.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).expect("report json");
    assert_eq!(report["run_id"], id);
    assert_eq!(report["status"], "passed");
}
