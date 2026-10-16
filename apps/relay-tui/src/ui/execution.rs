use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use relay_core::TaskState;
use relay_runtime::sanitize_terminal_output;

use crate::diff;
use crate::state::AppState;
use crate::theme::*;
use crate::util::{repository_inspection_line, scheduler_status_text, visible_scroll};

pub(crate) fn render_diff(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let lines = match state.diff_output.as_deref() {
        Some(diff_text) if !diff_text.is_empty() => {
            let document = diff::parse(diff_text);
            let display = diff::display_lines(&document).join("\n");
            format_diff_lines(&display, &theme)
        }
        _ => vec![ratatui::text::Line::from(
            "No diff available.\n\nRun a task first, then use /diff to view changes.",
        )],
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((state.diff_scroll, 0)),
        area,
    );
}

pub(crate) fn render_logs(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = if let Some(task_id) = state.selected_task {
        state
            .execution_views
            .get(&task_id)
            .map(|view| {
                let header = format!("Logs — Task {}\n\n", task_id);
                let output: String = view.output.iter().cloned().collect::<Vec<_>>().join("");
                if output.is_empty() {
                    format!("{header}No output captured.")
                } else {
                    format!("{header}{}", sanitize_terminal_output(&output))
                }
            })
            .unwrap_or_else(|| "No execution logs available.".to_owned())
    } else {
        "No task selected. Use /logs <task> to view logs.".to_owned()
    };
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .scroll((state.logs_scroll, 0)),
        area,
    );
}

pub(crate) fn render_execution(frame: &mut Frame, state: &AppState, area: Rect) {
    let mut permission_popup = None;

    let body = state.selected_task.and_then(|id| state.execution_views.get(&id).map(|view| (id, view))).map(|(id, view)| {
        let task = state.durable_tasks.iter().find(|task| task.id == id);
        let output = view.output.iter().cloned().collect::<Vec<_>>().join("");

        if let Some((_, request)) = state
            .pending_permissions
            .iter()
            .find(|(pending_task, _)| pending_task == &id)
        {
            permission_popup = Some(request.clone());
        }

        format!("Task: {id}\nProject: {}\nRuntime: {}\nModel: {}\nReasoning: {}\nHandoff status: {}\nPhase: {}\nRepair: {}\nElapsed: {}s\n{}\n\nOutput:\n{}",
            task.map(|task| task.project_key.as_str()).unwrap_or("unknown"),
            view.frozen_profile.as_deref().map(|p| state.runtime_label_for_profile(p)).unwrap_or_else(|| state.runtime_label()),
            view.frozen_model.as_deref().unwrap_or("provider default"),
            view.frozen_reasoning.as_deref().unwrap_or("profile default"),
            view.scheduler_status.as_ref().map(scheduler_status_text).unwrap_or("not queued"),
            view.durable_state.map(|state| state.as_str()).unwrap_or("state pending"), view.repair_attempt,
            view.started_at.map(|started| started.elapsed().as_secs()).unwrap_or(0),
            view.persistence_error.as_ref().map(|error| format!("STATE UNCERTAIN / DURABILITY FAILURE\n{error}")).unwrap_or_else(|| if view.cancellation_requested { "Cancellation requested...".to_owned() } else { String::new() }),
            output)
    }).unwrap_or_else(|| "No execution selected.".to_owned());
    let body = if !state.execution_follow_tail {
        format!("{body}\n\n── paused · End to resume live ──")
    } else {
        body
    };
    let scroll = state
        .execution_scroll
        .min(visible_scroll(&body, area.height));
    frame.render_widget(
        Paragraph::new(body)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );

    if let Some(request) = permission_popup {
        let kind = format!("{:?}", request.kind);
        let command = request
            .command
            .as_deref()
            .map(|c| format!("Command: {c}\n"))
            .unwrap_or_default();
        let cwd = request
            .cwd
            .as_ref()
            .map(|c| format!("cwd: {}\n", c.display()))
            .unwrap_or_default();
        let paths = if request.requested.paths.is_empty() {
            String::new()
        } else {
            let mut paths_str = "Paths:\n".to_owned();
            for path in &request.requested.paths {
                paths_str.push_str(&format!("  {}\n", path.display()));
            }
            paths_str
        };
        let reason = request
            .reason
            .as_deref()
            .unwrap_or("provider approval required");
        let popup_text = format!("Kind: {kind}\n{command}{cwd}{paths}Reason: {reason}");

        let popup_width = popup_text
            .lines()
            .map(|l| l.len())
            .max()
            .unwrap_or(50)
            .max(50)
            .min(area.width.saturating_sub(4) as usize) as u16
            + 4;
        let popup_height = popup_text.lines().count() as u16 + 2;

        let popup_x = area.x + (area.width.saturating_sub(popup_width) / 2);
        let popup_y = area.y + (area.height.saturating_sub(popup_height) / 2);

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        frame.render_widget(ratatui::widgets::Clear, popup_area);
        frame.render_widget(
            Paragraph::new(popup_text).block(
                Block::default()
                    .title(" Permission Required ")
                    .borders(Borders::ALL),
            ),
            popup_area,
        );
    }
}

pub(crate) fn render_result(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state.selected_stored_task().map(|task| {
        let report = state.last_reports.get(&task.id);
        let repository = state
            .repository_inspections
            .get(&task.project_key)
            .map(|i| repository_inspection_line(&i.eligibility, &i.status))
            .unwrap_or_else(|| "unavailable".to_owned());

        let task_id = &task.id;
        let project = &task.project_key;
        let runtime = state.execution_profile(task.id).map(|p| state.runtime_label_for_profile(&p)).unwrap_or_else(|| state.runtime_label());
        let model = state.execution_model(task.id).as_deref().unwrap_or("provider default").to_owned();
        let reasoning = state.execution_reasoning(task.id).as_deref().unwrap_or("profile default").to_owned();
        let task_state = task.state.as_str();
        let outcome = report.and_then(|r| r.outcome).map(|o| o.as_str()).unwrap_or("not confirmed");
        let verification = match task.state {
            TaskState::Done => "PASS",
            TaskState::NeedsDecision => "NEEDS DECISION",
            TaskState::NeedsClarification => "NEEDS CLARIFICATION",
            TaskState::Failed => "FAILED",
            TaskState::Cancelled => "CANCELLED",
            _ => "not available",
        };
        let repairs = state.execution_views.get(&task.id).map(|v| v.repair_attempt).unwrap_or(0);
        let summary = report.and_then(|r| r.outcome_summary.as_deref()).unwrap_or("No confirmed execution report.");

        format!("Task: {task_id}\nProject: {project}\nRuntime: {runtime}\nModel: {model}\nReasoning: {reasoning}\nState: {task_state}\nOutcome: {outcome}\nVerification: {verification}\nRepairs: {repairs}\nRepository: {repository}\n\nSummary:\n{summary}")
    }).unwrap_or_else(|| "No durable task selected.".to_owned());
    let scroll = state.result_scroll.min(visible_scroll(&body, area.height));
    frame.render_widget(
        Paragraph::new(body)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}
