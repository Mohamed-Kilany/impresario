//! TUI drawing. All colours are from the 16-colour ANSI palette so terminals
//! that cannot do true colour still render correctly; `IMPRESARIO_ASCII` and
//! `TERM=dumb` switch the icons and borders to text.

use std::path::Path;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
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
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(prompt_height(app)),
        Constraint::Length(4),
    ])
    .split(area);
    let header = chunk(&chunks, 0, area);
    let body = chunk(&chunks, 1, area);
    let prompt = chunk(&chunks, 2, area);
    let status = chunk(&chunks, 3, area);
    if app.screen == Screen::Landing {
        draw_landing(frame, app, area);
        if let Some(text) = &app.overlay {
            draw_overlay(frame, app, area, text);
        }
        return;
    }
    draw_header(frame, app, header);
    match app.screen {
        Screen::Menu => draw_menu(frame, app, body),
        Screen::Home => draw_home(frame, app, body),
        Screen::Runs => draw_runs(frame, app, body),
        Screen::Tests => draw_tests(frame, app, body),
        Screen::Devices => draw_devices(frame, app, body),
        Screen::Apps => draw_apps(frame, app, body),
        Screen::Config => draw_config(frame, app, body),
        Screen::Landing => {}
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
    u16::try_from(2 + lines).unwrap_or(7).max(3)
}

fn prompt_line_count(app: &App) -> usize {
    if app.search.is_some() {
        return 1;
    }
    let text = app.prompt.text();
    text.lines().count().max(1)
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let active = app
        .jobs
        .iter()
        .filter(|record| !record.status.terminal())
        .count();
    let workspace = app
        .ctx
        .base
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map_or_else(|| app.ctx.base.display().to_string(), str::to_string);
    let mut title = vec![
        Span::styled(" IMPRESARIO", title_style(app)),
        Span::styled("  /  ", secondary(app)),
        Span::styled(workspace, Style::default().add_modifier(Modifier::BOLD)),
    ];
    if !app.ctx.color {
        title.push(Span::styled("  [plain colours]", secondary(app)));
    }
    let summary = format!(
        "{active} active · {} · {} ",
        count_label(app.devices.len(), "device", "devices"),
        count_label(app.apps.len(), "app", "apps")
    );
    let title_line = with_right(title, &summary, area.width, secondary(app));

    let section = match app.screen {
        Screen::Landing => "Workspaces",
        Screen::Menu => "Workspace menu",
        Screen::Home => "Overview",
        Screen::Runs => "Runs",
        Screen::Tests => "Tests",
        Screen::Devices => "Devices",
        Screen::Apps => "Apps",
        Screen::Config => "Config",
    };
    let crumb = vec![
        Span::styled(" Workspace", secondary(app)),
        Span::styled("  ›  ", secondary(app)),
        Span::styled(
            section,
            Style::default()
                .fg(color_or_reset(app, Color::Cyan))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let hint = if app.screen == Screen::Menu {
        "Esc workspaces · ? help · Ctrl-Q quit "
    } else {
        "Esc menu · ? help · Ctrl-Q quit "
    };
    let tabs_line = with_right(crumb, hint, area.width, secondary(app));
    frame.render_widget(
        Paragraph::new(vec![title_line, tabs_line]).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_type(border_type(app))
                .border_style(secondary(app)),
        ),
        area,
    );
}

const WORDMARK_GLYPHS: [[&str; 5]; 10] = [
    [" ██  ", " ██  ", " ██  ", " ██  ", " ██  "],
    ["█   █", "██ ██", "█ █ █", "█   █", "█   █"],
    ["████ ", "█   █", "████ ", "█    ", "█    "],
    ["████ ", "█   █", "████ ", "█ █  ", "█  █ "],
    ["█████", "█    ", "████ ", "█    ", "█████"],
    ["█████", "█    ", "████ ", "    █", "█████"],
    [" ███ ", "█   █", "█████", "█   █", "█   █"],
    ["████ ", "█   █", "████ ", "█ █  ", "█  █ "],
    [" ██  ", " ██  ", " ██  ", " ██  ", " ██  "],
    [" ███ ", "█   █", "█   █", "█   █", " ███ "],
];

const LANDING_ACTIONS: [(&str, &str); 3] = [
    ("+", "Add current workspace"),
    ("✚", "Doctor"),
    ("×", "Quit"),
];

fn draw_landing(frame: &mut Frame, app: &App, area: Rect) {
    // The wordmark itself fits at 66 columns; keep it as art there and only
    // fall back to spaced letters when the terminal is genuinely too narrow
    // or too short.
    let compact_art = area.width < 66 || area.height < 16;
    let compact_layout = area.width < 76 || area.height < 20;
    let art_height = if compact_art { 3 } else { 6 };
    let chunks = Layout::vertical([
        Constraint::Length(art_height),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(area);
    let art_area = chunk(&chunks, 0, area);
    let body_area = chunk(&chunks, 1, area);
    let footer_area = chunk(&chunks, 2, area);

    let mut art = wordmark_lines(app, compact_art);
    art.push(Line::from(Span::styled(
        if compact_art {
            "workspace-first test automation"
        } else {
            "workspace-first test automation  ·  scenes, theatres and plays"
        },
        secondary(app),
    )));
    frame.render_widget(Paragraph::new(art).alignment(Alignment::Center), art_area);

    if compact_layout {
        draw_landing_list(frame, app, body_area);
    } else {
        let columns = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)])
            .spacing(1)
            .split(body_area);
        draw_landing_list(frame, app, chunk(&columns, 0, body_area));
        draw_landing_detail(frame, app, chunk(&columns, 1, body_area));
    }

    let keys = if app.ctx.ascii {
        "  Up/Down choose   Enter open   a add   d doctor   ? help   q quit"
    } else {
        "  ↑↓ choose   Enter open   a add   d doctor   ? help   q quit"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(keys, secondary(app))),
            Line::from(Span::styled(
                "  Active workspace is remembered; add others here.",
                secondary(app),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_type(border_type(app))
                .border_style(secondary(app)),
        ),
        footer_area,
    );
}

fn wordmark_lines(app: &App, compact: bool) -> Vec<Line<'static>> {
    if compact {
        let text = "I M P R E S A R I O";
        let mut spans = Vec::new();
        for (index, character) in text.chars().enumerate() {
            spans.push(Span::styled(
                character.to_string(),
                wordmark_style(app, index),
            ));
        }
        return vec![Line::from(spans)];
    }
    let mut lines = Vec::new();
    for row in 0..5 {
        let mut text = String::new();
        for (index, glyph) in WORDMARK_GLYPHS.iter().enumerate() {
            if index > 0 {
                text.push(' ');
            }
            text.push_str(glyph.get(row).copied().unwrap_or(""));
        }
        let mut spans = Vec::new();
        for (column, character) in text.chars().enumerate() {
            let character = if app.ctx.ascii && character == '█' {
                '#'
            } else {
                character
            };
            spans.push(Span::styled(
                character.to_string(),
                wordmark_style(app, column.saturating_add(row * 3)),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn wordmark_style(app: &App, index: usize) -> Style {
    if !app.ctx.color {
        return Style::default().add_modifier(Modifier::BOLD);
    }
    const PALETTE: [Color; 6] = [
        Color::LightCyan,
        Color::Cyan,
        Color::LightMagenta,
        Color::Magenta,
        Color::LightYellow,
        Color::LightGreen,
    ];
    Style::default()
        .fg(PALETTE
            .get((index / 5) % PALETTE.len())
            .copied()
            .unwrap_or(Color::Cyan))
        .add_modifier(Modifier::BOLD)
}

fn draw_landing_list(frame: &mut Frame, app: &App, area: Rect) {
    let width = usize::from(area.width.saturating_sub(4)).max(10);
    let height = usize::from(area.height.saturating_sub(2)).max(1);
    let total = app.landing_len();
    let start = if app.landing_index >= height {
        app.landing_index.saturating_add(1).saturating_sub(height)
    } else {
        0
    };
    let mut lines = Vec::new();
    for index in start..total.min(start.saturating_add(height)) {
        lines.push(landing_line(app, index, width));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, "Workspaces · Enter opens", true))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn landing_line(app: &App, index: usize, width: usize) -> Line<'static> {
    let selected = index == app.landing_index;
    let pointer = if selected {
        if app.ctx.ascii {
            ">"
        } else {
            "›"
        }
    } else {
        " "
    };
    let style = if selected {
        selected_style(app)
    } else {
        Style::default()
    };
    if let Some(workspace) = app.workspaces.get(index) {
        let badge = if workspace.current {
            "current"
        } else if workspace.missing {
            "missing"
        } else {
            ""
        };
        let badge = if badge.is_empty() {
            String::new()
        } else {
            format!("{badge} ")
        };
        let label = format!("{pointer} {}", workspace.name);
        let available = width.saturating_sub(badge.chars().count() + 1);
        let label = elide(&label, available);
        let pad = width.saturating_sub(label.chars().count() + badge.chars().count());
        return Line::from(vec![
            Span::styled(label, style),
            Span::styled(" ".repeat(pad), style),
            Span::styled(badge, if selected { style } else { secondary(app) }),
        ]);
    }
    let action = index.saturating_sub(app.workspaces.len());
    let (icon, label) = LANDING_ACTIONS
        .get(action)
        .copied()
        .unwrap_or(("·", "Unknown action"));
    let icon = if app.ctx.ascii {
        match action {
            0 => "+",
            1 => "D",
            _ => "x",
        }
    } else {
        icon
    };
    Line::from(Span::styled(format!("{pointer} {icon} {label}"), style))
}

fn draw_landing_detail(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    if let Some(workspace) = app.workspaces.get(app.landing_index) {
        lines.push(Line::from(Span::styled(
            workspace.name.clone(),
            title_style(app),
        )));
        lines.push(Line::from(Span::styled(
            elide(
                &workspace.path.display().to_string(),
                usize::from(area.width.saturating_sub(4)).max(8),
            ),
            secondary(app),
        )));
        lines.push(Line::from(""));
        lines.push(kv_line(
            app,
            "config",
            if workspace.path.join("impresario.config").exists() {
                "present".to_string()
            } else {
                "missing".to_string()
            },
        ));
        lines.push(kv_line(
            app,
            "status",
            if workspace.current {
                "active workspace".to_string()
            } else if workspace.missing {
                "path not found".to_string()
            } else {
                "remembered workspace".to_string()
            },
        ));
        lines.push(Line::from(""));
        if workspace.current {
            lines.push(Line::from(Span::styled(
                "Enter opens the workspace menu.",
                secondary(app),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "Enter switches to this workspace.",
                secondary(app),
            )));
        }
    } else {
        let action = app.landing_index.saturating_sub(app.workspaces.len());
        let (title, description) = match action {
            0 => (
                "Add current workspace",
                "Remember the current workspace in the global list so it appears here next time.",
            ),
            1 => (
                "Doctor",
                "Probe the toolchain, platform prerequisites and configured devices without starting a run.",
            ),
            _ => (
                "Quit",
                "Leave impresario. Ctrl-Q also quits from any workspace screen.",
            ),
        };
        lines.push(Line::from(Span::styled(title, title_style(app))));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(description, secondary(app))));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, "Selection", false).padding(Padding::horizontal(1)))
            .wrap(Wrap { trim: false }),
        area,
    );
}

const MENU: [(&str, &str); 9] = [
    ("Overview", "summary and activity"),
    ("Runs", "background jobs and live logs"),
    ("Tests", "scene, theatre and play explorer"),
    ("Devices", "configured targets"),
    ("Apps", "shared app bindings"),
    ("Config", "effective settings"),
    ("Doctor", "toolchain diagnostics"),
    ("Switch workspace", "back to the workspace picker"),
    ("Quit", "leave impresario"),
];

fn draw_menu(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::horizontal([Constraint::Percentage(54), Constraint::Percentage(46)])
        .spacing(1)
        .split(area);
    let left = chunk(&columns, 0, area);
    let right = chunk(&columns, 1, area);
    let width = usize::from(left.width.saturating_sub(4)).max(12);
    let mut lines = Vec::new();
    for (index, (label, hint)) in MENU.iter().enumerate() {
        let selected = index == app.menu_index;
        let pointer = if selected {
            if app.ctx.ascii {
                ">"
            } else {
                "›"
            }
        } else {
            " "
        };
        let style = if selected {
            selected_style(app)
        } else {
            Style::default()
        };
        let left_text = format!("{pointer} {label}");
        let hint = if selected { *hint } else { "" };
        let available = width.saturating_sub(hint.chars().count() + 2);
        let left_text = elide(&left_text, available);
        let pad = width.saturating_sub(left_text.chars().count() + hint.chars().count());
        lines.push(Line::from(vec![
            Span::styled(left_text, style),
            Span::styled(" ".repeat(pad), style),
            Span::styled(hint, secondary(app)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel(app, "Workspace menu · Enter opens", true)),
        left,
    );

    let right_chunks =
        Layout::vertical([Constraint::Percentage(58), Constraint::Min(5)]).split(right);
    let info_area = chunk(&right_chunks, 0, right);
    let help_area = chunk(&right_chunks, 1, right);
    let workspace = app
        .ctx
        .base
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map_or_else(|| app.ctx.base.display().to_string(), str::to_string);
    let active = app
        .jobs
        .iter()
        .filter(|record| !record.status.terminal())
        .count();
    let info_width = usize::from(info_area.width.saturating_sub(4)).max(8);
    let info = vec![
        Line::from(Span::styled(workspace, title_style(app))),
        Line::from(Span::styled(
            elide(&app.ctx.base.display().to_string(), info_width),
            secondary(app),
        )),
        Line::from(""),
        kv_line(
            app,
            "tests",
            count_label(app.tests.len(), "entry", "entries"),
        ),
        kv_line(
            app,
            "devices",
            count_label(app.devices.len(), "device", "devices"),
        ),
        kv_line(app, "apps", count_label(app.apps.len(), "app", "apps")),
        kv_line(app, "runs", count_label(app.jobs.len(), "run", "runs")),
        kv_line(app, "active", active.to_string()),
    ];
    frame.render_widget(
        Paragraph::new(info)
            .block(panel(app, "Workspace", false).padding(Padding::horizontal(1)))
            .wrap(Wrap { trim: false }),
        info_area,
    );
    let help = vec![
        Line::from(Span::styled(
            "Arrows move; Enter opens; Esc returns to the picker; Tab opens the prompt.",
            secondary(app),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "/run, /attach, /logs and the other slash commands still work here.",
            secondary(app),
        )),
    ];
    frame.render_widget(
        Paragraph::new(help)
            .block(panel(app, "Navigation", false).padding(Padding::horizontal(1)))
            .wrap(Wrap { trim: false }),
        help_area,
    );
}

fn draw_home(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
        .spacing(1)
        .split(area);
    let left_area = chunk(&columns, 0, area);
    let right_area = chunk(&columns, 1, area);
    let left_chunks = Layout::vertical([Constraint::Percentage(60), Constraint::Min(5)])
        .spacing(1)
        .split(left_area);
    let info_area = chunk(&left_chunks, 0, left_area);
    let jobs_area = chunk(&left_chunks, 1, left_area);

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

    let info_width = usize::from(info_area.width.saturating_sub(4)).max(10);
    let roomy = info_area.height >= 11;
    let mut info = vec![Line::from(Span::styled(
        elide(&app.ctx.base.display().to_string(), info_width),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if roomy {
        info.push(Line::from(""));
    }
    info.extend([
        kv_line(app, "config", relative_path(app, &app.ctx.config_path)),
        kv_line(app, "devices", relative_path(app, &app.ctx.devices_path)),
    ]);
    if roomy {
        info.push(kv_line(
            app,
            "apps",
            relative_path(app, &app.ctx.base.join("impresario.apps.config")),
        ));
        info.push(Line::from(""));
    }
    info.extend([
        Line::from(Span::styled(
            format!(
                "{} · {} · {}",
                count_label(scenes, "scene", "scenes"),
                count_label(theatres, "theatre", "theatres"),
                count_label(plays, "play", "plays")
            ),
            secondary(app),
        )),
        Line::from(Span::styled(
            if roomy {
                format!(
                    "{} · {} · {} · {} total",
                    count_label(app.devices.len(), "device", "devices"),
                    count_label(app.apps.len(), "app", "apps"),
                    count_label(active, "active job", "active jobs"),
                    count_label(app.jobs.len(), "run", "runs")
                )
            } else {
                format!(
                    "{} · {} · {active} active · {}",
                    count_label(app.devices.len(), "device", "devices"),
                    count_label(app.apps.len(), "app", "apps"),
                    count_label(app.jobs.len(), "run", "runs")
                )
            },
            secondary(app),
        )),
    ]);
    frame.render_widget(
        Paragraph::new(info)
            .block(panel(app, "Workspace", false))
            .wrap(Wrap { trim: false }),
        info_area,
    );

    let mut recent = Vec::new();
    if app.jobs.is_empty() {
        recent.push(Line::from(Span::styled(
            "No runs yet. Try /run --tag smoke --detach.",
            secondary(app),
        )));
    } else {
        let max = usize::from(jobs_area.height.saturating_sub(3)).max(2);
        for record in app.jobs.iter().take(max) {
            recent.push(job_line(
                app,
                record,
                false,
                jobs_area.width.saturating_sub(4),
            ));
        }
    }
    frame.render_widget(
        Paragraph::new(recent).block(panel(app, "Recent runs", false)),
        jobs_area,
    );

    let mut activity = Vec::new();
    let limit = usize::from(right_area.height.saturating_sub(3)).max(3);
    let skip = app.activity.len().saturating_sub(limit);
    for line in app.activity.iter().skip(skip) {
        activity.push(Line::from(Span::styled(
            format!("· {line}"),
            secondary(app),
        )));
    }
    if activity.is_empty() {
        activity.push(Line::from(Span::styled(
            "No activity yet. ? shows the keyboard map.",
            secondary(app),
        )));
    }
    frame.render_widget(
        Paragraph::new(activity)
            .block(panel(app, "Activity", false))
            .wrap(Wrap { trim: false }),
        right_area,
    );
}

fn draw_runs(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
        .spacing(1)
        .split(area);
    let list_area = chunk(&columns, 0, area);
    let detail_area = chunk(&columns, 1, area);

    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{} · {} active",
            count_label(app.jobs.len(), "run", "runs"),
            app.jobs
                .iter()
                .filter(|record| !record.status.terminal())
                .count()
        ),
        secondary(app),
    ))];
    let mut reserved = 2usize;
    if let Some(input) = &app.filter_input {
        lines.push(Line::from(Span::styled(
            format!("filter> {input}_  (Enter apply · Esc cancel)"),
            active_border(app),
        )));
        reserved = reserved.saturating_add(1);
    } else if !app.filter.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("filter: {}  (/ change · empty clear)", app.filter),
            secondary(app),
        )));
        reserved = reserved.saturating_add(1);
    }
    if let Some(device) = &app.device_filter {
        lines.push(Line::from(Span::styled(
            format!("device: {device}  (t cycles)"),
            secondary(app),
        )));
        reserved = reserved.saturating_add(1);
    }
    lines.push(Line::from(""));

    let visible = app.visible_job_indices();
    if app.jobs.is_empty() {
        lines.push(Line::from(Span::styled(
            "No runs yet. Run a play from the prompt.",
            secondary(app),
        )));
    } else if visible.is_empty() {
        lines.push(Line::from(Span::styled(
            "No runs match this filter.",
            secondary(app),
        )));
    } else {
        let max = usize::from(list_area.height.saturating_sub(2))
            .saturating_sub(reserved)
            .max(1);
        let selected = visible
            .iter()
            .position(|index| *index == app.job_index)
            .unwrap_or(0);
        let start = if visible.len() <= max {
            0
        } else {
            selected.saturating_sub(max / 2)
        };
        let end = start.saturating_add(max).min(visible.len());
        for index in visible.iter().skip(start).take(end.saturating_sub(start)) {
            if let Some(record) = app.jobs.get(*index) {
                lines.push(job_line(
                    app,
                    record,
                    *index == app.job_index,
                    list_area.width.saturating_sub(4),
                ));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(
                app,
                "Runs",
                app.focus == Focus::Content && app.screen == Screen::Runs,
            ))
            .wrap(Wrap { trim: false }),
        list_area,
    );

    let detail_chunks = Layout::vertical([Constraint::Percentage(62), Constraint::Percentage(38)])
        .spacing(1)
        .split(detail_area);
    let tree_area = chunk(&detail_chunks, 0, detail_area);
    let log_area = chunk(&detail_chunks, 1, detail_area);
    let record = app.selected_job();
    let title = match record {
        Some(record) => format!("Run · {}", record.status.label()),
        None => "Run".to_string(),
    };
    let mut tree_lines = Vec::new();
    if app.follow.is_none() {
        if let Some(record) = record {
            tree_lines.push(Line::from(Span::styled(
                record.id.clone(),
                title_style(app),
            )));
            tree_lines.push(Line::from(""));
            tree_lines.push(kv_line(app, "status", record.status.label().to_string()));
            tree_lines.push(kv_line(
                app,
                "duration",
                record
                    .duration_ms
                    .map(super::format_duration)
                    .unwrap_or_else(|| "-".to_string()),
            ));
            tree_lines.push(kv_line(app, "devices", record.devices.join(", ")));
            tree_lines.push(kv_line(app, "plays", record.plays.join(", ")));
            tree_lines.push(kv_line(
                app,
                "report",
                relative_path(app, Path::new(&record.run_dir)),
            ));
            if let Some(message) = &record.message {
                tree_lines.push(kv_line(app, "message", message.clone()));
            }
            tree_lines.push(Line::from(""));
            tree_lines.push(Line::from(Span::styled(
                "Enter follows this run and loads its live tree.",
                secondary(app),
            )));
        } else {
            tree_lines.push(Line::from(Span::styled("No run selected.", secondary(app))));
        }
    } else {
        if let Some(command) = Some(app.current_command.as_str()).filter(|text| !text.is_empty()) {
            tree_lines.push(Line::from(Span::styled(
                format!("current  {command}"),
                Style::default().fg(color_or_reset(app, Color::Yellow)),
            )));
            tree_lines.push(Line::from(""));
        }
        let max = usize::from(tree_area.height.saturating_sub(3)).max(3);
        let skip = app
            .tree
            .len()
            .saturating_sub(max.saturating_sub(tree_lines.len()));
        for line in app.tree.iter().skip(skip) {
            if app.line_matches_filter(&line.text) {
                tree_lines.push(tree_line(app, line));
            }
        }
        if app.tree.is_empty() {
            tree_lines.push(Line::from(Span::styled(
                "Waiting for the first event…",
                secondary(app),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(tree_lines)
            .block(panel(app, &title, app.follow.is_some()))
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
        if app.line_matches_filter(line) {
            logs.push(Line::from(line.clone()));
        }
    }
    if logs.is_empty() {
        logs.push(Line::from(Span::styled(
            "Device log lines and step output appear here.",
            secondary(app),
        )));
    }
    let log_title = if app.log_follow {
        "Logs · following (f pauses)"
    } else {
        "Logs · paused (f follows)"
    };
    frame.render_widget(
        Paragraph::new(logs)
            .block(panel(app, log_title, app.log_follow))
            .wrap(Wrap { trim: false }),
        log_area,
    );
}

fn draw_tests(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)])
        .spacing(1)
        .split(area);
    let list_area = chunk(&columns, 0, area);
    let detail_area = chunk(&columns, 1, area);
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{} · {} marked",
                count_label(app.tests.len(), "entry", "entries"),
                app.test_marks.len()
            ),
            secondary(app),
        )),
        Line::from(""),
    ];
    if let Some(error) = &app.test_error {
        lines.push(Line::from(Span::styled(error.clone(), error_style(app))));
    }
    if app.tests.is_empty() && app.test_error.is_none() {
        lines.push(Line::from(Span::styled(
            "No tests found in this workspace.",
            secondary(app),
        )));
    }
    let max = usize::from(list_area.height.saturating_sub(4)).max(2);
    let start = if app.tests.len() <= max {
        0
    } else {
        app.test_index.saturating_sub(max / 2)
    };
    let end = start.saturating_add(max).min(app.tests.len());
    for (index, row) in app
        .tests
        .iter()
        .enumerate()
        .skip(start)
        .take(end.saturating_sub(start))
    {
        lines.push(test_line(
            app,
            row,
            app.test_marks.contains(&index),
            index == app.test_index,
            list_area.width,
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(
                app,
                "Tests",
                app.focus == Focus::Content && app.screen == Screen::Tests,
            ))
            .wrap(Wrap { trim: false }),
        list_area,
    );

    let mut detail = Vec::new();
    if let Some(row) = app.selected_test() {
        detail.push(Line::from(Span::styled(row.name.clone(), title_style(app))));
        detail.push(Line::from(Span::styled(
            test_kind_label(row.kind),
            secondary(app),
        )));
        detail.push(Line::from(""));
        if let Some(file) = &row.file {
            detail.push(kv_line(app, "file", relative_path(app, file)));
        }
        if let Some(line) = row.line {
            detail.push(kv_line(app, "line", line.to_string()));
        }
        if let Some(tags) = tags_of(&row.label) {
            detail.push(kv_line(app, "tags", tags));
        }
        detail.push(Line::from(""));
        detail.push(Line::from(Span::styled("Enter opens it.", secondary(app))));
        detail.push(Line::from(Span::styled(
            "Space mark · r run · d dry-run",
            secondary(app),
        )));
        detail.push(Line::from(""));
        detail.push(Line::from(Span::styled("Selectors", title_style(app))));
        detail.push(Line::from(Span::styled(
            "/tests --tag smoke",
            secondary(app),
        )));
        detail.push(Line::from(Span::styled(
            "/tests --platform android",
            secondary(app),
        )));
    } else {
        detail.push(Line::from(Span::styled(
            "No test selected.",
            secondary(app),
        )));
    }
    frame.render_widget(
        Paragraph::new(detail)
            .block(panel(app, "Selected", false))
            .wrap(Wrap { trim: false }),
        detail_area,
    );
}

fn draw_devices(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} configured", app.devices.len()),
            secondary(app),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{:<24} {:<12} {}", "NAME", "PLATFORM", "TRANSPORT"),
            title_style(app),
        )),
    ];
    if app.devices.is_empty() {
        lines.push(Line::from(Span::styled(
            "No devices configured. Enter opens the devices file.",
            secondary(app),
        )));
    }
    for device in &app.devices {
        lines.push(Line::from(format!(
            "{:<24} {:<12} {}",
            device.name, device.platform, device.connection
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(
                app,
                "Devices",
                app.focus == Focus::Content && app.screen == Screen::Devices,
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_apps(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} configured", app.apps.len()),
            secondary(app),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{:<22} {:<22} {}", "NAME", "PLATFORMS", "DESCRIPTION"),
            title_style(app),
        )),
    ];
    if app.apps.is_empty() {
        lines.push(Line::from(Span::styled(
            "No apps configured. Enter opens impresario.apps.config.",
            secondary(app),
        )));
    }
    for app_row in &app.apps {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<22}", app_row.name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:<22}", app_row.platforms)),
            Span::styled(app_row.description.clone(), secondary(app)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(
                app,
                "Apps",
                app.focus == Focus::Content && app.screen == Screen::Apps,
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_config(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("{} setting(s)", app.config_rows.len()),
            secondary(app),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("{:<24} {:<28} {}", "KEY", "VALUE", "SOURCE"),
            title_style(app),
        )),
    ];
    if app.config_rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "No configuration rows. Enter opens the workspace config.",
            secondary(app),
        )));
    }
    for row in &app.config_rows {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<24}", row.key),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:<28}", row.value)),
            Span::styled(row.source.clone(), secondary(app)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(
                app,
                "Configuration",
                app.focus == Focus::Content && app.screen == Screen::Config,
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    let mut prefix = 0usize;
    let (cursor_line, cursor_column) = if let Some(search) = &app.search {
        let current = search.current().cloned().unwrap_or_default();
        let text = format!("(reverse-i-search)`{}`: {current}", search.query);
        let column = text.chars().count();
        lines.push(Line::from(text));
        (0usize, column)
    } else {
        let text = app.prompt.text();
        let mut all: Vec<String> = text.lines().map(str::to_string).collect();
        if all.is_empty() {
            all.push(String::new());
        }
        let show_from = all.len().saturating_sub(5);
        let mut shown: Vec<String> = all.iter().skip(show_from).cloned().collect();
        if shown.len() == 1 && text.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("❯ ", title_style(app)),
                Span::styled("Type a command, or /help for keys", secondary(app)),
            ]));
            prefix = 2;
        } else {
            if let Some(first) = shown.first_mut() {
                first.insert_str(0, "❯ ");
                prefix = 2;
            }
            for line in &shown {
                lines.push(Line::from(line.clone()));
            }
        }
        let before_cursor: String = app.prompt.chars.iter().take(app.prompt.cursor).collect();
        let line = before_cursor.matches('\n').count();
        let column = before_cursor
            .rsplit('\n')
            .next()
            .map_or(0, |line| line.chars().count());
        (line.saturating_sub(show_from).min(4), column)
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                panel(app, "Command", app.focus == Focus::Prompt).padding(Padding::horizontal(1)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
    if app.prompt.completion_visible && !app.prompt.completions.is_empty() && app.overlay.is_none()
    {
        draw_completions(frame, app, area);
    }
    if app.overlay.is_none() && (app.focus == Focus::Prompt || app.search.is_some()) {
        let x = area
            .x
            .saturating_add(2)
            .saturating_add(u16::try_from(prefix + cursor_column).unwrap_or(0))
            .min(area.right().saturating_sub(2));
        let y = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(cursor_line).unwrap_or(0))
            .min(area.bottom().saturating_sub(1));
        frame.set_cursor_position((x, y));
    }
}

fn draw_completions(frame: &mut Frame, app: &App, prompt: Rect) {
    let available_height = prompt.y.saturating_sub(3).max(3);
    let count = app
        .prompt
        .completions
        .len()
        .min(6)
        .min(usize::from(available_height.saturating_sub(2)));
    if count == 0 {
        return;
    }
    let width = app
        .prompt
        .completions
        .iter()
        .take(count)
        .map(|completion| completion.chars().count())
        .max()
        .unwrap_or(24)
        .saturating_add(8)
        .max("Completions · Tab accepts".chars().count() + 4)
        .min(usize::from(prompt.width).saturating_sub(4))
        .max(24);
    let width = u16::try_from(width).unwrap_or(40);
    let height = u16::try_from(count + 2).unwrap_or(8);
    let x = prompt
        .x
        .saturating_add(2)
        .min(prompt.right().saturating_sub(width));
    let y = prompt.y.saturating_sub(height);
    let popup = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup);
    let mut lines = Vec::new();
    for (index, completion) in app.prompt.completions.iter().take(count).enumerate() {
        let selected = index == app.prompt.completion_index;
        let style = if selected {
            selected_style(app)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{} {completion}", if selected { "›" } else { " " }),
            style,
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, "Completions · Tab accepts", false).padding(Padding::horizontal(1))),
        popup,
    );
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match app.screen {
        Screen::Menu => "↑↓ move · Enter open · Esc workspaces · Tab prompt · ? help",
        Screen::Runs if app.focus == Focus::Content => {
            if area.width < 100 {
                "Enter follow · b detach · x stop · Esc menu · Tab prompt"
            } else {
                "Enter follow · b detach · x stop · f log tail · t device · / filter · Esc menu · Tab prompt"
            }
        }
        Screen::Tests if app.focus == Focus::Content => {
            "Enter edit · Space mark · r run · d dry-run · Esc menu · Tab prompt"
        }
        Screen::Devices | Screen::Apps | Screen::Config if app.focus == Focus::Content => {
            "Enter open · r reload · Esc menu · Tab prompt"
        }
        Screen::Home if app.focus == Focus::Content => "Esc menu · Tab prompt · ? help",
        _ => "Enter run · Tab completes · ↑↓ history · Ctrl-R search · ? help",
    };
    let active = app
        .jobs
        .iter()
        .filter(|record| !record.status.terminal())
        .count();
    let selected = match app.screen {
        Screen::Landing => "choose a workspace".to_string(),
        Screen::Menu => "workspace menu".to_string(),
        Screen::Tests => app.selected_test().map_or_else(
            || "no test selected".to_string(),
            |row| {
                let location = row
                    .file
                    .as_ref()
                    .map(|file| {
                        let line = row.line.map(|line| format!(":{line}")).unwrap_or_default();
                        format!("  {}{line}", relative_path(app, file))
                    })
                    .unwrap_or_default();
                format!(
                    "{} {}  {}{location}",
                    test_kind_icon(app, row.kind),
                    row.name,
                    test_kind_label(row.kind)
                )
            },
        ),
        Screen::Devices => format!(
            "{} configured",
            count_label(app.devices.len(), "device", "devices")
        ),
        Screen::Apps => format!("{} configured", count_label(app.apps.len(), "app", "apps")),
        Screen::Config => format!(
            "{} available",
            count_label(app.config_rows.len(), "setting", "settings")
        ),
        Screen::Home => format!(
            "workspace ready  ·  {}",
            count_label(active, "active job", "active jobs")
        ),
        Screen::Runs => app.selected_job().map_or_else(
            || "no run selected".to_string(),
            |record| {
                let elapsed = app.elapsed_label();
                let elapsed = if elapsed == record.status.label() {
                    String::new()
                } else {
                    format!("  {elapsed}")
                };
                format!(
                    "{} {}  {}{elapsed}",
                    job_icon(app, record),
                    record.id,
                    record.status.label()
                )
            },
        ),
    };
    let message = app
        .activity
        .back()
        .cloned()
        .unwrap_or_else(|| "ready".to_string());
    let lines = vec![
        with_right(
            vec![Span::styled(keys, secondary(app))],
            &format!("{active} active · Ctrl-Q quit "),
            area.width,
            secondary(app),
        ),
        Line::from(Span::styled(selected, title_style(app))),
        Line::from(Span::styled(message, secondary(app))),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_type(border_type(app))
                    .border_style(secondary(app)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_overlay(frame: &mut Frame, app: &App, area: Rect, text: &str) {
    let (title, body) = if let Some(body) = text.strip_prefix("Run plan\n") {
        ("Run plan · Esc closes", body)
    } else if let Some(body) = text.strip_prefix("Doctor\n") {
        ("Doctor · Esc closes", body)
    } else if text.starts_with("Welcome") {
        ("Welcome · Esc skips", text)
    } else {
        ("Help · Esc closes", text)
    };
    let longest = body
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(40);
    let width = u16::try_from(longest.saturating_add(8))
        .unwrap_or(area.width)
        .min(area.width.saturating_sub(4))
        .max(24);
    let line_count = u16::try_from(body.lines().count())
        .unwrap_or(area.height)
        .saturating_add(2);
    let height = line_count.min(area.height.saturating_sub(2)).max(8);
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    let popup = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup);
    let lines: Vec<Line> = body
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, title, true).padding(Padding::horizontal(1)))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn test_line(
    app: &App,
    row: &super::app::TestRow,
    marked: bool,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let mark = if marked {
        if app.ctx.ascii {
            "*"
        } else {
            "●"
        }
    } else {
        " "
    };
    let pointer = if selected {
        if app.ctx.ascii {
            ">"
        } else {
            "›"
        }
    } else {
        " "
    };
    let icon = test_kind_icon(app, row.kind);
    let kind = test_kind_label(row.kind);
    let prefix = format!("{kind} ");
    let label = row.label.strip_prefix(&prefix).unwrap_or(&row.label);
    let left = format!("{mark}{pointer} {}{icon} {label}", "  ".repeat(row.indent));
    let badge = format!(" {kind} ");
    let style = if selected {
        selected_style(app)
    } else {
        Style::default()
    };
    let available = usize::from(width.saturating_sub(4));
    let left = elide(
        &left,
        available.saturating_sub(badge.chars().count()).max(4),
    );
    let pad = available.saturating_sub(left.chars().count() + badge.chars().count());
    Line::from(vec![
        Span::styled(left, style),
        Span::styled(" ".repeat(pad), style),
        Span::styled(badge, if selected { style } else { secondary(app) }),
    ])
}

fn test_kind_label(kind: TestKind) -> &'static str {
    match kind {
        TestKind::Scene => "scene",
        TestKind::Piece => "piece",
        TestKind::Theatre => "theatre",
        TestKind::Stage => "stage",
        TestKind::Play => "play",
        TestKind::Act => "act",
    }
}

fn test_kind_icon(app: &App, kind: TestKind) -> &'static str {
    if app.ctx.ascii {
        return match kind {
            TestKind::Scene => "S",
            TestKind::Piece => "p",
            TestKind::Theatre => "T",
            TestKind::Stage => "s",
            TestKind::Play => "P",
            TestKind::Act => "a",
        };
    }
    match kind {
        TestKind::Scene => "◇",
        TestKind::Piece => "◦",
        TestKind::Theatre => "▣",
        TestKind::Stage => "▸",
        TestKind::Play => "▶",
        TestKind::Act => "•",
    }
}

fn tags_of(label: &str) -> Option<String> {
    let (_, rest) = label.split_once('[')?;
    let (tags, _) = rest.split_once(']')?;
    Some(tags.to_string())
}

fn job_line(
    app: &App,
    record: &crate::jobs::JobRecord,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let icon = job_icon(app, record);
    let badge = record
        .duration_ms
        .map(super::format_duration)
        .or_else(|| {
            record
                .queue_position
                .map(|position| format!("queued #{position}"))
        })
        .unwrap_or_else(|| record.status.label().to_string());
    let available = usize::from(width);
    let id_width = available.saturating_sub(icon.chars().count() + 2 + badge.chars().count() + 2);
    let id = elide(&record.id, id_width.max(8));
    let left = format!("{icon} {id}");
    let style = if selected {
        selected_style(app)
    } else {
        job_status_style(app, record.status)
    };
    let badge_style = if selected {
        selected_style(app)
    } else {
        secondary(app)
    };
    let pad = available.saturating_sub(left.chars().count() + badge.chars().count() + 2);
    if pad == 0 {
        Line::from(Span::styled(left, style))
    } else {
        Line::from(vec![
            Span::styled(left, style),
            Span::styled(" ".repeat(pad), style),
            Span::styled(format!("  {badge}"), badge_style),
        ])
    }
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

fn job_status_style(app: &App, status: crate::jobs::JobStatus) -> Style {
    if !app.ctx.color {
        return Style::default();
    }
    let color = match status {
        crate::jobs::JobStatus::Passed => Color::Green,
        crate::jobs::JobStatus::Failed
        | crate::jobs::JobStatus::Error
        | crate::jobs::JobStatus::Stopped => Color::Red,
        crate::jobs::JobStatus::Interrupted => Color::Magenta,
        crate::jobs::JobStatus::Queued => Color::DarkGray,
        crate::jobs::JobStatus::Running => Color::Cyan,
    };
    Style::default().fg(color)
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
    if app.ctx.color {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
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

fn panel(app: &App, title: &str, active: bool) -> Block<'static> {
    let border = if active {
        active_border(app)
    } else {
        secondary(app)
    };
    let heading_style = if active {
        title_style(app)
    } else {
        secondary(app)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(border_type(app))
        .border_style(border)
        .padding(Padding::horizontal(1))
        .title(Line::from(format!(" {title} ")).style(heading_style))
}

fn border_type(app: &App) -> BorderType {
    if app.ctx.ascii {
        BorderType::Plain
    } else {
        BorderType::Rounded
    }
}

fn kv_line(app: &App, key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<10}"), secondary(app)),
        Span::raw(value),
    ])
}

fn relative_path(app: &App, path: &Path) -> String {
    path.strip_prefix(&app.ctx.base)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn with_right(
    mut left: Vec<Span<'static>>,
    right: &str,
    width: u16,
    right_style: Style,
) -> Line<'static> {
    let left_width: usize = left.iter().map(|span| span.content.chars().count()).sum();
    let needed = left_width + right.chars().count();
    if needed + 4 > usize::from(width) {
        return Line::from(left);
    }
    let pad = usize::from(width).saturating_sub(needed);
    left.push(Span::raw(" ".repeat(pad)));
    left.push(Span::styled(right.to_string(), right_style));
    Line::from(left)
}

fn color_or_reset(app: &App, color: Color) -> Color {
    if app.ctx.color {
        color
    } else {
        Color::Reset
    }
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn elide(value: &str, width: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= width {
        return value.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let left = (width - 3) / 2;
    let right = width - 3 - left;
    let head: String = chars.iter().take(left).collect();
    let tail: String = chars.iter().skip(chars.len() - right).collect();
    format!("{head}...{tail}")
}
