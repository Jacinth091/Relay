use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use relay_core::{SchedulerTaskStatus, TaskState};

use crate::state::AppState;
use crate::theme::{Theme, marker_waiting};
use crate::util::{distribution_rows, escalated_tasks};

/// Dashboard as calm bordered panels. Wide terminals show all three side by
/// side; narrow terminals stack them in priority order: escalations, then
/// distribution, then active work.
pub(crate) fn render_dashboard(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();

    let (panel_area, feedback_area) = if state.feedback.is_empty() {
        (area, None)
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(1)])
            .split(area);
        (rows[0], Some(rows[1]))
    };

    if state.relay.configured_project_count() == 0 {
        let welcome = Paragraph::new(vec![
            Line::from("Welcome to Relay."),
            Line::from(String::new()),
            Line::from(
                "Relay needs a trusted local Git repository before it can run implementation tasks.",
            ),
        ])
        .wrap(Wrap { trim: false })
        .style(theme.text());
        frame.render_widget(welcome, panel_area);
        render_feedback_line(frame, state, feedback_area, &theme);
        return;
    }

    let escalations = escalated_tasks(state);
    let distribution = distribution_rows(state);

    let columns = if panel_area.width > 100 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 3); 3])
            .split(panel_area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Percentage(25),
                Constraint::Fill(1),
            ])
            .split(panel_area)
    };

    render_escalations_panel(frame, &escalations, columns[0], &theme);
    render_distribution_panel(frame, &distribution, columns[1], &theme);
    render_active_panel(frame, state, &escalations, columns[2], &theme);
    render_feedback_line(frame, state, feedback_area, &theme);
}

fn render_feedback_line(frame: &mut Frame, state: &AppState, area: Option<Rect>, theme: &Theme) {
    if let Some(area) = area {
        frame.render_widget(
            Paragraph::new(state.feedback.clone()).style(theme.muted()),
            area,
        );
    }
}

fn panel_block(title: &'static str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme.muted())
        .title(Span::styled(format!(" {title} "), theme.header()))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let keep = max.saturating_sub(1);
        format!("{}…", text.chars().take(keep).collect::<String>())
    }
}

fn render_escalations_panel(
    frame: &mut Frame,
    escalations: &[&relay_core::StoredTask],
    area: Rect,
    theme: &Theme,
) {
    let block = panel_block("ESCALATIONS", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let mut lines = Vec::new();
    if escalations.is_empty() {
        lines.push(Line::from(Span::styled(
            "No escalations waiting.",
            theme.muted(),
        )));
    } else {
        for task in escalations {
            let (kind, kind_style) = if task.state == TaskState::NeedsClarification {
                ("CLARIFICATION", theme.warning())
            } else {
                ("DECISION", theme.error())
            };
            let id = task.id.to_string();
            let short_id: String = id.chars().take(8).collect();
            lines.push(Line::from(vec![
                Span::styled("! ", theme.error()),
                Span::styled(kind, kind_style),
                Span::raw(" "),
                Span::styled(
                    truncate(&task.handoff.title, width.saturating_sub(14)),
                    theme.text(),
                ),
            ]));
            lines.push(Line::from(Span::styled(
                format!("   {} · {}", task.project_key.as_str(), short_id),
                theme.muted(),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_distribution_panel(frame: &mut Frame, rows: &[String], area: Rect, theme: &Theme) {
    let block = panel_block("DISTRIBUTION", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = if rows.is_empty() {
        vec![Line::from(Span::styled(
            "No approved work yet.",
            theme.muted(),
        ))]
    } else {
        rows.iter()
            .map(|row| match row.split_once(':') {
                Some((label, rest)) => Line::from(vec![
                    Span::styled(label.to_owned(), theme.text()),
                    Span::styled(format!(":{rest}"), theme.muted()),
                ]),
                None => Line::from(row.clone()),
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_active_panel(
    frame: &mut Frame,
    state: &AppState,
    escalations: &[&relay_core::StoredTask],
    area: Rect,
    theme: &Theme,
) {
    let block = panel_block("ACTIVE", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    let running: Vec<_> = state
        .scheduler_snapshot
        .tasks
        .iter()
        .filter(|snapshot| snapshot.status == SchedulerTaskStatus::Running)
        .collect();
    let queued = state
        .scheduler_snapshot
        .tasks
        .iter()
        .filter(|snapshot| {
            matches!(
                snapshot.status,
                SchedulerTaskStatus::QueuedReady
                    | SchedulerTaskStatus::QueuedGlobal
                    | SchedulerTaskStatus::QueuedProject
            )
        })
        .count();

    for snapshot in &running {
        let title = state
            .durable_tasks
            .iter()
            .find(|task| task.id == snapshot.task_id)
            .map(|task| task.handoff.title.as_str())
            .unwrap_or("handoff task");
        let elapsed = state
            .execution_views
            .get(&snapshot.task_id)
            .and_then(|view| view.started_at)
            .map(|started| started.elapsed().as_secs());
        let mut spans = vec![
            Span::styled("● ", theme.success()),
            Span::styled(
                truncate(
                    title,
                    width.saturating_sub(if elapsed.is_some() { 10 } else { 2 }),
                ),
                theme.text(),
            ),
        ];
        if let Some(secs) = elapsed {
            spans.push(Span::styled(format!(" {secs}s"), theme.muted()));
        }
        lines.push(Line::from(spans));
    }

    if queued > 0 {
        lines.push(Line::from(Span::styled(
            format!("{queued} queued"),
            theme.muted(),
        )));
    } else if running.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{} No active tasks.", marker_waiting()),
            theme.muted(),
        )));
    }

    for task in escalations {
        let id = task.id.to_string();
        let short_id: String = id.chars().take(8).collect();
        lines.push(Line::from(vec![
            Span::styled("! ", theme.error()),
            Span::styled(
                truncate(&task.handoff.title, width.saturating_sub(11)),
                theme.text(),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("   {}", short_id),
            theme.muted(),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
