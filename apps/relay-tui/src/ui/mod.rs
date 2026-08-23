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
    widgets::{Block, Borders, Paragraph},
};

use crate::state::{AppState, Screen};
use crate::theme::*;

pub(crate) fn render(frame: &mut Frame, state: &AppState) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Min(3),    // Main content area
            Constraint::Length(2), // Command footer
        ])
        .split(frame.area());

    let runtime_status_marker =
        if state.runtime_health.is_ready() || state.antigravity_health.is_ready() {
            marker_active()
        } else {
            marker_waiting()
        };
    let header_text = format!(
        "Relay                                  {} {} {}",
        state.active_runtime_provider, runtime_status_marker, state.active_profile_id
    );
    let header_widget = Paragraph::new(header_text).style(style_header());
    frame.render_widget(header_widget, areas[0]);

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

    let prompt = if state.command_mode {
        format!("> {}", state.command_input)
    } else {
        "> /_  (type / for commands)".to_owned()
    };
    let footer_text = format!("────────────────────────────────────────\n{prompt}");
    let footer_widget = Paragraph::new(footer_text).style(style_muted());
    frame.render_widget(footer_widget, areas[2]);

    if state.command_mode {
        render_command_palette_overlay(frame, state, frame.area());
    }
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
