use std::error::Error;

use tokio::process::Command as TokioCommand;

use command::{RelayCommand, parse_relay_command};

use crate::command;
use crate::state::{AppState, Screen, build_runtime_router};

pub(crate) async fn execute_relay_command(
    state: &mut AppState,
    input: &str,
) -> Result<bool, Box<dyn Error>> {
    let command = match parse_relay_command(input) {
        Ok(command) => command,
        Err(error) => {
            state.feedback = error;
            return Ok(false);
        }
    };
    match command {
        RelayCommand::Home => state.screen = Screen::Dashboard,
        RelayCommand::New => state.start_new_task(),
        RelayCommand::Tasks => state.enter_tasks(),
        RelayCommand::Projects => state.enter_projects(),
        RelayCommand::ProjectAdd { path } => state.start_detected_project(path),
        RelayCommand::Settings => state.screen = Screen::Settings,
        RelayCommand::Doctor => {
            state.screen = Screen::Doctor;
            state.begin_health_refresh(true);
        }
        RelayCommand::Updates => {
            state.screen = Screen::Updates;
            state.begin_health_refresh(true);
        }
        RelayCommand::Help => {
            state.screen = Screen::Dashboard;
            state.feedback = command::COMMANDS.join("\n");
        }
        RelayCommand::Quit => return Ok(true),
        RelayCommand::Run {
            task,
            model,
            reasoning,
        } => {
            state.one_run_model_override = model;
            state.one_run_reasoning_override = reasoning;
            state.run_task_selector(&task).await;
        }
        RelayCommand::Cancel { task } => state.cancel_task_selector(&task),
        RelayCommand::Runtimes => {
            state.screen = Screen::Runtimes;
            state.begin_health_refresh(true);
        }
        RelayCommand::RuntimeDetail { provider } => {
            state.runtime_detail_provider = Some(provider.clone());
            state.screen = Screen::RuntimeDetail;
            state.begin_health_refresh(true);
        }
        RelayCommand::RuntimeUse { profile } => state.apply_runtime_profile(&profile),
        RelayCommand::RuntimeModel { model } => {
            let provider = state
                .active_profile_id
                .split('/')
                .next()
                .unwrap_or_default();
            if let Some(integration) = build_runtime_router().integration(provider) {
                let catalog = integration.models().await;
                if !catalog.available_models.is_empty()
                    && !catalog
                        .available_models
                        .iter()
                        .any(|candidate| candidate.id == model)
                {
                    state.feedback =
                        format!("Model '{model}' is not available from provider '{provider}'.");
                    return Ok(false);
                }
            }
            let entry = state
                .runtime_settings
                .profile_overrides
                .entry(state.active_profile_id.clone())
                .or_default();
            entry.model = Some(model.clone());
            state.active_model = relay_core::RuntimeModelSelection::Explicit(model.clone());
            state.feedback = match state
                .runtime_settings
                .save_to_path(&state.runtime_config_path)
            {
                Ok(()) => format!("Model selected for {}: {model}", state.active_profile_id),
                Err(error) => {
                    format!("Model selected in memory but could not persist settings: {error}")
                }
            };
        }
        RelayCommand::RuntimeReasoning { effort } => {
            let parsed = match effort.to_ascii_lowercase().as_str() {
                "low" => Some(relay_core::ReasoningEffort::Low),
                "medium" => Some(relay_core::ReasoningEffort::Medium),
                "high" => Some(relay_core::ReasoningEffort::High),
                "xhigh" => Some(relay_core::ReasoningEffort::XHigh),
                _ => None,
            };
            let Some(parsed) = parsed else {
                state.feedback = "Reasoning must be low, medium, high, or xhigh.".to_owned();
                return Ok(false);
            };
            let provider = state
                .active_profile_id
                .split('/')
                .next()
                .unwrap_or_default();
            let selected_model = state.active_model.as_option().unwrap_or("default");
            if let Some(integration) = build_runtime_router().integration(provider) {
                let catalog = integration.models().await;
                let effective_model = if selected_model == "default" {
                    catalog.default_model.as_str()
                } else {
                    selected_model
                };
                if let Some(model_info) = catalog
                    .available_models
                    .iter()
                    .find(|model| model.id == effective_model)
                {
                    if !model_info.supported_reasoning_efforts.is_empty()
                        && !model_info.supported_reasoning_efforts.contains(&parsed)
                    {
                        state.feedback = format!(
                            "Reasoning '{effort}' is not supported by model '{effective_model}'."
                        );
                        return Ok(false);
                    }
                }
            }
            let entry = state
                .runtime_settings
                .profile_overrides
                .entry(state.active_profile_id.clone())
                .or_default();
            entry.reasoning_effort = Some(effort.to_ascii_lowercase());
            state.active_reasoning_effort = relay_core::RuntimeReasoningSelection::Explicit(parsed);
            state.feedback = match state
                .runtime_settings
                .save_to_path(&state.runtime_config_path)
            {
                Ok(()) => format!(
                    "Reasoning selected for {}: {effort}",
                    state.active_profile_id
                ),
                Err(error) => {
                    format!("Reasoning selected in memory but could not persist settings: {error}")
                }
            };
        }
        RelayCommand::RuntimeTest { provider } => state.begin_provider_health_refresh(provider),
        RelayCommand::Diff { task } => {
            if let Some(t) = task {
                state.select_task_by_selector(&t);
            }
            let diff = match state.selected_stored_task() {
                Some(task) => match state.relay.resolve_project(task.project_key.as_str()) {
                    Ok(project) => {
                        let mut command = TokioCommand::new("git");
                        #[cfg(windows)]
                        command.creation_flags(0x08000000);
                        match command
                            .args(["diff", "--no-ext-diff", "--no-color", "--unified=3"])
                            .current_dir(project.local_path())
                            .kill_on_drop(true)
                            .output()
                            .await
                        {
                            Ok(output) => {
                                let out = String::from_utf8_lossy(&output.stdout).to_string();
                                if out.is_empty() {
                                    "No changes detected.".to_owned()
                                } else {
                                    out
                                }
                            }
                            Err(e) => format!("Failed to run git diff: {}", e),
                        }
                    }
                    Err(e) => format!("Project resolution failed: {}", e),
                },
                None => "No task selected.".to_owned(),
            };
            state.diff_output = Some(diff);
            state.diff_scroll = 0;
            state.screen = Screen::Diff;
        }
        RelayCommand::RuntimeProfile { profile } => {
            state.active_profile_id = profile;
            state.screen = Screen::RuntimeDetail;
        }
        RelayCommand::Logs { task } => {
            if let Some(t) = task {
                state.select_task_by_selector(&t);
            }
            state.logs_scroll = 0;
            state.screen = Screen::Logs;
        }
    }
    Ok(false)
}
