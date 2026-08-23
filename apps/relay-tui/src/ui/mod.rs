pub(crate) mod dashboard;
pub(crate) mod execution;
pub(crate) mod projects;
pub(crate) mod runtime;
pub(crate) mod tasks;

pub(crate) use dashboard::render_dashboard;
pub(crate) use execution::{render_diff, render_execution, render_logs, render_result};
pub(crate) use projects::{render_add_project, render_projects};
pub(crate) use runtime::{render_doctor, render_runtime_detail, render_runtimes, render_settings};
pub(crate) use tasks::{
    render_import_file, render_new_task, render_paste_task, render_task_preview,
    render_task_review, render_tasks,
};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use relay_runtime_antigravity::AntigravityRuntimeHealth;
use relay_runtime_codex::CodexRuntimeHealth;

use crate::input::command_mode_available;
use crate::state::{AppState, Screen};
use crate::theme::*;
use relay_core::TaskState;

pub(crate) fn render(frame: &mut Frame, state: &AppState) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Top bar
            Constraint::Min(3),    // Main content area
            Constraint::Length(1), // Command prompt
            Constraint::Length(1), // Key bar
        ])
        .split(frame.area());

    render_top_bar(frame, state, areas[0]);

    match state.screen {
        Screen::Dashboard => render_dashboard(frame, state, areas[1]),
        Screen::Tasks => render_tasks(frame, state, areas[1]),
        Screen::Projects => render_projects(frame, state, areas[1]),
        Screen::AddProject => render_add_project(frame, state, areas[1]),
        Screen::NewTask => render_new_task(frame, state, areas[1]),
        Screen::ImportFile => render_import_file(frame, state, areas[1]),
        Screen::PasteTask => render_paste_task(frame, state, areas[1]),
        Screen::TaskPreview => render_task_preview(frame, state, areas[1]),
        Screen::TaskReview => render_task_review(frame, state, areas[1]),
        Screen::Execution => render_execution(frame, state, areas[1]),
        Screen::Result => render_result(frame, state, areas[1]),
        Screen::Runtimes => render_runtimes(frame, state, areas[1]),
        Screen::RuntimeDetail => render_runtime_detail(frame, state, areas[1]),
        Screen::Diff => render_diff(frame, state, areas[1]),
        Screen::Logs => render_logs(frame, state, areas[1]),
        Screen::Settings => render_settings(frame, state, areas[1]),
        Screen::Doctor => render_doctor(frame, state, areas[1]),
    }

    render_prompt_line(frame, state, areas[2]);
    render_key_bar(frame, state, areas[3]);

    if state.command_mode {
        render_command_palette_overlay(frame, state, frame.area());
    }
}

fn screen_title(screen: Screen) -> &'static str {
    match screen {
        Screen::Dashboard => "dashboard",
        Screen::Tasks => "tasks",
        Screen::Projects => "projects",
        Screen::AddProject => "add project",
        Screen::NewTask => "new handoff",
        Screen::ImportFile => "import file",
        Screen::PasteTask => "paste task",
        Screen::TaskPreview => "preview",
        Screen::TaskReview => "review",
        Screen::Execution => "execution",
        Screen::Result => "result",
        Screen::Runtimes => "runtimes",
        Screen::RuntimeDetail => "runtime detail",
        Screen::Diff => "diff",
        Screen::Logs => "logs",
        Screen::Settings => "settings",
        Screen::Doctor => "doctor",
    }
}

fn active_provider_health(state: &AppState) -> (&'static str, Style) {
    let theme = Theme::default();
    let checking = matches!(state.runtime_health, CodexRuntimeHealth::Checking)
        || matches!(state.antigravity_health, AntigravityRuntimeHealth::Checking);
    if state.runtime_health.is_ready() || state.antigravity_health.is_ready() {
        (marker_active(), theme.success())
    } else if checking {
        ("…", theme.muted())
    } else {
        (marker_waiting(), theme.error())
    }
}

fn render_top_bar(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let title = screen_title(state.screen);
    let (marker, marker_style) = active_provider_health(state);
    let pad = (area.width as usize).saturating_sub(title.len() + marker.chars().count() + 10);
    let line = Line::from(vec![
        Span::styled("relay", theme.muted()),
        Span::styled(" · ", theme.muted()),
        Span::styled(title, theme.accent()),
        Span::raw(" ".repeat(pad)),
        Span::styled(format!("{} ", state.active_runtime_provider), theme.muted()),
        Span::styled(marker, marker_style),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_prompt_line(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let prompt = if state.command_mode {
        format!("> {}", state.command_input)
    } else {
        "> /_  (type / for commands)".to_owned()
    };
    let style = if state.command_mode {
        theme.text()
    } else {
        theme.muted()
    };
    frame.render_widget(Paragraph::new(prompt).style(style), area);
}

/// Key hints valid for the current screen and state.
///
/// Each table mirrors the match arms in `input.rs` so the bottom bar and the
/// actual key handling cannot drift apart. Returns `(key, label)` pairs.
pub(crate) fn key_hints(state: &AppState) -> Vec<(String, String)> {
    if state.command_mode {
        return vec![
            ("tab".to_owned(), "complete".to_owned()),
            ("enter".to_owned(), "run".to_owned()),
            ("esc".to_owned(), "close".to_owned()),
        ];
    }

    let mut hints: Vec<(String, String)> = match state.screen {
        Screen::Dashboard => vec![
            ("n", "new handoff"),
            ("t", "tasks"),
            ("p", "projects"),
            ("a", "add project"),
            ("q", "quit"),
        ],
        Screen::Tasks => vec![("enter", "open"), ("esc", "dashboard"), ("q", "quit")],
        Screen::Projects => vec![
            ("r", "re-check repos"),
            ("a", "add project"),
            ("esc", "dashboard"),
            ("q", "quit"),
        ],
        Screen::AddProject => vec![("tab", "next field"), ("enter", "save"), ("esc", "cancel")],
        Screen::NewTask => vec![
            ("v", "paste task"),
            ("f", "import file"),
            ("enter", "select"),
            ("esc", "dashboard"),
            ("q", "quit"),
        ],
        Screen::ImportFile => vec![("enter", "import"), ("esc", "back")],
        Screen::PasteTask => vec![
            ("ctrl+enter", "newline"),
            ("enter", "validate"),
            ("esc", "back"),
        ],
        Screen::TaskPreview => vec![("i", "import"), ("c", "cancel"), ("q", "quit")],
        Screen::TaskReview => task_review_hints(state),
        Screen::Execution => execution_hints(state),
        Screen::Result => vec![("t", "tasks"), ("d", "dashboard"), ("n", "new handoff")],
        Screen::Diff | Screen::Logs => vec![("esc", "dashboard")],
        Screen::Runtimes | Screen::Doctor => vec![("r", "re-check"), ("esc", "dashboard")],
        Screen::Settings => {
            if state.settings_form.editing {
                vec![
                    ("enter", "save"),
                    ("esc", "cancel edit"),
                    ("backspace", "delete"),
                ]
            } else {
                vec![
                    ("up/down", "select"),
                    ("enter/←→", "edit"),
                    ("0-9", "set value"),
                    ("r", "re-check"),
                    ("esc", "dashboard"),
                ]
            }
        }
        Screen::RuntimeDetail => {
            if state.runtime_detail_submenu.is_some() {
                vec![("enter", "choose"), ("esc", "close")]
            } else {
                vec![("enter", "select"), ("r", "re-check"), ("esc", "dashboard")]
            }
        }
    }
    .into_iter()
    .map(|(key, label)| (key.to_owned(), label.to_owned()))
    .collect();

    if command_mode_available(state.screen) {
        hints.push(("/".to_owned(), "commands".to_owned()));
    }
    hints
}

/// Mirrors the `Screen::TaskReview` arms in `input.rs`, including submenu state.
fn task_review_hints(state: &AppState) -> Vec<(&'static str, &'static str)> {
    if state.task_review_submenu.is_some() {
        return vec![("enter", "choose"), ("esc", "back")];
    }
    let eligible = state.selected_stored_task().is_some_and(|task| {
        task.state == TaskState::Ready && state.ui_preflight(task) == "eligible"
    });
    if eligible {
        vec![("r", "run"), ("enter", "select"), ("b", "back")]
    } else {
        vec![("enter", "open"), ("b", "back")]
    }
}

/// Mirrors the `Screen::Execution` arms in `input.rs`: permission decisions
/// take priority, then cancel confirmation, then navigation.
fn execution_hints(state: &AppState) -> Vec<(&'static str, &'static str)> {
    if !state.pending_permissions.is_empty() {
        return vec![("y", "allow once"), ("s", "allow this run"), ("n", "deny")];
    }
    if state.cancel_confirmation.is_some() {
        return vec![("y", "confirm cancel"), ("n", "keep running")];
    }
    vec![("c", "cancel"), ("b", "tasks")]
}

fn render_key_bar(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let mut spans = Vec::new();
    for (i, (key, label)) in key_hints(state).iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", theme.muted()));
        }
        spans.push(Span::styled(key.clone(), theme.accent()));
        spans.push(Span::styled(format!(" {label}"), theme.muted()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_command_palette_overlay(frame: &mut Frame, state: &AppState, area: Rect) {
    let suggestions = state.get_current_suggestions();
    if suggestions.is_empty() {
        return;
    }

    let max_visible = 6;
    let visible_count = std::cmp::min(suggestions.len(), max_visible);
    let popup_height = visible_count as u16 + 2;

    if area.height <= 3 {
        return;
    }

    let popup_y = area.height.saturating_sub(popup_height + 2);
    let popup_rect = Rect {
        x: area.x + 2,
        y: popup_y,
        width: area.width.saturating_sub(4).min(90),
        height: popup_height,
    };

    frame.render_widget(ratatui::widgets::Clear, popup_rect);

    let theme = Theme::default();
    let selected_idx = state
        .command_selected_index
        .min(suggestions.len().saturating_sub(1));

    let start_offset = if selected_idx >= max_visible {
        selected_idx.saturating_sub(max_visible - 1)
    } else {
        0
    };

    let mut lines = Vec::new();
    for (i, item) in suggestions
        .iter()
        .skip(start_offset)
        .take(max_visible)
        .enumerate()
    {
        let actual_idx = start_offset + i;
        let is_selected = actual_idx == selected_idx;

        let marker = if is_selected { "> " } else { "  " };
        let mut spans = Vec::new();

        if is_selected {
            spans.push(ratatui::text::Span::styled(marker, theme.selection()));
            spans.push(ratatui::text::Span::styled(
                &item.display_primary,
                theme.accent().add_modifier(ratatui::style::Modifier::BOLD),
            ));
            if let Some(sec) = &item.display_secondary {
                spans.push(ratatui::text::Span::styled(
                    format!(" ({sec})"),
                    theme.muted(),
                ));
            }
            let pad = 30_usize.saturating_sub(item.display_primary.len());
            spans.push(ratatui::text::Span::raw(" ".repeat(pad)));
            spans.push(ratatui::text::Span::styled(&item.description, theme.text()));
            if let Some(reason) = &item.disabled_reason {
                spans.push(ratatui::text::Span::styled(
                    format!(" × {reason}"),
                    theme.warning(),
                ));
            }
            lines.push(ratatui::text::Line::from(spans).style(theme.selection()));
        } else {
            spans.push(ratatui::text::Span::styled(marker, theme.muted()));
            spans.push(ratatui::text::Span::styled(
                &item.display_primary,
                theme.accent(),
            ));
            if let Some(sec) = &item.display_secondary {
                spans.push(ratatui::text::Span::styled(
                    format!(" ({sec})"),
                    theme.muted(),
                ));
            }
            let pad = 30_usize.saturating_sub(item.display_primary.len());
            spans.push(ratatui::text::Span::raw(" ".repeat(pad)));
            spans.push(ratatui::text::Span::styled(
                &item.description,
                theme.muted(),
            ));
            if let Some(reason) = &item.disabled_reason {
                spans.push(ratatui::text::Span::styled(
                    format!(" × {reason}"),
                    theme.muted(),
                ));
            }
            lines.push(ratatui::text::Line::from(spans));
        }
    }

    let popup_block = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme.muted())
            .title(" Commands "),
    );
    frame.render_widget(popup_block, popup_rect);
}
