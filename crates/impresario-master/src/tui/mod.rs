//! The default interactive TUI (TASK-2 3.1/3.2/3.3).
//!
//! The TUI is intentionally a client of the same CLI: every command it runs is
//! a real `impresario` subprocess, and runs are always detached into the job
//! supervisor. That keeps one execution path, one set of exit codes and one
//! place where secrets are redacted.

mod app;
mod render;

#[cfg(test)]
mod snapshots;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use impresario_core::home::Home;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::cli::{EXIT_INFRA, EXIT_OK};
use app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Screen {
    #[default]
    Home,
    Runs,
    Tests,
    Devices,
    Apps,
    Config,
}

impl Screen {
    pub(super) fn label(self) -> &'static str {
        match self {
            Screen::Home => "home",
            Screen::Runs => "runs",
            Screen::Tests => "tests",
            Screen::Devices => "devices",
            Screen::Apps => "apps",
            Screen::Config => "config",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Focus {
    #[default]
    Prompt,
    Content,
}

#[derive(Debug)]
pub(super) enum Action {
    None,
    Quit,
    OpenEditor(PathBuf),
    OpenExternally(PathBuf),
    Suspend,
}

/// Everything the TUI needs to resolve paths and start child commands.
#[derive(Debug, Clone)]
pub struct Context {
    pub home: Home,
    pub base: PathBuf,
    pub config_path: PathBuf,
    pub devices_path: PathBuf,
    pub color: bool,
    pub ascii: bool,
}

impl Context {
    #[must_use]
    pub fn new(
        home: Home,
        base: PathBuf,
        config_path: PathBuf,
        devices_path: PathBuf,
        color: bool,
    ) -> Self {
        let ascii = std::env::var("TERM").is_ok_and(|term| term.eq_ignore_ascii_case("dumb"))
            || std::env::var_os("IMPRESARIO_ASCII").is_some();
        Self {
            home,
            base,
            config_path,
            devices_path,
            color,
            ascii,
        }
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    entered: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("cannot enable raw mode: {error}"))?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            crossterm::cursor::Hide
        ) {
            let _ = disable_raw_mode();
            return Err(format!("cannot enter the alternate screen: {error}"));
        }
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                restore_terminal();
                return Err(format!("cannot create the terminal: {error}"));
            }
        };
        if let Err(error) = terminal.clear() {
            restore_terminal();
            return Err(format!("cannot clear the terminal: {error}"));
        }
        Ok(Self {
            terminal,
            entered: true,
        })
    }

    fn leave(&mut self) {
        let _ = self.terminal.show_cursor();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = disable_raw_mode();
        self.entered = false;
    }

    fn reenter(&mut self) -> Result<(), String> {
        enable_raw_mode().map_err(|error| format!("cannot enable raw mode: {error}"))?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            crossterm::cursor::Hide
        )
        .map_err(|error| format!("cannot re-enter the alternate screen: {error}"))?;
        self.entered = true;
        let _ = self.terminal.clear();
        Ok(())
    }

    fn restore(&mut self) {
        if self.entered {
            self.leave();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

fn restore_terminal() {
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    use std::sync::Mutex;
    type Hook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;
    static PREVIOUS: Mutex<Option<Hook>> = Mutex::new(None);
    let Ok(mut slot) = PREVIOUS.lock() else {
        return;
    };
    if slot.is_some() {
        return;
    }
    *slot = Some(std::panic::take_hook());
    drop(slot);
    std::panic::set_hook(Box::new(|info| {
        restore_terminal();
        if let Ok(mut slot) = PREVIOUS.lock() {
            if let Some(previous) = slot.take() {
                previous(info);
                return;
            }
        }
        eprintln!("impresario: {info}");
    }));
}

fn uninstall_panic_hook() {
    use std::sync::Mutex;
    type Hook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;
    static PREVIOUS: Mutex<Option<Hook>> = Mutex::new(None);
    let _ = std::panic::take_hook();
    if let Ok(mut slot) = PREVIOUS.lock() {
        if let Some(previous) = slot.take() {
            std::panic::set_hook(previous);
        }
    }
}

/// Run the interactive TUI until the user quits. Returns the process exit code.
pub fn run(ctx: Context) -> i32 {
    install_panic_hook();
    let mut guard = match TerminalGuard::enter() {
        Ok(guard) => guard,
        Err(error) => {
            uninstall_panic_hook();
            eprintln!("impresario: {error}");
            return EXIT_INFRA;
        }
    };
    let mut app = App::new(ctx);
    let code = loop {
        app.tick();
        if let Err(error) = guard.terminal.draw(|frame| render::draw(frame, &app)) {
            eprintln!("impresario: cannot draw the TUI: {error}");
            break EXIT_INFRA;
        }
        let action = match event::poll(Duration::from_millis(200)) {
            Ok(true) => match event::read() {
                Ok(event) => app.handle_event(event),
                Err(error) => {
                    app.push_activity(format!("input: {error}"));
                    Action::None
                }
            },
            Ok(false) => Action::None,
            Err(error) => {
                app.push_activity(format!("input: {error}"));
                Action::None
            }
        };
        match action {
            Action::None => {}
            Action::Quit => break EXIT_OK,
            Action::OpenEditor(path) => {
                guard.leave();
                let status = open_editor(&path);
                if let Err(error) = guard.reenter() {
                    eprintln!("impresario: {error}");
                    break EXIT_INFRA;
                }
                if let Err(message) = status {
                    app.push_activity(message);
                }
            }
            Action::OpenExternally(path) => {
                if let Err(message) = open_external(&path) {
                    app.push_activity(message);
                }
            }
            Action::Suspend => {
                guard.leave();
                #[cfg(unix)]
                {
                    // Default SIGTSTP: the shell resumes us, then we restore.
                    // This is a deliberate, documented terminal suspend.
                    // SAFETY: raising a signal has no memory-safety impact.
                    unsafe {
                        libc::raise(libc::SIGTSTP);
                    }
                }
                if let Err(error) = guard.reenter() {
                    eprintln!("impresario: {error}");
                    break EXIT_INFRA;
                }
            }
        }
        if app.should_quit {
            break EXIT_OK;
        }
    };
    let state = app.save_state();
    guard.restore();
    uninstall_panic_hook();
    if let Err(error) = state {
        eprintln!("impresario: state: {error}");
    }
    code
}

impl App {
    fn handle_event(&mut self, event: Event) -> Action {
        match event {
            Event::Resize(_, _) => Action::None,
            Event::Paste(text) => {
                if let Some(search) = self.search.as_mut() {
                    search.query.push_str(&text.replace('\n', " "));
                    search.index = 0;
                    let history = &self.history;
                    search.update(history);
                } else {
                    self.prompt.insert_text(&text);
                }
                Action::None
            }
            Event::Key(key) => self.handle_key(key),
            _ => Action::None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Action::None;
        }
        let modifiers = key.modifiers;
        if modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            return Action::Quit;
        }
        if key.code == KeyCode::F(1) {
            self.overlay = Some(app::help_text());
            return Action::None;
        }
        if let Some(screen) = function_screen(key.code) {
            self.set_screen(screen);
            self.overlay = None;
            return Action::None;
        }
        if modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('z') {
            return Action::Suspend;
        }
        if self.first_run && self.overlay.is_some() {
            match key.code {
                KeyCode::Char('y' | 'Y') => self.create_first_workspace(),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.first_run = false;
                    self.overlay = None;
                }
                _ => {}
            }
            return Action::None;
        }
        if self.filter_input.is_some() {
            return self.handle_filter_key(key);
        }
        if key.code == KeyCode::Esc {
            if self.overlay.take().is_some() {
                return Action::None;
            }
            if self.search.is_some() {
                self.reverse_search_cancel();
                return Action::None;
            }
            if self.prompt.completion_visible {
                self.prompt.completion_visible = false;
                return Action::None;
            }
            self.focus = Focus::Prompt;
            return Action::None;
        }
        if modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.search.is_some() {
                self.reverse_search_cancel();
                return Action::None;
            }
            if !self.prompt.chars.is_empty() {
                self.prompt.clear();
                self.ctrl_c_at = None;
                return Action::None;
            }
            if self
                .ctrl_c_at
                .is_some_and(|instant| instant.elapsed() < Duration::from_millis(1200))
            {
                return Action::Quit;
            }
            self.ctrl_c_at = Some(std::time::Instant::now());
            self.push_activity(
                "Press Ctrl-C again within one second to quit (Ctrl-Q quits immediately)."
                    .to_string(),
            );
            return Action::None;
        }
        if self.search.is_some() {
            return self.handle_search_key(key);
        }
        if modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            self.start_reverse_search();
            return Action::None;
        }
        if self.focus == Focus::Content {
            return self.handle_content_key(key);
        }
        self.handle_prompt_key(key)
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.reverse_search_next();
            }
            KeyCode::Enter => self.accept_reverse_search(),
            KeyCode::Esc => self.reverse_search_cancel(),
            KeyCode::Backspace => self.reverse_search_backspace(),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.reverse_search_input(character);
            }
            _ => {}
        }
        Action::None
    }

    fn handle_filter_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Enter => self.apply_filter(),
            KeyCode::Esc => self.cancel_filter(),
            KeyCode::Backspace => {
                if let Some(input) = self.filter_input.as_mut() {
                    input.pop();
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(input) = self.filter_input.as_mut() {
                    input.push(character);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_content_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll(-1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll(1);
                Action::None
            }
            KeyCode::Char('g') => {
                match self.screen {
                    Screen::Runs => {
                        if let Some(first) = self.visible_job_indices().first() {
                            self.job_index = *first;
                        }
                    }
                    Screen::Tests => self.test_index = 0,
                    _ => {}
                }
                Action::None
            }
            KeyCode::Char('G') => {
                match self.screen {
                    Screen::Runs => {
                        if let Some(last) = self.visible_job_indices().last() {
                            self.job_index = *last;
                        }
                    }
                    Screen::Tests => {
                        self.test_index = self.tests.len().saturating_sub(1);
                    }
                    _ => {}
                }
                Action::None
            }
            KeyCode::Tab if self.screen == Screen::Runs => {
                self.cycle_device_filter();
                Action::None
            }
            KeyCode::BackTab => {
                self.focus = Focus::Prompt;
                Action::None
            }
            KeyCode::Tab => {
                self.focus = Focus::Prompt;
                Action::None
            }
            KeyCode::Char('?') => {
                self.overlay = Some(app::help_text());
                Action::None
            }
            KeyCode::Char('/') if self.screen == Screen::Runs => {
                self.begin_filter();
                Action::None
            }
            KeyCode::Enter => match self.screen {
                Screen::Runs => {
                    self.follow_selected();
                    Action::None
                }
                Screen::Tests => self.open_selected_test(),
                Screen::Config => self.open_config(),
                Screen::Devices => Action::OpenEditor(self.ctx.devices_path.clone()),
                Screen::Apps => Action::OpenEditor(self.ctx.base.join("impresario.apps.config")),
                Screen::Home => Action::None,
            },
            KeyCode::Char('r') => {
                if self.screen == Screen::Runs {
                    self.rerun(false);
                } else if self.screen == Screen::Tests {
                    self.run_selected_test();
                } else {
                    self.reload_screen();
                }
                Action::None
            }
            KeyCode::Char('F') if self.screen == Screen::Runs => {
                self.rerun(true);
                Action::None
            }
            KeyCode::Char('b') if self.screen == Screen::Runs => {
                self.detach();
                Action::None
            }
            KeyCode::Char('f') if self.screen == Screen::Runs => {
                self.toggle_log_follow();
                Action::None
            }
            KeyCode::Char('x' | 's') if self.screen == Screen::Runs => {
                self.stop_selected();
                Action::None
            }
            KeyCode::Char('d') if self.screen == Screen::Runs => {
                self.detach();
                Action::None
            }
            KeyCode::Char('o') if self.screen == Screen::Runs => self.open_report(false),
            KeyCode::Char('p') if self.screen == Screen::Runs => self.open_report(true),
            KeyCode::Char('X') | KeyCode::Delete if self.screen == Screen::Runs => {
                self.remove_selected();
                Action::None
            }
            KeyCode::Char('d') if self.screen == Screen::Tests => {
                self.dry_run_selected_test();
                Action::None
            }
            KeyCode::Char(' ') if self.screen == Screen::Tests => {
                let index = self.test_index;
                if !self.test_marks.remove(&index) {
                    self.test_marks.insert(index);
                }
                Action::None
            }
            KeyCode::Char('q') => Action::Quit,
            _ => Action::None,
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.prompt.insert_char('\n');
                Action::None
            }
            KeyCode::Enter => self.execute_prompt(),
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.insert_char('\n');
                Action::None
            }
            KeyCode::Tab => {
                if self.prompt.completion_visible {
                    self.prompt.select_completion();
                } else {
                    self.focus = Focus::Content;
                }
                Action::None
            }
            KeyCode::BackTab => {
                self.prompt.previous_completion();
                Action::None
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                self.prompt.move_line(-1);
                Action::None
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                self.prompt.move_line(1);
                Action::None
            }
            KeyCode::Up => {
                if self.prompt.completion_visible {
                    self.prompt.previous_completion();
                } else if let Some(previous) = self.history.previous() {
                    self.prompt.set_text(&previous);
                }
                Action::None
            }
            KeyCode::Down => {
                if self.prompt.completion_visible {
                    self.prompt.next_completion();
                } else if let Some(next) = self.history.next() {
                    self.prompt.set_text(&next);
                } else {
                    self.prompt.clear();
                }
                Action::None
            }
            KeyCode::Left => {
                self.prompt.left();
                Action::None
            }
            KeyCode::Right => {
                self.prompt.right();
                Action::None
            }
            KeyCode::Home => {
                self.prompt.home();
                Action::None
            }
            KeyCode::End => {
                self.prompt.end();
                Action::None
            }
            KeyCode::Backspace => {
                self.prompt.backspace();
                Action::None
            }
            KeyCode::Delete => {
                self.prompt.delete();
                Action::None
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.home();
                Action::None
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.end();
                Action::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.kill_to_start();
                Action::None
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.kill_to_end();
                Action::None
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt.kill_word();
                Action::None
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.prompt.insert_char(character);
                Action::None
            }
            _ => Action::None,
        }
    }
}

fn function_screen(code: KeyCode) -> Option<Screen> {
    match code {
        KeyCode::F(2) => Some(Screen::Home),
        KeyCode::F(3) => Some(Screen::Runs),
        KeyCode::F(4) => Some(Screen::Tests),
        KeyCode::F(5) => Some(Screen::Devices),
        KeyCode::F(6) => Some(Screen::Apps),
        KeyCode::F(7) => Some(Screen::Config),
        _ => None,
    }
}

pub(crate) fn format_duration(millis: u64) -> String {
    let seconds = millis / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3_600, (seconds % 3_600) / 60)
    }
}

fn open_editor(path: &std::path::Path) -> Result<(), String> {
    let (program, prefix) = crate::cli::editor_command()?;
    let status = std::process::Command::new(program)
        .args(prefix)
        .arg(path)
        .status()
        .map_err(|error| format!("cannot start the editor: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("editor exited with {status}"))
    }
}

fn open_external(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]).arg(path);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot open {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_is_compact() {
        assert_eq!(format_duration(5_000), "5s");
        assert_eq!(format_duration(65_000), "1m05s");
        assert_eq!(format_duration(3_665_000), "1h01m");
    }

    #[test]
    fn function_keys_map_to_screens() {
        assert_eq!(function_screen(KeyCode::F(3)), Some(Screen::Runs));
        assert_eq!(function_screen(KeyCode::F(7)), Some(Screen::Config));
        assert_eq!(function_screen(KeyCode::F(8)), None);
    }
}
