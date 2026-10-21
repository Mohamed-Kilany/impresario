//! TUI drawing. All colours are from the 16-colour ANSI palette so terminals
//! that cannot do true colour still render correctly; `IMPRESARIO_ASCII` and
//! `TERM=dumb` switch the icons to text.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, LineStatus, TestKind};
use super::{Focus, Screen};

pub(super) fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < 40 || area.height < 12 {
        let text = vec![
            Line::from("impresario: terminal too small"),
            Line::from("resize to at least 40x12, or run with --plain"),
        ];
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
        return;
    }
    let prompt_height = prompt_height(app);
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(prompt_height),
        Constraint::Length(4),
    ])
    .split(area);
    let header = chunk(&chunks, 0, area);
    let body = chunk(&chunks, 1, area);
    let prompt = chunk(&chunks, 2, area);
    let status = chunk(&chunks, 3, area);
    draw_header(frame, app, header);
    match app.screen {
        Screen::Home => draw_home(frame, app, body),
        Screen::Runs => draw_runs(frame, app, body),
        Screen::Tests => draw_tests(frame, app, body),
        Screen::Devices => draw_devices(frame, app, body),
        Screen::Apps => draw_apps(frame, app, body),
        Screen::Config => draw_config(frame, app, body),
    }
    draw_prompt(frame, app, prompt);
    draw_status(frame, app, status);
    if let Some(text) = &app.overlay {
        draw_overlay(frame, app, area, text);
    }
}

fn chunk(chunks: &[Rect], index: usize, fallback: Rect) -> Rect {
    chunks.get(index).copied().unwrap_or(fallback)
}

fn prompt_height(app: &App) -> u16 {
    let lines = prompt_line_count(app).min(5);
    let completions = if app.prompt.completion_visible {
        app.prompt.completions.len().min(5)
    } else {
        0
    };
    let extra = if completions > 0 { completions + 1 } else { 0 };
    u16::try_from(2 + lines + extra).unwrap_or(7).max(3)
}

fn prompt_line_count(app: &App) -> usize {
    if app.search.is_some() {
        return 1;
    }
    let text = app.prompt.text();
    text.lines().count().max(1)
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!(
        " impresario  {}  {}",
        app.ctx.base.display(),
        if app.ctx.color { "" } else { "[plain colours]" }
    );
    let tabs = [
        Screen::Home,
        Screen::Runs,
        Screen::Tests,
        Screen::Devices,
        Screen::Apps,
        Screen::Config,
    ];
    let mut tab_spans = Vec::new();
    for screen in tabs {
        let style = if screen == app.screen {
            Style::default()
                .fg(if app.ctx.color {
                    Color::Cyan
                } else {
                    Color::Reset
                })
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
        };
        tab_spans.push(Span::styled(format!(" {} ", screen.label()), style));
    }
    let active = app
        .jobs
        .iter()
        .filter(|record| !record.status.terminal())
        .count();
    let summary = format!(
        "{active} active job(s) | {} device(s) | {} app(s) | F1 help",
        app.devices.len(),
        app.apps.len()
    );
    let lines = vec![
        Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(tab_spans),
        Line::from(Span::styled(summary, secondary(app))),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(secondary(app)),
        ),
        area,
    );
}

fn draw_home(frame: &mut Frame, app: &App, area: Rect) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(area);
    let left_area = chunk(&chunks, 0, area);
    let right_area = chunk(&chunks, 1, area);
    let active = app
        .jobs
        .iter()
        .filter(|record| !record.status.terminal())
        .count();
    let scenes = app
        .tests
        .iter()
        .filter(|row| row.kind == TestKind::Scene)
        .count();
    let theatres = app
        .tests
        .iter()
        .filter(|row| row.kind == TestKind::Theatre)
        .count();
    let plays = app
        .tests
        .iter()
        .filter(|row| row.kind == TestKind::Play)
        .count();
    let mut left = vec![
        Line::from(Span::styled("Dashboard", title_style(app))),
        Line::from(""),
        Line::from(format!("workspace     {}", app.ctx.base.display())),
        Line::from(format!("config        {}", app.ctx.config_path.display())),
        Line::from(format!("devices       {}", app.ctx.devices_path.display())),
        Line::from(""),
        Line::from(format!(
            "tests         {scenes} scene(s), {theatres} theatre(s), {plays} play(s)"
        )),
        Line::from(format!("devices       {} configured", app.devices.len())),
        Line::from(format!("apps          {} configured", app.apps.len())),
        Line::from(format!(
            "jobs          {active} active, {} total",
            app.jobs.len()
        )),
        Line::from(""),
        Line::from(Span::styled("Recent jobs", title_style(app))),
    ];
    if app.jobs.is_empty() {
        left.push(Line::from(Span::styled(
            "  no jobs yet; run a play from the prompt",
            secondary(app),
        )));
    } else {
        for record in app.jobs.iter().take(6) {
            left.push(job_line(app, record, false));
        }
    }
    left.push(Line::from(""));
    left.push(Line::from(Span::styled(
        "Try: run --tag smoke --detach",
        secondary(app),
    )));
    left.push(Line::from(Span::styled(
        "     tests --tag smoke --platform android",
        secondary(app),
    )));
    frame.render_widget(
        Paragraph::new(left)
            .block(Block::default().borders(Borders::ALL).title("workspace"))
            .wrap(Wrap { trim: false }),
        left_area,
    );

    let mut right = Vec::new();
    right.push(Line::from(Span::styled("Activity", title_style(app))));
    right.push(Line::from(""));
    let limit = usize::from(right_area.height.saturating_sub(2)).max(4);
    let skip = app.activity.len().saturating_sub(limit);
    for line in app.activity.iter().skip(skip) {
        right.push(Line::from(line.clone()));
    }
    if app.activity.is_empty() {
        right.push(Line::from(Span::styled("No activity yet.", secondary(app))));
    }
    frame.render_widget(
        Paragraph::new(right)
            .block(Block::default().borders(Borders::ALL).title("log"))
            .wrap(Wrap { trim: false }),
        right_area,
    );
}

fn draw_runs(frame: &mut Frame, app: &App, area: Rect) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)]).split(area);
    let list_area = chunk(&chunks, 0, area);
    let detail_area = chunk(&chunks, 1, area);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("Jobs ({})", app.jobs.len()),
        title_style(app),
    )));
    lines.push(Line::from(Span::styled(
        "Enter follow  b background  x stop  f log tail",
        secondary(app),
    )));
    lines.push(Line::from(Span::styled(
        "Tab device  / filter  ? help  r rerun  F failed",
        secondary(app),
    )));
    lines.push(Line::from(Span::styled(
        "o report  p screenshot  X remove",
        secondary(app),
    )));
    if let Some(input) = &app.filter_input {
        lines.push(Line::from(Span::styled(
            format!("filter> {input}_  (Enter apply, Esc cancel)"),
            active_border(app),
        )));
    } else if !app.filter.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("filter: {}  (/ to change, empty to clear)", app.filter),
            secondary(app),
        )));
    }
    if let Some(device) = &app.device_filter {
        lines.push(Line::from(Span::styled(
            format!("device: {device}  (Tab cycles, Shift-Tab clears)"),
            secondary(app),
        )));
    }
    lines.push(Line::from(""));
    let visible = app.visible_job_indices();
    if app.jobs.is_empty() {
        lines.push(Line::from("No jobs. Run a play from the prompt."));
    } else if visible.is_empty() {
        lines.push(Line::from("No jobs match the filter."));
    }
    for index in visible {
        let Some(record) = app.jobs.get(index) else {
            continue;
        };
        let selected = index == app.job_index;
        let followed = app.follow.as_deref() == Some(record.id.as_str());
        lines.push(job_line(app, record, selected || followed));
        if let Some(position) = record.queue_position {
            lines.push(Line::from(Span::styled(
                format!("    queued (position {position})"),
                secondary(app),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).border_style(
                if app.focus == Focus::Content {
                    active_border(app)
                } else {
                    secondary(app)
                },
            ))
            .wrap(Wrap { trim: false }),
        list_area,
    );

    let detail_chunks = Layout::vertical([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(detail_area);
    let tree_area = chunk(&detail_chunks, 0, detail_area);
    let log_area = chunk(&detail_chunks, 1, detail_area);
    let record = app.selected_job();
    let title = match record {
        Some(record) => format!(
            "{}  {}  {}",
            record.id,
            record.status.label(),
            app.elapsed_label()
        ),
        None => "run tree".to_string(),
    };
    let mut tree_lines = Vec::new();
    if let Some(command) = Some(app.current_command.as_str()).filter(|text| !text.is_empty()) {
        tree_lines.push(Line::from(Span::styled(
            format!("current: {command}"),
            Style::default().fg(if app.ctx.color {
                Color::Yellow
            } else {
                Color::Reset
            }),
        )));
    }
    if app.follow.is_none() {
        if let Some(record) = record {
            tree_lines.push(Line::from(format!("job      {}", record.id)));
            tree_lines.push(Line::from(format!("status   {}", record.status.label())));
            tree_lines.push(Line::from(format!(
                "devices  {}",
                record.devices.join(", ")
            )));
            tree_lines.push(Line::from(format!("plays    {}", record.plays.join(", "))));
            if let Some(message) = &record.message {
                tree_lines.push(Line::from(format!("message  {message}")));
            }
            tree_lines.push(Line::from(""));
            tree_lines.push(Line::from(Span::styled(
                "Press Enter to follow this job and load its live run tree.",
                secondary(app),
            )));
        } else {
            tree_lines.push(Line::from("No job selected."));
        }
    } else {
        let max = usize::from(tree_area.height.saturating_sub(3)).max(3);
        let skip = app.tree.len().saturating_sub(max);
        for line in app.tree.iter().skip(skip) {
            if !app.line_matches_filter(&line.text) {
                continue;
            }
            tree_lines.push(tree_line(app, line));
        }
        if app.tree.is_empty() {
            tree_lines.push(Line::from("Waiting for the first event..."));
        }
    }
    frame.render_widget(
        Paragraph::new(tree_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(active_border(app)),
            )
            .wrap(Wrap { trim: false }),
        tree_area,
    );

    let mut logs = Vec::new();
    let max = usize::from(log_area.height.saturating_sub(3)).max(3);
    let skip = if app.log_follow {
        app.logs.len().saturating_sub(max)
    } else {
        0
    };
    for line in app.logs.iter().skip(skip).take(max) {
        if !app.line_matches_filter(line) {
            continue;
        }
        logs.push(Line::from(line.clone()));
    }
    if logs.is_empty() {
        logs.push(Line::from(Span::styled(
            "Device log lines and step output appear here.",
            secondary(app),
        )));
    }
    let log_title = if app.log_follow {
        "logs  (following · f pauses)"
    } else {
        "logs  (paused · f follows)"
    };
    frame.render_widget(
        Paragraph::new(logs)
            .block(Block::default().borders(Borders::ALL).title(log_title))
            .wrap(Wrap { trim: false }),
        log_area,
    );
}

fn draw_tests(frame: &mut Frame, app: &App, area: Rect) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(66), Constraint::Percentage(34)]).split(area);
    let list_area = chunk(&chunks, 0, area);
    let detail_area = chunk(&chunks, 1, area);
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "Test explorer ({} row(s), {} marked)",
            app.tests.len(),
            app.test_marks.len()
        ),
        title_style(app),
    )));
    lines.push(Line::from(Span::styled(
        "Enter edit  Space mark  r run  d dry-run",
        secondary(app),
    )));
    lines.push(Line::from(""));
    if let Some(error) = &app.test_error {
        lines.push(Line::from(Span::styled(error.clone(), error_style(app))));
    }
    if app.tests.is_empty() && app.test_error.is_none() {
        lines.push(Line::from("No tests found."));
    }
    let max = usize::from(list_area.height.saturating_sub(4)).max(4);
    let start = app.test_index.saturating_sub(max / 2);
    let end = start.saturating_add(max).min(app.tests.len());
    for (index, row) in app
        .tests
        .iter()
        .enumerate()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        let indicator = if app.test_marks.contains(&index) {
            if app.ctx.ascii {
                "[*]"
            } else {
                "●"
            }
        } else {
            " "
        };
        let kind = match row.kind {
            TestKind::Scene => "scene",
            TestKind::Piece => "piece",
            TestKind::Theatre => "theatre",
            TestKind::Stage => "stage",
            TestKind::Play => "play",
            TestKind::Act => "act",
        };
        let location = match (&row.file, row.line) {
            (Some(file), Some(line)) => format!("  {}:{line}", file.display()),
            (Some(file), None) => format!("  {}", file.display()),
            _ => String::new(),
        };
        let label = format!(
            "{indicator} {}{}  [{kind}]{location}",
            "  ".repeat(row.indent),
            row.label
        );
        let style = if index == app.test_index {
            selected_style(app)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(label, style)));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).border_style(
                if app.focus == Focus::Content {
                    active_border(app)
                } else {
                    secondary(app)
                },
            ))
            .wrap(Wrap { trim: false }),
        list_area,
    );

    let mut detail = vec![Line::from(Span::styled("Selection", title_style(app)))];
    if let Some(row) = app.selected_test() {
        detail.push(Line::from(""));
        detail.push(Line::from(format!("name  {}", row.name)));
        detail.push(Line::from(format!(
            "kind  {}",
            match row.kind {
                TestKind::Scene => "scene",
                TestKind::Piece => "piece",
                TestKind::Theatre => "theatre",
                TestKind::Stage => "stage",
                TestKind::Play => "play",
                TestKind::Act => "act",
            }
        )));
        if let Some(file) = &row.file {
            detail.push(Line::from(format!("file  {}", file.display())));
        }
        if let Some(line) = row.line {
            detail.push(Line::from(format!("line  {line}")));
        }
        detail.push(Line::from(""));
        detail.push(Line::from(Span::styled(
            "r runs the selected play/act; marked rows are added. d prints the dry-run matrix.",
            secondary(app),
        )));
        if let Some(file) = &row.file {
            detail.push(Line::from(""));
            detail.push(Line::from(Span::styled(
                format!("run:  --play {}", file.display()),
                secondary(app),
            )));
        }
    } else {
        detail.push(Line::from(""));
        detail.push(Line::from("No row selected."));
    }
    detail.push(Line::from(""));
    detail.push(Line::from(Span::styled(
        "Filters: /tests --tag smoke",
        secondary(app),
    )));
    detail.push(Line::from(Span::styled(
        "         /tests --play plays/login.play",
        secondary(app),
    )));
    detail.push(Line::from(Span::styled(
        "         /tests --platform android --exclude-tag slow",
        secondary(app),
    )));
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title("details"))
            .wrap(Wrap { trim: false }),
        detail_area,
    );
}

fn draw_devices(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled("Devices", title_style(app))),
        Line::from(Span::styled(
            "Enter opens the devices file. r reloads.",
            secondary(app),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{:<24} {:<12} {}", "NAME", "PLATFORM", "TRANSPORT"),
            title_style(app),
        )),
    ];
    if app.devices.is_empty() {
        lines.push(Line::from("No devices configured."));
    }
    for device in &app.devices {
        lines.push(Line::from(format!(
            "{:<24} {:<12} {}",
            device.name, device.platform, device.connection
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(active_border(app)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_apps(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled("Apps", title_style(app))),
        Line::from(Span::styled(
            "Enter opens impresario.apps.config. r reloads.",
            secondary(app),
        )),
        Line::from(""),
    ];
    if app.apps.is_empty() {
        lines.push(Line::from("No apps configured."));
    }
    for app_row in &app.apps {
        lines.push(Line::from(Span::styled(
            app_row.name.clone(),
            title_style(app),
        )));
        lines.push(Line::from(format!(
            "  platforms: {}  {}",
            app_row.platforms, app_row.description
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(active_border(app)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_config(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled("Configuration", title_style(app))),
        Line::from(Span::styled(
            "Enter opens the workspace config. r reloads.",
            secondary(app),
        )),
        Line::from(""),
    ];
    let max = usize::from(area.height.saturating_sub(5)).max(4);
    for row in app.config_rows.iter().take(max) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<24}", row.key),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(row.value.clone()),
            Span::styled(format!("   [{}]", row.source), secondary(app)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(active_border(app)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    if app.prompt.completion_visible {
        for (index, completion) in app.prompt.completions.iter().take(5).enumerate() {
            let style = if index == app.prompt.completion_index {
                selected_style(app)
            } else {
                secondary(app)
            };
            lines.push(Line::from(Span::styled(format!("  {completion}"), style)));
        }
        if !app.prompt.completions.is_empty() {
            lines.push(Line::from(Span::styled(
                "  Tab accepts · Up/Down cycles",
                secondary(app),
            )));
        }
    }
    let (display_lines, cursor_line, cursor_column) = if let Some(search) = &app.search {
        let current = search.current().cloned().unwrap_or_default();
        let text = format!("(reverse-i-search)`{}`: {current}", search.query);
        (vec![text.clone()], 0usize, text.chars().count())
    } else {
        let text = app.prompt.text();
        let mut all: Vec<String> = text.lines().map(str::to_string).collect();
        if all.is_empty() {
            all.push(String::new());
        }
        let show_from = all.len().saturating_sub(5);
        let mut shown: Vec<String> = all.iter().skip(show_from).cloned().collect();
        if let Some(first) = shown.first_mut() {
            if !text.is_empty() || app.focus == Focus::Prompt {
                first.insert_str(0, "❯ ");
            }
        }
        let before_cursor: String = app.prompt.chars.iter().take(app.prompt.cursor).collect();
        let cursor_line = before_cursor.matches('\n').count();
        let cursor_column = before_cursor
            .rsplit('\n')
            .next()
            .map_or(0, |line| line.chars().count());
        let shown_cursor_line = cursor_line.saturating_sub(show_from).min(4);
        (shown, shown_cursor_line, cursor_column)
    };
    for line in &display_lines {
        lines.push(Line::from(line.clone()));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(if app.focus == Focus::Prompt {
                        "prompt  (Tab completes · Ctrl-R history · F1 help)"
                    } else {
                        "prompt  (Tab focuses the prompt)"
                    })
                    .border_style(if app.focus == Focus::Prompt {
                        active_border(app)
                    } else {
                        secondary(app)
                    }),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
    if app.overlay.is_none() {
        let completions = if app.prompt.completion_visible {
            app.prompt.completions.len().min(5) + usize::from(!app.prompt.completions.is_empty())
        } else {
            0
        };
        let x = area
            .x
            .saturating_add(2)
            .saturating_add(u16::try_from(cursor_column).unwrap_or(0))
            .min(area.right().saturating_sub(1));
        let y = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(completions).unwrap_or(0))
            .saturating_add(u16::try_from(cursor_line).unwrap_or(0))
            .min(area.bottom().saturating_sub(2));
        frame.set_cursor_position((x, y));
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match app.screen {
        Screen::Runs if app.focus == Focus::Content => {
            "Enter follow · b background · x stop · f log tail · Tab device · / filter · ? help · Esc prompt"
        }
        Screen::Tests if app.focus == Focus::Content => {
            "Enter edit · Space mark · r run · d dry-run · j/k move · Tab prompt"
        }
        Screen::Devices | Screen::Apps | Screen::Config if app.focus == Focus::Content => {
            "Enter open · r reload · j/k scroll · Tab prompt"
        }
        _ => "F1 help · F2 home · F3 runs · F4 tests · F5 devices · F6 apps · F7 config · Ctrl-Q quit",
    };
    let selected = app
        .selected_job()
        .map(|record| {
            let elapsed = app.elapsed_label();
            let elapsed = if elapsed == record.status.label() {
                String::new()
            } else {
                format!(" {elapsed}")
            };
            format!("job {} {}{elapsed}", record.id, record.status.label(),)
        })
        .unwrap_or_else(|| "no job selected".to_string());
    let message = app
        .activity
        .back()
        .cloned()
        .unwrap_or_else(|| "ready".to_string());
    let lines = vec![
        Line::from(Span::styled(keys, secondary(app))),
        Line::from(Span::styled(selected, title_style(app))),
        Line::from(Span::styled(message, secondary(app))),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::TOP))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_overlay(frame: &mut Frame, app: &App, area: Rect, text: &str) {
    let width = area.width.saturating_mul(4) / 5;
    let height = area.height.saturating_mul(4) / 5;
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    let popup = Rect::new(x, y, width.max(20), height.max(8));
    frame.render_widget(Clear, popup);
    let lines: Vec<Line> = text
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" help · Esc closes ")
                    .border_style(active_border(app)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn job_line(app: &App, record: &crate::jobs::JobRecord, selected: bool) -> Line<'static> {
    let icon = job_icon(app, record);
    let detail = record
        .duration_ms
        .map(super::format_duration)
        .unwrap_or_else(|| record.status.label().to_string());
    let text = format!("{icon} {}  {detail}", record.id);
    let style = if selected {
        selected_style(app)
    } else {
        Style::default()
    };
    Line::from(Span::styled(text, style))
}

fn job_icon(app: &App, record: &crate::jobs::JobRecord) -> &'static str {
    let ascii = app.ctx.ascii;
    match record.status {
        crate::jobs::JobStatus::Passed => {
            if ascii {
                "[ok]"
            } else {
                "✓"
            }
        }
        crate::jobs::JobStatus::Failed
        | crate::jobs::JobStatus::Error
        | crate::jobs::JobStatus::Stopped => {
            if ascii {
                "[x]"
            } else {
                "✗"
            }
        }
        crate::jobs::JobStatus::Interrupted => {
            if ascii {
                "[!]"
            } else {
                "!"
            }
        }
        crate::jobs::JobStatus::Queued => {
            if ascii {
                "[.]"
            } else {
                "…"
            }
        }
        crate::jobs::JobStatus::Running => {
            if ascii {
                "[>]"
            } else {
                "▶"
            }
        }
    }
}

fn tree_line(app: &App, line: &super::app::TreeLine) -> Line<'static> {
    let icon = match line.status {
        LineStatus::Passed => {
            if app.ctx.ascii {
                "[ok]"
            } else {
                "✓"
            }
        }
        LineStatus::Failed => {
            if app.ctx.ascii {
                "[x]"
            } else {
                "✗"
            }
        }
        LineStatus::Skipped => "-",
        LineStatus::Aborted => "!",
        LineStatus::Running => {
            if app.ctx.ascii {
                "[>]"
            } else {
                "▶"
            }
        }
    };
    let style = status_style(app, line.status);
    let prefix = "  ".repeat(line.indent);
    Line::from(Span::styled(format!("{prefix}{icon} {}", line.text), style))
}

fn status_style(app: &App, status: LineStatus) -> Style {
    if !app.ctx.color {
        return Style::default();
    }
    let color = match status {
        LineStatus::Passed => Color::Green,
        LineStatus::Failed => Color::Red,
        LineStatus::Skipped => Color::DarkGray,
        LineStatus::Aborted => Color::Magenta,
        LineStatus::Running => Color::Cyan,
    };
    Style::default().fg(color)
}

fn selected_style(app: &App) -> Style {
    let style = Style::default().add_modifier(Modifier::REVERSED);
    if app.ctx.color {
        style.fg(Color::Yellow)
    } else {
        style
    }
}

fn title_style(app: &App) -> Style {
    let style = Style::default().add_modifier(Modifier::BOLD);
    if app.ctx.color {
        style.fg(Color::Cyan)
    } else {
        style
    }
}

fn secondary(app: &App) -> Style {
    if app.ctx.color {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    }
}

fn error_style(app: &App) -> Style {
    if app.ctx.color {
        Style::default().fg(Color::Red)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn active_border(app: &App) -> Style {
    if app.ctx.color {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}
