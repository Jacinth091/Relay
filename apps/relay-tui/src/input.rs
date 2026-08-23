use std::error::Error;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use relay_core::TaskState;
use relay_runtime::RuntimePermissionDecision;

use crate::commands::execute_relay_command;
use crate::state::{
    AppState, RuntimeDetailSubmenu, Screen, TaskReviewSubmenu, reasoning_effort_label,
};

pub(crate) async fn handle_key(
    state: &mut AppState,
    key: KeyEvent,
) -> Result<bool, Box<dyn Error>> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }
    if state.command_mode {
        let suggs = state.get_current_suggestions();
        let sugg_count = suggs.len();

        match key.code {
            KeyCode::Up => {
                if sugg_count > 0 {
                    if state.command_selected_index == 0 {
                        state.command_selected_index = sugg_count - 1;
                    } else {
                        state.command_selected_index -= 1;
                    }
                }
                return Ok(false);
            }
            KeyCode::Down => {
                if sugg_count > 0 {
                    state.command_selected_index = (state.command_selected_index + 1) % sugg_count;
                }
                return Ok(false);
            }
            KeyCode::Tab => {
                if !suggs.is_empty() {
                    let idx = state.command_selected_index.min(sugg_count - 1);
                    state.command_input = suggs[idx].completion.clone();
                    state.command_selected_index = 0;
                }
                return Ok(false);
            }
            KeyCode::Enter => {
                let typed = state.command_input.trim().to_owned();
                let typed_match = if typed.is_empty() {
                    None
                } else {
                    suggs.iter().find(|sugg| {
                        sugg.executable && sugg.completion.eq_ignore_ascii_case(&typed)
                    })
                };
                if let Some(matched) = typed_match {
                    // Execute the command the user fully typed (canonicalized),
                    // never a different highlighted suggestion.
                    let command = matched.completion.clone();
                    state.command_mode = false;
                    state.command_selected_index = 0;
                    return execute_relay_command(state, &command).await;
                }
                if !suggs.is_empty() {
                    let idx = state.command_selected_index.min(sugg_count - 1);
                    let sel = &suggs[idx];
                    if sel.executable {
                        let command = sel.completion.clone();
                        state.command_mode = false;
                        state.command_input.clear();
                        state.command_selected_index = 0;
                        return execute_relay_command(state, &command).await;
                    } else {
                        state.command_input = sel.completion.clone();
                        state.command_selected_index = 0;
                        return Ok(false);
                    }
                } else {
                    let command = std::mem::take(&mut state.command_input);
                    state.command_mode = false;
                    state.command_selected_index = 0;
                    return execute_relay_command(state, &command).await;
                }
            }
            KeyCode::Esc => {
                state.command_mode = false;
                state.command_input.clear();
                state.command_selected_index = 0;
                return Ok(false);
            }
            KeyCode::Backspace => {
                state.command_input.pop();
                state.command_selected_index = 0;
                return Ok(false);
            }
            KeyCode::Char(character) if !character.is_control() => {
                state.command_input.push(character);
                state.command_selected_index = 0;
                return Ok(false);
            }
            _ => return Ok(false),
        }
    }
    if key.code == KeyCode::Char('/') && command_mode_available(state.screen) {
        state.command_mode = true;
        state.command_input = "/".to_owned();
        state.command_selected_index = 0;
        return Ok(false);
    }
    match state.screen {
        Screen::Dashboard => match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => Ok(true),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                state.start_new_task();
                Ok(false)
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                state.enter_projects();
                Ok(false)
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                state.enter_tasks();
                Ok(false)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                state.try_start_add_project();
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Tasks => match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => Ok(true),
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            KeyCode::Up => {
                state.select_previous_task();
                Ok(false)
            }
            KeyCode::Down => {
                state.select_next_task();
                Ok(false)
            }
            KeyCode::Enter => {
                state.open_selected_task();
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Projects => match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => Ok(true),
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                state.refresh_repository_inspections();
                Ok(false)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                state.try_start_add_project();
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::AddProject => {
            match key.code {
                KeyCode::Esc => state.screen = Screen::Projects,
                KeyCode::Tab | KeyCode::Down => state.add_field = state.add_field.next(),
                KeyCode::BackTab | KeyCode::Up => state.add_field = state.add_field.previous(),
                KeyCode::Enter => state.submit_add_project()?,
                KeyCode::Backspace => {
                    state.active_add_field_mut().pop();
                }
                KeyCode::Char(char) if !char.is_control() => {
                    state.active_add_field_mut().push(char)
                }
                _ => {}
            }
            Ok(false)
        }
        Screen::NewTask => match key.code {
            KeyCode::Up => {
                state.new_task_option = state.new_task_option.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                if state.new_task_option < 1 {
                    state.new_task_option += 1;
                }
                Ok(false)
            }
            KeyCode::Enter => {
                if state.new_task_option == 0 {
                    state.start_paste_task();
                } else {
                    state.start_import_file();
                }
                Ok(false)
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                state.start_import_file();
                Ok(false)
            }
            KeyCode::Char('v') | KeyCode::Char('V') => {
                state.start_paste_task();
                Ok(false)
            }
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => Ok(true),
            _ => Ok(false),
        },
        Screen::ImportFile => {
            match key.code {
                KeyCode::Esc => state.screen = Screen::NewTask,
                KeyCode::Enter => state.submit_import_file(),
                KeyCode::Backspace => {
                    state.import_file_path.pop();
                }
                KeyCode::Char(char) if !char.is_control() => state.import_file_path.push(char),
                _ => {}
            }
            Ok(false)
        }
        Screen::PasteTask => {
            match key.code {
                KeyCode::Esc => state.screen = Screen::NewTask,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.paste_text.push('\n')
                }
                KeyCode::Enter => state.submit_paste_task(),
                KeyCode::Backspace => {
                    state.paste_text.pop();
                }
                KeyCode::Char(char) if !char.is_control() => state.paste_text.push(char),
                _ => {}
            }
            Ok(false)
        }
        Screen::TaskPreview => match key.code {
            KeyCode::Char('i') | KeyCode::Char('I') | KeyCode::Enter => {
                state.confirm_task_import().await;
                Ok(false)
            }
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                state.cancel_task_import();
                Ok(false)
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => Ok(true),
            _ => Ok(false),
        },
        Screen::TaskReview => match key.code {
            KeyCode::Esc | KeyCode::Char('b') | KeyCode::Char('B') => {
                if state.task_review_submenu.is_some() {
                    state.task_review_submenu = None;
                    state.task_review_menu_index = 0;
                    Ok(false)
                } else {
                    state.screen = Screen::Tasks;
                    Ok(false)
                }
            }
            KeyCode::Up => {
                state.task_review_menu_index = state.task_review_menu_index.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                let max = match state.task_review_submenu {
                    Some(TaskReviewSubmenu::ModelSelector) => state
                        .cached_model_catalog
                        .as_ref()
                        .map(|c| c.available_models.len().saturating_sub(1))
                        .unwrap_or(0),
                    Some(TaskReviewSubmenu::ReasoningSelector) => {
                        let options = state
                            .reasoning_options_for_model(state.one_run_model_override.as_deref());
                        options.len().saturating_sub(1)
                    }
                    None => 2, // 3 items (0 to 2)
                };
                if state.task_review_menu_index < max {
                    state.task_review_menu_index += 1;
                }
                Ok(false)
            }
            KeyCode::Enter => {
                if let Some(task) = state.selected_stored_task() {
                    let preflight = state.ui_preflight(task);
                    if task.state == TaskState::Ready && preflight == "eligible" {
                        match state.task_review_submenu {
                            None => match state.task_review_menu_index {
                                0 => {
                                    state.task_review_submenu =
                                        Some(TaskReviewSubmenu::ModelSelector);
                                    state.task_review_menu_index = 0;
                                }
                                1 => {
                                    state.task_review_submenu =
                                        Some(TaskReviewSubmenu::ReasoningSelector);
                                    state.task_review_menu_index = 0;
                                }
                                2 => {
                                    state.submit_selected_task().await;
                                }
                                _ => {}
                            },
                            Some(TaskReviewSubmenu::ModelSelector) => {
                                if let Some(catalog) = &state.cached_model_catalog {
                                    if let Some(model) =
                                        catalog.available_models.get(state.task_review_menu_index)
                                    {
                                        state.one_run_model_override = Some(model.id.clone());
                                    }
                                }
                                state.task_review_submenu = None;
                                state.task_review_menu_index = 0;
                            }
                            Some(TaskReviewSubmenu::ReasoningSelector) => {
                                let options = state.reasoning_options_for_model(
                                    state.one_run_model_override.as_deref(),
                                );
                                if let Some(effort) = options.get(state.task_review_menu_index) {
                                    state.one_run_reasoning_override =
                                        Some(reasoning_effort_label(*effort).to_owned());
                                }
                                state.task_review_submenu = None;
                                state.task_review_menu_index = 0;
                            }
                        }
                        return Ok(false);
                    }
                }
                state.open_selected_task();
                Ok(false)
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if state.task_review_submenu.is_none() {
                    state.submit_selected_task().await;
                }
                Ok(false)
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                state.start_new_task();
                Ok(false)
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => Ok(true),
            _ => Ok(false),
        },
        Screen::Execution => match key.code {
            KeyCode::Char('y')
            | KeyCode::Char('Y')
            | KeyCode::Char('s')
            | KeyCode::Char('S')
            | KeyCode::Char('n')
            | KeyCode::Char('N')
            | KeyCode::Esc
                if !state.pending_permissions.is_empty() =>
            {
                if let Some((task_id, request)) = (!state.pending_permissions.is_empty())
                    .then(|| state.pending_permissions.remove(0))
                {
                    let decision = match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            RuntimePermissionDecision::AllowOnce
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            RuntimePermissionDecision::AllowForRun
                        }
                        _ => RuntimePermissionDecision::Deny,
                    };
                    let scope = match decision {
                        RuntimePermissionDecision::AllowOnce => {
                            relay_runtime::RuntimePermissionScope::Once
                        }
                        RuntimePermissionDecision::AllowForRun => {
                            relay_runtime::RuntimePermissionScope::Session
                        }
                        _ => relay_runtime::RuntimePermissionScope::Once,
                    };
                    let grant = relay_runtime::RuntimePermissionGrant {
                        decision,
                        scope,
                        granted_paths: request.requested.paths.clone(),
                    };
                    if let Some(scheduler) = state.scheduler.clone() {
                        match scheduler
                            .resolve_permission(task_id, request.request_id.clone(), grant)
                            .await
                        {
                            Ok(()) => {}
                            Err(error) => {
                                state.feedback =
                                    format!("Failed to deliver permission decision: {error}");
                                state.pending_permissions.push((task_id, request));
                            }
                        }
                    } else {
                        state.feedback =
                            "No active execution; permission decision was not delivered."
                                .to_owned();
                        state.pending_permissions.push((task_id, request));
                    }
                }
                Ok(false)
            }
            KeyCode::Up => {
                state.execution_follow_tail = false;
                state.execution_scroll = state.execution_scroll.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                state.execution_scroll = state.execution_scroll.saturating_add(1);
                Ok(false)
            }
            KeyCode::PageUp => {
                state.execution_follow_tail = false;
                state.execution_scroll = state.execution_scroll.saturating_sub(20);
                Ok(false)
            }
            KeyCode::PageDown => {
                state.execution_scroll = state.execution_scroll.saturating_add(20);
                Ok(false)
            }
            KeyCode::Home => {
                state.execution_follow_tail = false;
                state.execution_scroll = 0;
                Ok(false)
            }
            KeyCode::End => {
                state.execution_follow_tail = true;
                state.execution_scroll = u16::MAX;
                Ok(false)
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                state.cancel_confirmation = state.selected_task;
                state.feedback = "Cancel execution? Y: confirm  N / Esc: keep running".to_owned();
                Ok(false)
            }
            KeyCode::Char('y') | KeyCode::Char('Y') if state.cancel_confirmation.is_some() => {
                state.confirm_cancel().await;
                Ok(false)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc
                if state.cancel_confirmation.is_some() =>
            {
                state.cancel_confirmation = None;
                state.feedback = "Cancellation not requested.".to_owned();
                Ok(false)
            }
            KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Esc => {
                state.screen = Screen::Tasks;
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Result => match key.code {
            KeyCode::Up => {
                state.result_scroll = state.result_scroll.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                state.result_scroll = state.result_scroll.saturating_add(1);
                Ok(false)
            }
            KeyCode::PageUp => {
                state.result_scroll = state.result_scroll.saturating_sub(20);
                Ok(false)
            }
            KeyCode::PageDown => {
                state.result_scroll = state.result_scroll.saturating_add(20);
                Ok(false)
            }
            KeyCode::Home => {
                state.result_scroll = 0;
                Ok(false)
            }
            KeyCode::End => {
                state.result_scroll = u16::MAX;
                Ok(false)
            }
            KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Esc => {
                state.screen = Screen::Tasks;
                Ok(false)
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                state.start_new_task();
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Diff => match key.code {
            KeyCode::Up => {
                state.diff_scroll = state.diff_scroll.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                state.diff_scroll = state.diff_scroll.saturating_add(1);
                Ok(false)
            }
            KeyCode::PageUp => {
                state.diff_scroll = state.diff_scroll.saturating_sub(20);
                Ok(false)
            }
            KeyCode::PageDown => {
                state.diff_scroll = state.diff_scroll.saturating_add(20);
                Ok(false)
            }
            KeyCode::Home => {
                state.diff_scroll = 0;
                Ok(false)
            }
            KeyCode::End => {
                state.diff_scroll = u16::MAX;
                Ok(false)
            }
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Logs => match key.code {
            KeyCode::Up => {
                state.logs_scroll = state.logs_scroll.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                state.logs_scroll = state.logs_scroll.saturating_add(1);
                Ok(false)
            }
            KeyCode::PageUp => {
                state.logs_scroll = state.logs_scroll.saturating_sub(20);
                Ok(false)
            }
            KeyCode::PageDown => {
                state.logs_scroll = state.logs_scroll.saturating_add(20);
                Ok(false)
            }
            KeyCode::Home => {
                state.logs_scroll = 0;
                Ok(false)
            }
            KeyCode::End => {
                state.logs_scroll = u16::MAX;
                Ok(false)
            }
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Runtimes | Screen::Doctor => match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => {
                state.begin_health_refresh(true);
                Ok(false)
            }
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Settings => {
            let editing = state.settings_form.editing;
            match key.code {
                KeyCode::Esc => {
                    if editing {
                        state.cancel_settings_edit();
                    } else {
                        state.screen = Screen::Dashboard;
                    }
                    Ok(false)
                }
                // Browse mode: select a field, then start editing it.
                KeyCode::Up if !editing => {
                    state.settings_form.select_previous();
                    Ok(false)
                }
                KeyCode::Down if !editing => {
                    state.settings_form.select_next();
                    Ok(false)
                }
                KeyCode::Enter | KeyCode::Left | KeyCode::Right if !editing => {
                    state.begin_settings_edit(None);
                    Ok(false)
                }
                KeyCode::Char(digit) if digit.is_ascii_digit() && !editing => {
                    state.begin_settings_edit(Some(digit));
                    Ok(false)
                }
                KeyCode::Char('r') | KeyCode::Char('R') if !editing => {
                    state.begin_health_refresh(true);
                    Ok(false)
                }
                // Edit mode: digits and backspace shape the buffer; Enter commits.
                KeyCode::Enter if editing => {
                    state.commit_settings_edit();
                    Ok(false)
                }
                KeyCode::Backspace if editing => {
                    state.settings_form.buffer.pop();
                    Ok(false)
                }
                KeyCode::Char(digit) if digit.is_ascii_digit() && editing => {
                    state.settings_form.buffer.push(digit);
                    Ok(false)
                }
                _ => Ok(false),
            }
        }
        Screen::RuntimeDetail => match key.code {
            KeyCode::Esc => {
                if state.runtime_detail_submenu.is_some() {
                    state.runtime_detail_submenu = None;
                    state.runtime_detail_menu_index = 0;
                    Ok(false)
                } else {
                    state.screen = Screen::Dashboard;
                    Ok(false)
                }
            }
            KeyCode::Up => {
                state.runtime_detail_menu_index = state.runtime_detail_menu_index.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                let max = match state.runtime_detail_submenu {
                    Some(RuntimeDetailSubmenu::ModelSelector) => state
                        .cached_model_catalog
                        .as_ref()
                        .map(|c| c.available_models.len().saturating_sub(1))
                        .unwrap_or(0),
                    Some(RuntimeDetailSubmenu::ReasoningSelector) => {
                        let options = state.reasoning_options_for_model(None);
                        options.len().saturating_sub(1)
                    }
                    Some(RuntimeDetailSubmenu::ProfileSelector) => {
                        let provider = state.runtime_detail_provider.as_deref().unwrap_or("codex");
                        AppState::provider_profiles(provider)
                            .len()
                            .saturating_sub(1)
                    }
                    None => 3, // 4 items (0 to 3)
                };
                if state.runtime_detail_menu_index < max {
                    state.runtime_detail_menu_index += 1;
                }
                Ok(false)
            }
            KeyCode::Enter => {
                match state.runtime_detail_submenu {
                    None => match state.runtime_detail_menu_index {
                        0 => {
                            state.runtime_detail_submenu =
                                Some(RuntimeDetailSubmenu::ModelSelector);
                            state.runtime_detail_menu_index = 0;
                        }
                        1 => {
                            state.runtime_detail_submenu =
                                Some(RuntimeDetailSubmenu::ReasoningSelector);
                            state.runtime_detail_menu_index = 0;
                        }
                        2 => {
                            state.runtime_detail_submenu =
                                Some(RuntimeDetailSubmenu::ProfileSelector);
                            state.runtime_detail_menu_index = 0;
                        }
                        3 => {
                            state.begin_health_refresh(true);
                        }
                        _ => {}
                    },
                    Some(RuntimeDetailSubmenu::ModelSelector) => {
                        if let Some(catalog) = &state.cached_model_catalog {
                            if let Some(model) = catalog
                                .available_models
                                .get(state.runtime_detail_menu_index)
                            {
                                state.active_model =
                                    relay_core::RuntimeModelSelection::Explicit(model.id.clone());
                            }
                        }
                        state.runtime_detail_submenu = None;
                        state.runtime_detail_menu_index = 0;
                    }
                    Some(RuntimeDetailSubmenu::ReasoningSelector) => {
                        let options = state.reasoning_options_for_model(None);
                        if let Some(effort) = options.get(state.runtime_detail_menu_index) {
                            state.active_reasoning_effort =
                                relay_core::RuntimeReasoningSelection::Explicit(*effort);
                        }
                        state.runtime_detail_submenu = None;
                        state.runtime_detail_menu_index = 0;
                    }
                    Some(RuntimeDetailSubmenu::ProfileSelector) => {
                        // Applying goes through the same validation path as
                        // `/runtime use` (provider lookup, profile resolution,
                        // cross-provider tuning re-resolution, persistence).
                        let provider = state
                            .runtime_detail_provider
                            .clone()
                            .unwrap_or_else(|| "codex".to_owned());
                        let selected_profile = AppState::provider_profiles(&provider)
                            .get(state.runtime_detail_menu_index)
                            .map(|profile| profile.id.clone());
                        state.runtime_detail_submenu = None;
                        state.runtime_detail_menu_index = 0;
                        if let Some(profile) = selected_profile {
                            state.apply_runtime_profile(&profile);
                        }
                    }
                }
                Ok(false)
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                state.begin_health_refresh(true);
                Ok(false)
            }
            KeyCode::Char('d') | KeyCode::Char('D') if state.runtime_detail_submenu.is_none() => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            _ => Ok(false),
        },
    }
}

pub(crate) fn command_mode_available(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::Dashboard
            | Screen::Tasks
            | Screen::Projects
            | Screen::TaskReview
            | Screen::Execution
            | Screen::Result
            | Screen::Runtimes
            | Screen::RuntimeDetail
            | Screen::Diff
            | Screen::Logs
            | Screen::Settings
            | Screen::Doctor
    )
}
