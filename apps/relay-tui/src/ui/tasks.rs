use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use relay_core::TaskState;

use crate::state::{AppState, RunPath, TaskReviewSubmenu, reasoning_effort_label};
use crate::util::{handoff_summary_text, task_preview_text, task_selection_listing};

pub(crate) fn render_tasks(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = task_selection_listing(state);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

pub(crate) fn render_new_task(frame: &mut Frame, state: &AppState, area: Rect) {
    let option_paste = if state.new_task_option == 0 {
        "> Paste task"
    } else {
        "  Paste task"
    };
    let option_file = if state.new_task_option == 1 {
        "> Import task file"
    } else {
        "  Import task file"
    };
    let body = format!(
        "New Task\n\nImport a user-approved Relay handoff.\n\n{}\n{}\n\n{}",
        option_paste, option_file, state.feedback
    );
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

pub(crate) fn render_import_file(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = format!("Path: {}\n\n{}", state.import_file_path, state.feedback);
    let import_file = Paragraph::new(body).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(" Import File ")
            .borders(Borders::ALL),
    );
    frame.render_widget(import_file, area);
}

pub(crate) fn render_paste_task(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = format!(
        "{}\n\n{}",
        if state.paste_text.is_empty() {
            "Paste canonical JSON or Markdown with a Relay task block here.".to_owned()
        } else {
            state.paste_text.clone()
        },
        state.feedback
    );
    let paste = Paragraph::new(body)
        .wrap(Wrap { trim: false })
        .block(Block::default().title(" Paste Task ").borders(Borders::ALL));
    frame.render_widget(paste, area);
}

pub(crate) fn render_task_preview(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state
        .task_candidate
        .as_ref()
        .map(task_preview_text)
        .unwrap_or_else(|| format!("No valid task preview.\n\n{}", state.feedback));
    let preview = Paragraph::new(body).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(" Task Preview ")
            .borders(Borders::ALL),
    );
    frame.render_widget(preview, area);
}

pub(crate) fn render_task_review(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state
        .selected_stored_task()
        .map(|task| {
            let preflight = state.ui_preflight(task);
            let mut action = String::new();
            if task.state == TaskState::Ready && preflight == "eligible" {
                if let Some(submenu) = state.task_review_submenu {
                    match submenu {
                        TaskReviewSubmenu::ModelSelector => {
                            action.push_str("\n\nSelect Model for this run:\n");
                            if let Some(catalog) = &state.cached_model_catalog {
                                for (i, m) in catalog.available_models.iter().enumerate() {
                                    let cursor = if i == state.task_review_menu_index { ">" } else { " " };
                                    action.push_str(&format!("{cursor} {}\n", m.id));
                                }
                            } else {
                                action.push_str("  (No models available or loading...)\n");
                            }
                        }
                        TaskReviewSubmenu::ReasoningSelector => {
                            action.push_str("\n\nSelect Reasoning Effort for this run:\n");
                            let options = state
                                .reasoning_options_for_model(state.one_run_model_override.as_deref());
                            for (i, e) in options.iter().enumerate() {
                                let cursor = if i == state.task_review_menu_index { ">" } else { " " };
                                action.push_str(&format!("{cursor} {}\n", reasoning_effort_label(*e)));
                            }
                        }
                    }
                } else {
                    let menu_items = [
                        "Change model for this run",
                        "Change reasoning for this run",
                        "Run",
                    ];
                    let current_model = state.one_run_model_override.as_deref().unwrap_or("provider default");
                    let current_reasoning = state.one_run_reasoning_override.as_deref().unwrap_or("profile default");

                    action.push_str(&format!("\n\nRuntime: {}\nModel: {}\nReasoning: {}\n\n",
                        state.active_profile_id, current_model, current_reasoning));

                    for (i, item) in menu_items.iter().enumerate() {
                        let cursor = if i == state.task_review_menu_index { ">" } else { " " };
                        action.push_str(&format!("{cursor} {item}\n"));
                    }
                }
            } else if matches!(
                task.state,
                TaskState::Done | TaskState::Failed | TaskState::Cancelled
            ) {
                action = "\n\nView result from here.".to_owned();
            } else if matches!(
                task.state,
                TaskState::NeedsClarification | TaskState::NeedsDecision
            ) {
                action = "\n\nATTENTION REQUIRED: external resolution is required.".to_owned();
            } else {
                action = String::new();
            };
            let requested_stack = task.handoff.stack.as_deref().unwrap_or("not requested");
            let effective_stack = state
                .effective_stack(task)
                .unwrap_or("missing or unsupported");
            let feedback = if state.run_path == RunPath::DaemonV2 && !state.feedback.is_empty() {
                format!("\n\n{}", state.feedback)
            } else {
                String::new()
            };
            format!(
                "Task Review\n\nTask: {}\nProject: {}\nRequested stack: {}\nEffective stack: {}\nRuntime availability: {}\nState: {}\nRepository: {}\n\n{}{}{}",
                task.id,
                task.project_key,
                requested_stack,
                effective_stack,
                if state.active_runtime_provider == "antigravity" {
                    state.antigravity_health.display_status()
                } else {
                    state.runtime_health.display_status()
                },
                task.state.as_str(),
                preflight,
                handoff_summary_text(
                    &task.handoff,
                    task.project_key.as_str(),
                    state
                        .relay
                        .resolve_project(task.project_key.as_str())
                        .ok()
                        .and_then(|p| p.name())
                ),
                action,
                feedback
            )
        })
        .unwrap_or_else(|| "No durable task selected.".to_owned());
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}
