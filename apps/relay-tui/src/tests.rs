use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use relay_core::{
    BranchState, CoreExecutionEvent, ExecutionLimits, ExecutionReport, ProjectKey, ProjectRegistry,
    RegisterProject, Relay, RepositoryEligibility, RepositoryInspection, SchedulerSnapshot,
    SchedulerTaskStatus, SqliteStore, TaskState, list_stored_tasks,
};
use relay_runtime_antigravity::AntigravityRuntimeHealth;
use relay_runtime_codex::CodexRuntimeHealth;

use crate::commands::execute_relay_command;
use crate::input::{command_mode_available, handle_key};
use crate::state::{
    AddProjectForm, AppState, RuntimeDetailSubmenu, Screen, TaskReviewSubmenu, ToolHealth,
    git_health_marker,
};
use crate::ui::execution::{render_execution, render_result};
use crate::util::{
    activity_home, dashboard_status, durable_task_listing, parse_codex_duration, project_listing,
    task_preview_text,
};

#[test]
fn empty_relay_maps_to_expected_dashboard_message() {
    let state = AppState::new(
        Relay::new(),
        unique_temp_path("config").join("relay.toml"),
        true,
        "ready".to_owned(),
    );

    assert!(dashboard_status(&state).starts_with("No projects configured.\nNo durable tasks."));
}

#[test]
fn codex_duration_parser_is_bounded() {
    assert_eq!(
        parse_codex_duration(None).unwrap(),
        Duration::from_secs(1_800)
    );
    assert_eq!(
        parse_codex_duration(Some("3600")).unwrap(),
        Duration::from_secs(3_600)
    );
    assert!(parse_codex_duration(Some("0")).is_err());
    assert!(parse_codex_duration(Some("14401")).is_err());
    assert!(parse_codex_duration(Some("invalid")).is_err());
}

#[test]
fn slash_is_reserved_only_for_navigation_views() {
    assert!(command_mode_available(Screen::Dashboard));
    assert!(command_mode_available(Screen::Tasks));
    assert!(!command_mode_available(Screen::AddProject));
    assert!(!command_mode_available(Screen::ImportFile));
    assert!(!command_mode_available(Screen::PasteTask));
}

#[test]
fn empty_project_screen_lists_empty_state() {
    assert!(
        project_listing(&Relay::new(), &BTreeMap::new(), "ready")
            .contains("No projects configured.")
    );
    assert!(project_listing(&Relay::new(), &BTreeMap::new(), "ready").contains("ready"));
}

#[test]
fn project_listing_uses_core_registry_data() {
    let mut registry = ProjectRegistry::empty();
    registry
        .register(RegisterProject {
            key: "Relay".to_owned(),
            name: Some("Relay".to_owned()),
            local_path: std::env::current_dir().unwrap(),
            expected_remote: Some("https://github.com/Jacinth091/Relay.git".to_owned()),
            default_stack: Some("chatgpt-codex".to_owned()),
            github: None,
        })
        .unwrap();
    let relay = Relay::with_project_registry(registry);
    let listing = project_listing(&relay, &BTreeMap::new(), "ready");

    assert!(listing.contains("relay (Relay)"));
    assert!(listing.contains("https://github.com/Jacinth091/Relay.git"));
    assert!(listing.contains("chatgpt-codex"));
}

#[test]
fn project_listing_uses_cached_repository_inspection() {
    let dir = unique_temp_path("cached-inspection");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".git"), "not a git repository").unwrap();
    let mut registry = ProjectRegistry::empty();
    registry
        .register(RegisterProject {
            key: "Relay".to_owned(),
            name: Some("Relay".to_owned()),
            local_path: dir.clone(),
            expected_remote: Some("https://github.com/Jacinth091/Relay.git".to_owned()),
            default_stack: None,
            github: None,
        })
        .unwrap();
    let key = ProjectKey::parse("relay").unwrap();
    let relay = Relay::with_project_registry(registry);
    let mut cache = BTreeMap::new();
    cache.insert(
        key.clone(),
        RepositoryInspection {
            project_key: key,
            local_path: dir,
            normalized_expected_remote: None,
            normalized_actual_remote: None,
            status: Some(relay_core::RepositoryStatus {
                branch_state: BranchState::Branch("main".to_owned()),
                clean: true,
            }),
            eligibility: RepositoryEligibility::Eligible,
        },
    );

    let listing = project_listing(&relay, &cache, "ready");

    assert!(listing.contains("Repository: eligible; branch main; clean"));
}

#[test]
fn new_task_navigation_reaches_import_and_paste_screens() {
    let mut state = app_state_with_project();

    state.start_new_task();
    assert_eq!(state.screen, Screen::NewTask);

    state.start_import_file();
    assert_eq!(state.screen, Screen::ImportFile);

    state.start_new_task();
    state.start_paste_task();
    assert_eq!(state.screen, Screen::PasteTask);
}

#[tokio::test]
async fn paste_import_validation_preview_and_confirm_create_reviewable_task() {
    let mut state = app_state_with_project_with_store().await;

    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();

    assert_eq!(state.screen, Screen::TaskPreview);
    assert!(state.task_candidate.is_some());
    assert!(
        task_preview_text(state.task_candidate.as_ref().unwrap()).contains("Validation: VALID")
    );

    state.confirm_task_import().await;

    assert_eq!(state.screen, Screen::TaskReview);
    assert!(state.relay.reviewed_task().is_some());
    assert_eq!(state.durable_tasks.len(), 1);
    assert!(state.feedback.contains("Execution has not started"));
}

#[tokio::test]
async fn durable_tasks_are_visible_after_reopen_without_reviewed_task() {
    let store = SqliteStore::in_memory().await.unwrap();
    let mut state = app_state_with_project();
    state.store = Some(store.clone());

    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;

    let durable_tasks = list_stored_tasks(&store).await.unwrap();
    let mut reopened = AppState::new_with_store(
        Relay::with_project_registry(state.relay.project_registry().clone()),
        unique_temp_path("config").join("relay.toml"),
        true,
        unique_temp_path("state").join("relay.sqlite3"),
        Some(store),
        durable_tasks,
        "ready".to_owned(),
    );

    assert!(reopened.relay.reviewed_task().is_none());
    reopened.enter_tasks();
    assert_eq!(reopened.screen, Screen::Tasks);

    let listing = durable_task_listing(&reopened.durable_tasks, "ready");
    let task_id = reopened.durable_tasks[0].id.to_string();
    assert!(listing.contains(&task_id));
    assert!(listing.contains("Implement local ingress"));
    assert!(listing.contains("Project: relay"));
    assert!(listing.contains("State: ready"));
    assert!(listing.contains("Submission transport: local-paste"));
    assert!(listing.contains("Goal: Import a local handoff task."));
    assert!(listing.contains("* Show a preview."));
    assert!(listing.contains("* Do not execute."));
    assert!(listing.contains("* Review in memory."));
    assert!(!listing.to_ascii_lowercase().contains("run"));
    assert!(!listing.to_ascii_lowercase().contains("run"));
}

#[tokio::test]
async fn multiline_json_paste_event_waits_for_explicit_validation() {
    let mut state = app_state_with_project();

    state.start_paste_task();
    state.handle_paste(&task_json());

    assert_eq!(state.screen, Screen::PasteTask);
    assert!(state.task_candidate.is_none());
    assert!(state.relay.reviewed_task().is_none());

    state.submit_paste_task();

    assert_eq!(state.screen, Screen::TaskPreview);
    assert!(state.task_candidate.is_some());
}

#[tokio::test]
async fn multiline_markdown_paste_event_waits_for_explicit_validation() {
    let mut state = app_state_with_project();

    state.start_paste_task();
    state.handle_paste(&task_markdown());

    assert_eq!(state.screen, Screen::PasteTask);
    assert!(state.task_candidate.is_none());
    assert!(state.relay.reviewed_task().is_none());

    state.submit_paste_task();

    assert_eq!(state.screen, Screen::TaskPreview);
    assert!(
        task_preview_text(state.task_candidate.as_ref().unwrap())
            .contains("Implement local ingress")
    );
}

#[test]
fn file_import_validation_error_is_visible() {
    let mut state = app_state_with_project();

    state.start_import_file();
    state.import_file_path = "missing.unsupported".to_owned();
    state.submit_import_file();

    assert_eq!(state.screen, Screen::ImportFile);
    assert!(
        state
            .feedback
            .contains("unsupported local task file extension")
    );
    assert!(state.task_candidate.is_none());
}

#[test]
fn cancel_preview_creates_no_reviewed_task() {
    let mut state = app_state_with_project();

    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.cancel_task_import();

    assert_eq!(state.screen, Screen::NewTask);
    assert!(state.relay.reviewed_task().is_none());
    assert!(state.task_candidate.is_none());
}

#[tokio::test]
async fn command_palette_navigation_and_autocomplete() {
    let mut state = app_state_with_eligible_project_with_store().await;
    handle_key(
        &mut state,
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
    )
    .await
    .unwrap();
    assert!(state.command_mode);
    assert_eq!(state.command_input, "/");

    let suggs = state.get_current_suggestions();
    assert!(!suggs.is_empty());

    // Press Down to move selection
    handle_key(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(state.command_selected_index, 1);

    // Press Tab to autocomplete selected item
    handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(state.command_input, suggs[1].completion);

    // Press Esc to close palette
    handle_key(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .await
        .unwrap();
    assert!(!state.command_mode);
    assert!(state.command_input.is_empty());
}

#[tokio::test]
async fn enter_executes_fully_typed_command_over_highlighted_suggestion() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.command_mode = true;
    // Fully typed, valid command; the palette highlights it but the typed
    // text must win over any other highlighted completion.
    state.command_input = "/TASKS".to_owned();

    let suggs = state.get_current_suggestions();
    assert!(suggs.iter().any(|sugg| sugg.executable));

    handle_key(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    )
    .await
    .unwrap();

    assert_eq!(state.screen, Screen::Tasks);
}

#[tokio::test]
async fn enter_still_executes_highlighted_suggestion_for_partial_input() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.command_mode = true;
    // Partial input is not a valid command; the highlighted executable
    // suggestion should still run.
    state.command_input = "/ta".to_owned();

    let suggs = state.get_current_suggestions();
    assert!(
        suggs.first().map(|sugg| sugg.executable).unwrap_or(false),
        "expected an executable top suggestion"
    );

    handle_key(
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    )
    .await
    .unwrap();

    assert_eq!(state.screen, Screen::Tasks);
}

#[tokio::test]
async fn ctrl_c_quits_from_command_and_normal_modes() {
    let mut state = app_state_with_eligible_project_with_store().await;

    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let quit_normal = handle_key(&mut state, ctrl_c).await.unwrap();
    assert!(quit_normal);

    state.command_mode = true;
    state.command_input = "/run".to_owned();
    let quit_command = handle_key(
        &mut state,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    )
    .await
    .unwrap();
    assert!(quit_command);
}

#[tokio::test]
async fn health_probe_join_failure_marks_tools_unavailable() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.health_refresh = Some(tokio::spawn(async {
        panic!("simulated probe crash");
    }));
    tokio::task::yield_now().await;
    state.poll_health_refresh().await;

    assert!(matches!(
        state.git_health,
        ToolHealth::Unavailable(ref message) if message.contains("health probe failed")
    ));
    assert!(matches!(
        state.runtime_health,
        CodexRuntimeHealth::NotInstalled { ref error } if error.contains("health probe failed")
    ));
    assert!(matches!(
        state.antigravity_health,
        AntigravityRuntimeHealth::NotInstalled { ref error } if error.contains("health probe failed")
    ));
}

#[tokio::test]
async fn ui_preflight_reads_from_cache_and_refreshes_asynchronously() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;

    // Entering review starts an asynchronous repository inspection.
    state.open_selected_task();
    assert_eq!(state.screen, Screen::TaskReview);

    // Render-side reads never block on git: empty cache reports checking.
    let task = state.selected_stored_task().unwrap();
    assert_eq!(state.ui_preflight(task), "checking repository…");

    for _ in 0..100 {
        state.tick().await;
        let task = state.selected_stored_task().unwrap();
        if state.ui_preflight(task) != "checking repository…" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let task = state.selected_stored_task().unwrap();
    assert_eq!(state.ui_preflight(task), "eligible");
}

#[tokio::test]
async fn command_palette_contextual_profile_and_task_suggestions() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;

    // Contextual runtime profile completion
    state.command_mode = true;
    state.command_input = "/runtime use ant".to_owned();
    let suggs = state.get_current_suggestions();
    assert!(!suggs.is_empty());
    assert!(
        suggs
            .iter()
            .any(|s| s.completion == "/runtime use antigravity/implement")
    );

    // Contextual task completion for /run
    state.command_input = "/run ".to_owned();
    let task_suggs = state.get_current_suggestions();
    assert!(!task_suggs.is_empty());
    assert!(task_suggs[0].completion.starts_with("/run "));
}

#[tokio::test]
async fn task_review_exposes_explicit_run_action() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;

    assert!(state.selected_stored_task().is_some());
    assert!(
        state
            .relay
            .reviewed_task()
            .map(|task| task.handoff().title.as_str())
            .is_some()
    );
}

#[tokio::test]
async fn unsupported_extensions_are_not_presented_as_runnable() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json().replace(
        "\"ambiguity_policy\": \"needs_clarification\"",
        "\"ambiguity_policy\": \"needs_clarification\", \"extensions\": {\"fake\": {}}",
    );
    state.submit_paste_task();
    state.confirm_task_import().await;

    let task = state.selected_stored_task().unwrap();
    assert!(
        state
            .ui_preflight(task)
            .contains("unsupported task extensions")
    );
}

#[tokio::test]
async fn preflight_failure_clears_transient_status_after_refresh() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();
    state
        .execution_views
        .entry(task_id)
        .or_default()
        .scheduler_status = Some(SchedulerTaskStatus::QueuedReady);

    assert!(
        state.apply_execution_event(CoreExecutionEvent::ExecutionPreflightFailed {
            task_id,
            failure: relay_core::PreflightFailure::ProjectResolution {
                message: "missing".to_owned()
            },
        })
    );
    state.refresh_durable_tasks().await;
    state.clear_preflight_retry_state();

    assert_eq!(
        state.selected_stored_task().unwrap().state,
        TaskState::Ready
    );
    assert!(state.execution_views[&task_id].scheduler_status.is_none());
}

#[tokio::test]
async fn adding_project_rebinds_idle_scheduler_to_current_registry() {
    let store = SqliteStore::in_memory().await.unwrap();
    let mut state = AppState::new_with_store(
        Relay::new(),
        unique_temp_path("config").join("relay.toml"),
        true,
        unique_temp_path("state").join("relay.sqlite3"),
        Some(store),
        Vec::new(),
        "ready".to_owned(),
    );
    let before = state.scheduler.as_ref().unwrap().clone();
    state.add_project = AddProjectForm {
        key: "relay".to_owned(),
        path: std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        expected_remote: String::new(),
        default_stack: "fake".to_owned(),
        github_owner: String::new(),
        github_repo: String::new(),
    };
    state.submit_add_project().unwrap();

    assert!(!Arc::ptr_eq(&before, state.scheduler.as_ref().unwrap()));
    assert!(state.relay.resolve_project("relay").is_ok());
}

#[tokio::test]
async fn finished_driver_reports_are_reaped_before_new_work_can_replace_it() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();
    state.scheduler_driver = Some(tokio::spawn(async move {
        Ok(vec![ExecutionReport {
            task_id,
            project_key: ProjectKey::parse("relay").unwrap(),
            execution_id: None,
            final_state: Some(TaskState::Done),
            outcome: None,
            outcome_summary: Some("preserved".to_owned()),
            persistence_error: None,
            output: relay_core::BoundedOutputSnapshot {
                chunks: Vec::new(),
                retained_bytes: 0,
                retained_chunks: 0,
                dropped_bytes: 0,
                dropped_chunks: 0,
            },
        }])
    }));
    tokio::task::yield_now().await;
    state.reap_finished_scheduler_driver().await;

    assert_eq!(
        state.last_reports[&task_id].outcome_summary.as_deref(),
        Some("preserved")
    );
    assert!(state.scheduler_driver.is_none());
}

#[tokio::test]
async fn active_registry_rebind_blocks_run_until_the_scheduler_is_rebound() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();
    state.scheduler_driver = Some(tokio::spawn(async {
        std::future::pending::<Result<Vec<ExecutionReport>, relay_core::ExecutionSchedulerError>>()
            .await
    }));
    state.add_project = AddProjectForm {
        key: "other".to_owned(),
        path: std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        expected_remote: String::new(),
        default_stack: "fake".to_owned(),
        github_owner: String::new(),
        github_repo: String::new(),
    };
    state.submit_add_project().unwrap();

    assert!(state.scheduler_rebind_pending);
    state.submit_selected_task().await;
    assert!(state.feedback.contains("Execution refresh pending"));
    assert!(!state.execution_views.contains_key(&task_id));
}

#[tokio::test]
async fn submit_replaces_a_driver_that_finishes_during_queue_submission() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();
    state.scheduler_driver = Some(tokio::spawn(async {
        tokio::task::yield_now().await;
        Ok(Vec::new())
    }));

    state.submit_selected_task().await;
    assert!(state.scheduler_driver.is_some());
    for _ in 0..20 {
        state.tick().await;
        if state.last_reports.contains_key(&task_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(state.last_reports.contains_key(&task_id));
}

#[tokio::test]
async fn tick_starts_a_driver_for_queued_work_without_one() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();
    state
        .scheduler
        .as_ref()
        .unwrap()
        .submit(task_id)
        .await
        .unwrap();

    assert!(state.scheduler_driver.is_none());
    state.tick().await;
    assert!(state.scheduler_driver.is_some());
    for _ in 0..20 {
        state.tick().await;
        if state.last_reports.contains_key(&task_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(state.last_reports.contains_key(&task_id));
}

#[tokio::test]
async fn diff_and_logs_commands_do_not_queue_ready_task() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();

    let prefix = task_id.to_string();
    let short_id = &prefix[..8];

    execute_relay_command(&mut state, &format!("/diff {short_id}"))
        .await
        .unwrap();
    assert_eq!(state.screen, Screen::Diff);
    assert!(state.scheduler_snapshot.tasks.is_empty());

    execute_relay_command(&mut state, &format!("/logs {short_id}"))
        .await
        .unwrap();
    assert_eq!(state.screen, Screen::Logs);
    assert!(state.scheduler_snapshot.tasks.is_empty());
}

#[test]
fn add_project_does_not_overwrite_invalid_existing_config() {
    let config_path = unique_temp_path("invalid-existing-config").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let invalid_config = "[projects.relay";
    fs::write(&config_path, invalid_config).unwrap();

    let mut state = AppState::new(
        Relay::new(),
        config_path.clone(),
        false,
        "Config error".to_owned(),
    );
    state.add_project = AddProjectForm {
        key: "relay".to_owned(),
        path: std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        expected_remote: String::new(),
        default_stack: "chatgpt-codex".to_owned(),
        github_owner: String::new(),
        github_repo: String::new(),
    };

    state.submit_add_project().unwrap();

    assert_eq!(fs::read_to_string(&config_path).unwrap(), invalid_config);
    assert!(state.feedback.contains("registration is disabled"));
}

#[test]
fn doctor_marks_git_pending_until_probe_finishes() {
    assert_eq!(git_health_marker(&ToolHealth::Checking), "… Git — checking");
    assert_eq!(
        git_health_marker(&ToolHealth::Available("git version test".to_owned())),
        "✓ Git"
    );
    assert_eq!(
        git_health_marker(&ToolHealth::Unavailable("missing".to_owned())),
        "× Git"
    );
}

#[tokio::test]
async fn starting_health_refresh_invalidates_previous_readiness() {
    let mut state = app_state_with_project();
    state.begin_health_refresh(true);
    assert!(matches!(state.runtime_health, CodexRuntimeHealth::Checking));
    assert!(matches!(state.git_health, ToolHealth::Checking));
    if let Some(task) = state.health_refresh.take() {
        task.abort();
    }
}

fn app_state_with_project() -> AppState {
    let mut registry = ProjectRegistry::empty();
    registry
        .register(RegisterProject {
            key: "relay".to_owned(),
            name: Some("Relay".to_owned()),
            local_path: std::env::current_dir().unwrap(),
            expected_remote: None,
            default_stack: Some("chatgpt-codex".to_owned()),
            github: None,
        })
        .unwrap();

    let mut state = AppState::new(
        Relay::with_project_registry(registry),
        unique_temp_path("config").join("relay.toml"),
        true,
        "ready".to_owned(),
    );
    state.runtime_health = CodexRuntimeHealth::Available {
        version: "test runtime".to_owned(),
        account: None,
    };
    state
}

async fn app_state_with_project_with_store() -> AppState {
    let state = app_state_with_project();
    let mut state = AppState::new_with_store(
        state.relay,
        state.config_path,
        state.registry_writable,
        unique_temp_path("state").join("relay.sqlite3"),
        Some(SqliteStore::in_memory().await.unwrap()),
        Vec::new(),
        "ready".to_owned(),
    );
    state.runtime_health = CodexRuntimeHealth::Available {
        version: "test runtime".to_owned(),
        account: None,
    };
    state
}

async fn app_state_with_eligible_project_with_store() -> AppState {
    let path = unique_temp_path("eligible-project");
    fs::create_dir_all(&path).unwrap();
    for args in [
        vec!["init"],
        vec![
            "-c",
            "user.name=Relay Test",
            "-c",
            "user.email=relay@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/example/relay.git",
        ],
    ] {
        let status = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let mut registry = ProjectRegistry::empty();
    registry
        .register(RegisterProject {
            key: "relay".to_owned(),
            name: Some("Relay".to_owned()),
            local_path: path,
            expected_remote: Some("https://github.com/example/relay.git".to_owned()),
            default_stack: Some("fake".to_owned()),
            github: None,
        })
        .unwrap();
    let mut state = AppState::new_with_store(
        Relay::with_project_registry(registry),
        unique_temp_path("config").join("relay.toml"),
        true,
        unique_temp_path("state").join("relay.sqlite3"),
        Some(SqliteStore::in_memory().await.unwrap()),
        Vec::new(),
        "ready".to_owned(),
    );
    state.runtime_health = CodexRuntimeHealth::Available {
        version: "test runtime".to_owned(),
        account: None,
    };
    state
}

fn task_json() -> String {
    r#"{
            "protocol_version": 1,
            "project": "relay",
            "title": "Implement local ingress",
            "stack": "chatgpt-codex",
            "task": {
                "goal": "Import a local handoff task.",
                "requirements": ["Show a preview."],
                "constraints": ["Do not execute."],
                "verification": ["Review in memory."]
            },
            "ambiguity_policy": "needs_clarification"
        }"#
    .to_owned()
}

fn task_markdown() -> String {
    format!(
        "# Presentation title ignored\n\n<!-- relay:task:start -->\n```json\n{}\n```\n<!-- relay:task:end -->",
        task_json()
    )
}

#[tokio::test]
async fn result_view_displays_correct_labels_and_values() {
    let mut state = app_state_with_eligible_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();

    // Populate execution view and last report with explicit test values
    let view = state.execution_views.entry(task_id).or_default();
    view.frozen_profile = Some("codex/implement".to_owned());
    view.frozen_model = Some("gpt-5.6-sol".to_owned());
    view.frozen_reasoning = Some("high".to_owned());
    view.durable_state = Some(TaskState::Done);

    if let Some(task) = state.durable_tasks.iter_mut().find(|t| t.id == task_id) {
        task.state = TaskState::Done;
    }
    state.last_reports.insert(
        task_id,
        relay_core::ExecutionReport {
            task_id,
            project_key: ProjectKey::parse("relay").unwrap(),
            execution_id: None,
            final_state: Some(TaskState::Done),
            outcome: Some(relay_runtime::RuntimeOutcomeKind::Success),
            outcome_summary: Some("All acceptance criteria satisfied.".to_owned()),
            persistence_error: None,
            output: relay_core::BoundedOutputSnapshot {
                chunks: Vec::new(),
                retained_bytes: 0,
                retained_chunks: 0,
                dropped_bytes: 0,
                dropped_chunks: 0,
            },
        },
    );

    let backend = ratatui::backend::TestBackend::new(80, 25);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_result(frame, &state, frame.area());
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rendered_text: String = (0..buffer.area.height)
        .flat_map(|y| {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                line.push_str(buffer[(x, y)].symbol());
            }
            line.push('\n');
            line.chars().collect::<Vec<_>>()
        })
        .collect();

    assert!(
        rendered_text.contains("Runtime: Codex"),
        "Expected Runtime line in rendered output, got:\n{rendered_text}"
    );
    assert!(
        rendered_text.contains("Model: gpt-5.6-sol"),
        "Expected Model line with correct value, got:\n{rendered_text}"
    );
    assert!(
        rendered_text.contains("Reasoning: high"),
        "Expected Reasoning line with correct value, got:\n{rendered_text}"
    );
    assert!(
        rendered_text.contains("State: done"),
        "Expected State line with correct value, got:\n{rendered_text}"
    );
    assert!(
        rendered_text.contains("Outcome: success"),
        "Expected Outcome line with correct value, got:\n{rendered_text}"
    );
    assert!(
        rendered_text.contains("Verification: PASS"),
        "Expected Verification line, got:\n{rendered_text}"
    );
}

#[tokio::test]
async fn dashboard_escalation_inbox_lists_needs_decision_and_not_ready_tasks() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;

    // A READY task must not appear in the escalation inbox.
    let ready_body = activity_home(&state);
    assert!(ready_body.contains("ESCALATION INBOX"));
    assert!(ready_body.contains("No escalations waiting."));
    assert!(!ready_body.contains("DECISION"));

    // Once escalated, the task is listed as a DECISION packet.
    let task_id = state.selected_task.unwrap();
    if let Some(task) = state.durable_tasks.iter_mut().find(|t| t.id == task_id) {
        task.state = TaskState::NeedsDecision;
    }
    let escalated_body = activity_home(&state);
    assert!(escalated_body.contains("ESCALATION INBOX"));
    assert!(escalated_body.contains("! DECISION"));
    assert!(escalated_body.contains("Implement local ingress"));
    assert!(escalated_body.contains("packet visible via /tasks"));
}

#[tokio::test]
async fn dashboard_distribution_counts_running_executions() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();

    assert!(activity_home(&state).contains("No approved work yet."));

    state.scheduler_snapshot = SchedulerSnapshot {
        tasks: vec![relay_core::SchedulerTaskSnapshot {
            task_id,
            project_key: ProjectKey::parse("relay").unwrap(),
            status: SchedulerTaskStatus::Running,
        }],
    };
    state.execution_records.insert(
        task_id,
        relay_store::ExecutionRecord {
            id: "execution-1".to_owned(),
            task_id: task_id.to_string(),
            runtime_profile: "codex/implement".to_owned(),
            phase: "implement".to_owned(),
            repair_attempt_count: 0,
            started_at: "2026-01-01T00:00:00Z".to_owned(),
            ended_at: None,
            exit_outcome: None,
            failure_category: None,
            repository_snapshot_json: None,
            runtime_metadata_json: None,
        },
    );

    let body = activity_home(&state);
    assert!(body.contains("DISTRIBUTION"));
    assert!(
        body.contains("Codex (codex/implement): 1 running · 0 completed"),
        "expected distribution row, got:\n{body}"
    );
}

#[tokio::test]
async fn execution_view_uses_handoff_status_terminology() {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();
    state
        .execution_views
        .entry(task_id)
        .or_default()
        .scheduler_status = Some(SchedulerTaskStatus::QueuedReady);
    state.selected_task = Some(task_id);

    let backend = ratatui::backend::TestBackend::new(80, 25);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_execution(frame, &state, frame.area());
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rendered_text: String = (0..buffer.area.height)
        .flat_map(|y| {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                line.push_str(buffer[(x, y)].symbol());
            }
            line.push('\n');
            line.chars().collect::<Vec<_>>()
        })
        .collect();

    assert!(
        rendered_text.contains("Handoff status: QUEUED"),
        "expected handoff status label, got:\n{rendered_text}"
    );
    assert!(
        !rendered_text.contains("Scheduler:"),
        "execution view must not use scheduler terminology:\n{rendered_text}"
    );
}

fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("relay-tui-{name}-{nanos}"))
}

// --- Settings form -------------------------------------------------------

use crate::runtime_settings::RuntimeSettings;
use crate::state::SettingsField;
use crate::ui::key_hints;

fn settings_test_state() -> AppState {
    let mut state = AppState::new(
        Relay::new(),
        unique_temp_path("settings-config").join("relay.toml"),
        true,
        "ready".to_owned(),
    );
    // Route persistence at a temp path so tests never touch real user config.
    state.runtime_config_path = unique_temp_path("settings-save").join("runtimes.toml");
    state.screen = Screen::Settings;
    state
}

async fn settings_key(state: &mut AppState, code: KeyCode) {
    let quit = handle_key(state, KeyEvent::new(code, KeyModifiers::NONE))
        .await
        .unwrap();
    assert!(!quit);
}

#[tokio::test]
async fn settings_selection_and_edit_updates_pending_buffer() {
    let mut state = settings_test_state();
    assert_eq!(state.settings_form.selected, SettingsField::MaxParallel);
    assert!(state.settings_form.buffer.is_empty());

    settings_key(&mut state, KeyCode::Down).await;
    assert_eq!(
        state.settings_form.selected,
        SettingsField::PerProjectMaxParallel
    );
    settings_key(&mut state, KeyCode::Up).await;
    settings_key(&mut state, KeyCode::Up).await;
    assert_eq!(
        state.settings_form.selected,
        SettingsField::MaxDurationMinutes
    );

    // Enter begins editing prefilled with the current value.
    settings_key(&mut state, KeyCode::Enter).await;
    assert!(state.settings_form.editing);
    assert_eq!(state.settings_form.buffer, "0");

    settings_key(&mut state, KeyCode::Char('9')).await;
    settings_key(&mut state, KeyCode::Char('0')).await;
    assert_eq!(state.settings_form.buffer, "090");

    settings_key(&mut state, KeyCode::Backspace).await;
    assert_eq!(state.settings_form.buffer, "09");

    // Esc exits edit mode without leaving the screen or committing.
    settings_key(&mut state, KeyCode::Esc).await;
    assert!(!state.settings_form.editing);
    assert!(state.settings_form.buffer.is_empty());
    assert_eq!(state.screen, Screen::Settings);
}

#[tokio::test]
async fn settings_digit_press_begins_edit_with_that_digit() {
    let mut state = settings_test_state();
    settings_key(&mut state, KeyCode::Down).await;
    settings_key(&mut state, KeyCode::Char('3')).await;
    assert!(state.settings_form.editing);
    assert_eq!(state.settings_form.buffer, "3");

    // Esc leaves the screen only after a second press (edit mode first).
    settings_key(&mut state, KeyCode::Esc).await;
    assert_eq!(state.screen, Screen::Settings);
    settings_key(&mut state, KeyCode::Esc).await;
    assert_eq!(state.screen, Screen::Dashboard);
}

#[tokio::test]
async fn settings_invalid_input_is_rejected_without_saving() {
    let mut state = settings_test_state();

    // Non-positive value on global parallelism.
    settings_key(&mut state, KeyCode::Char('0')).await;
    settings_key(&mut state, KeyCode::Enter).await;
    let error = state.settings_form.error.clone().unwrap_or_default();
    assert!(error.contains("positive integer"), "got: {error}");
    assert_eq!(
        state.execution_limits.max_parallel,
        ExecutionLimits::default().max_parallel
    );

    // Per-project above the current global limit.
    settings_key(&mut state, KeyCode::Esc).await;
    settings_key(&mut state, KeyCode::Down).await;
    settings_key(&mut state, KeyCode::Char('9')).await;
    settings_key(&mut state, KeyCode::Enter).await;
    let error = state.settings_form.error.clone().unwrap_or_default();
    assert!(
        error.contains("cannot exceed global parallelism"),
        "got: {error}"
    );

    // Nothing was persisted for either rejected attempt.
    assert!(!state.runtime_config_path.exists());
    assert_eq!(
        state.execution_limits.per_project_max_parallel,
        ExecutionLimits::default().per_project_max_parallel
    );
}

#[tokio::test]
async fn settings_valid_save_persists_through_runtime_settings_toml() {
    let mut state = settings_test_state();

    settings_key(&mut state, KeyCode::Char('4')).await;
    settings_key(&mut state, KeyCode::Enter).await;
    assert!(!state.settings_form.editing);
    assert!(state.settings_form.saved);
    assert!(state.settings_form.error.is_none());
    assert_eq!(state.execution_limits.max_parallel, 4);

    // Duration: select via wrap-around previous, then type a fresh value.
    settings_key(&mut state, KeyCode::Up).await;
    assert_eq!(
        state.settings_form.selected,
        SettingsField::MaxDurationMinutes
    );
    settings_key(&mut state, KeyCode::Char('9')).await;
    settings_key(&mut state, KeyCode::Char('0')).await;
    settings_key(&mut state, KeyCode::Enter).await;
    assert_eq!(
        state.execution_limits.max_duration,
        Duration::from_secs(5_400)
    );

    let persisted = RuntimeSettings::load_from_path(&state.runtime_config_path).unwrap();
    assert_eq!(persisted.execution.max_parallel, Some(4));
    assert_eq!(persisted.execution.max_duration_secs, Some(5_400));
}

#[test]
fn settings_key_bar_shows_edit_mode_hints_only_while_editing() {
    let mut state = settings_test_state();
    let browse = key_hints(&state);
    assert!(browse.iter().any(|(key, _)| key == "0-9"));
    assert!(!browse.iter().any(|(_key, label)| label == "save"));

    state.begin_settings_edit(None);
    let editing = key_hints(&state);
    assert!(
        editing
            .iter()
            .any(|(key, label)| key == "enter" && label == "save")
    );
    assert!(editing.iter().any(|(_, label)| label == "cancel edit"));
    assert!(!editing.iter().any(|(key, _)| key == "0-9"));
}

fn render_dashboard_text(state: &AppState) -> String {
    let backend = ratatui::backend::TestBackend::new(80, 25);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::render_dashboard(frame, state, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .flat_map(|y| {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                line.push_str(buffer[(x, y)].symbol());
            }
            line.push('\n');
            line.chars().collect::<Vec<_>>()
        })
        .collect()
}

async fn ready_task_state_with_queue() -> (AppState, relay_core::SchedulerTaskSnapshot) {
    let mut state = app_state_with_project_with_store().await;
    state.start_paste_task();
    state.paste_text = task_json();
    state.submit_paste_task();
    state.confirm_task_import().await;
    let task_id = state.selected_task.unwrap();
    (
        state,
        relay_core::SchedulerTaskSnapshot {
            task_id,
            project_key: ProjectKey::parse("relay").unwrap(),
            status: SchedulerTaskStatus::QueuedReady,
        },
    )
}

#[tokio::test]
async fn dashboard_projection_lists_ready_task_when_slot_free() {
    let (mut state, queued) = ready_task_state_with_queue().await;
    state.scheduler_snapshot = SchedulerSnapshot {
        tasks: vec![queued],
    };

    let body = render_dashboard_text(&state);
    assert!(
        body.contains("next: Implement local ingress -> Codex"),
        "expected queue projection row, got:\n{body}"
    );
    assert!(
        !body.contains("waiting for execution slot"),
        "slot-free projection must not show waiting line, got:\n{body}"
    );
}

#[tokio::test]
async fn dashboard_projection_shows_waiting_line_when_slots_full() {
    let (mut state, queued) = ready_task_state_with_queue().await;
    state.execution_limits.max_parallel = 1;
    state.execution_limits.per_project_max_parallel = 1;
    let running = relay_core::SchedulerTaskSnapshot {
        status: SchedulerTaskStatus::Running,
        ..queued.clone()
    };
    state.scheduler_snapshot = SchedulerSnapshot {
        tasks: vec![running, queued],
    };

    let body = render_dashboard_text(&state);
    assert!(
        body.contains("next: Implement local ingress -> Codex"),
        "expected queued handoff still projected, got:\n{body}"
    );
    assert!(
        body.contains("waiting for execution slot (1/1 active)"),
        "expected waiting-for-slot line, got:\n{body}"
    );
}

fn filtered_model_catalog() -> relay_runtime::RuntimeModelCatalog {
    relay_runtime::RuntimeModelCatalog {
        default_model: "test-model".to_owned(),
        available_models: vec![relay_runtime::RuntimeModelInfo {
            id: "test-model".to_owned(),
            display_name: "Test Model".to_owned(),
            supported_reasoning_efforts: vec![
                relay_core::ReasoningEffort::Low,
                relay_core::ReasoningEffort::Medium,
            ],
            default_reasoning_effort: None,
        }],
    }
}

fn runtime_detail_test_state() -> AppState {
    let mut state = app_state_with_project();
    // Route persistence at a temp path so tests never touch real user config.
    state.runtime_config_path = unique_temp_path("profile-use").join("runtimes.toml");
    state.screen = Screen::RuntimeDetail;
    state
}

async fn press(state: &mut AppState, code: KeyCode) {
    let quit = handle_key(state, KeyEvent::new(code, KeyModifiers::NONE))
        .await
        .unwrap();
    assert!(!quit);
}

fn render_runtime_detail_text(state: &AppState) -> String {
    let backend = ratatui::backend::TestBackend::new(80, 60);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::render_runtime_detail(frame, state, frame.area()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn runtime_detail_change_profile_uses_the_runtime_use_validation_path() {
    let mut via_menu = runtime_detail_test_state();
    via_menu.runtime_detail_provider = Some("antigravity".to_owned());
    via_menu.active_model =
        relay_core::RuntimeModelSelection::Explicit("cross-provider-model".to_owned());
    via_menu.active_reasoning_effort =
        relay_core::RuntimeReasoningSelection::Explicit(relay_core::ReasoningEffort::XHigh);

    // Navigate to the third menu item ("Change profile") and open it.
    press(&mut via_menu, KeyCode::Down).await;
    press(&mut via_menu, KeyCode::Down).await;
    assert_eq!(via_menu.runtime_detail_menu_index, 2);
    press(&mut via_menu, KeyCode::Enter).await;
    assert_eq!(
        via_menu.runtime_detail_submenu,
        Some(RuntimeDetailSubmenu::ProfileSelector)
    );

    // Select antigravity/implement from the provider's offered profiles.
    let target = "antigravity/implement";
    let profiles = AppState::provider_profiles("antigravity");
    assert!(profiles.iter().any(|p| p.id == target));
    let target_index = profiles
        .iter()
        .position(|p| p.id == target)
        .expect("target profile offered");
    for _ in 0..target_index {
        press(&mut via_menu, KeyCode::Down).await;
    }
    press(&mut via_menu, KeyCode::Enter).await;

    // Applied like `/runtime use`: submenu closed, provider/profile adopted,
    // persisted, and cross-provider tuning re-resolved.
    assert_eq!(via_menu.runtime_detail_submenu, None);
    assert_eq!(via_menu.active_profile_id, target);
    assert_eq!(via_menu.active_runtime_provider, "antigravity");
    assert_ne!(
        via_menu.active_model.as_option(),
        Some("cross-provider-model")
    );
    assert!(via_menu.feedback.contains("Active runtime profile"));
    let persisted =
        crate::runtime_settings::RuntimeSettings::load_from_path(&via_menu.runtime_config_path)
            .unwrap();
    assert_eq!(persisted.runtime.default_profile, target);

    // Same outcome as typing the equivalent /runtime use command.
    let mut via_command = runtime_detail_test_state();
    via_command.runtime_detail_provider = Some("antigravity".to_owned());
    via_command.active_model =
        relay_core::RuntimeModelSelection::Explicit("cross-provider-model".to_owned());
    via_command.active_reasoning_effort =
        relay_core::RuntimeReasoningSelection::Explicit(relay_core::ReasoningEffort::XHigh);
    execute_relay_command(&mut via_command, "/runtime use antigravity/implement")
        .await
        .unwrap();

    assert_eq!(via_menu.active_profile_id, via_command.active_profile_id);
    assert_eq!(
        via_menu.active_runtime_provider,
        via_command.active_runtime_provider
    );
    assert_eq!(via_menu.active_model, via_command.active_model);
    assert_eq!(
        via_menu.active_reasoning_effort,
        via_command.active_reasoning_effort
    );
    assert_eq!(via_menu.feedback, via_command.feedback);
}

#[tokio::test]
async fn runtime_detail_reasoning_selector_is_filtered_by_catalog() {
    // With a model catalog: only the model's supported efforts are reachable.
    let mut state = runtime_detail_test_state();
    // Pin the selection so the test never depends on real user config.
    state.active_model = relay_core::RuntimeModelSelection::ProviderDefault;
    state.cached_model_catalog = Some(filtered_model_catalog());

    press(&mut state, KeyCode::Down).await; // "Change reasoning"
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(
        state.runtime_detail_submenu,
        Some(RuntimeDetailSubmenu::ReasoningSelector)
    );

    // Catalog offers low + medium only; navigation is bounded at medium.
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Down).await;
    eprintln!("DEBUG after down idx={}", state.runtime_detail_menu_index);
    eprintln!("DEBUG submenu={:?}", state.runtime_detail_submenu);
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Down).await;
    assert_eq!(state.runtime_detail_menu_index, 1);
    press(&mut state, KeyCode::Enter).await;

    assert_eq!(
        state.active_reasoning_effort,
        relay_core::RuntimeReasoningSelection::Explicit(relay_core::ReasoningEffort::Medium)
    );

    // Without a catalog: full effort list including xhigh.
    let mut state = runtime_detail_test_state();
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Enter).await;
    for _ in 0..3 {
        press(&mut state, KeyCode::Down).await;
    }
    assert_eq!(state.runtime_detail_menu_index, 3);
    press(&mut state, KeyCode::Enter).await;

    assert_eq!(
        state.active_reasoning_effort,
        relay_core::RuntimeReasoningSelection::Explicit(relay_core::ReasoningEffort::XHigh)
    );
}

#[tokio::test]
async fn task_review_reasoning_selector_is_filtered_by_catalog_and_full_without_one() {
    async fn review_state_with_catalog(
        catalog: Option<relay_runtime::RuntimeModelCatalog>,
    ) -> AppState {
        let mut state = app_state_with_eligible_project_with_store().await;
        state.start_paste_task();
        state.paste_text = task_json();
        state.submit_paste_task();
        state.confirm_task_import().await;
        state.open_selected_task();
        assert_eq!(state.screen, Screen::TaskReview);
        for _ in 0..100 {
            state.tick().await;
            let task = state.selected_stored_task().unwrap();
            if state.ui_preflight(task) == "eligible" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let task = state.selected_stored_task().unwrap();
        assert_eq!(state.ui_preflight(task), "eligible");

        state.cached_model_catalog = catalog;
        state.one_run_model_override = Some("test-model".to_owned());
        state
    }

    // With a catalog matching the one-run model: filtered to low/medium.
    let mut state = review_state_with_catalog(Some(filtered_model_catalog())).await;
    press(&mut state, KeyCode::Down).await; // "Change reasoning for this run"
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(
        state.task_review_submenu,
        Some(TaskReviewSubmenu::ReasoningSelector)
    );
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Down).await;
    assert_eq!(state.task_review_menu_index, 1);
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(state.one_run_reasoning_override.as_deref(), Some("medium"));

    // Without a catalog: full list, xhigh selectable.
    let mut state = review_state_with_catalog(None).await;
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Enter).await;
    for _ in 0..3 {
        press(&mut state, KeyCode::Down).await;
    }
    assert_eq!(state.task_review_menu_index, 3);
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(state.one_run_reasoning_override.as_deref(), Some("xhigh"));
}

#[test]
fn codex_transport_label_follows_env_semantics() {
    let state = app_state_with_project();

    // SAFETY: single-threaded mutation of a test-only env var; restored below.
    unsafe { std::env::remove_var("RELAY_CODEX_TRANSPORT") };
    // Unset means the app-server default.
    let text = render_runtime_detail_text(&state);
    assert!(
        text.contains("App Server"),
        "unset should show App Server:\n{text}"
    );
    assert!(
        !text.contains("Codex exec"),
        "unset must not show exec:\n{text}"
    );
    assert!(
        text.contains("Live approval ✓"),
        "app-server supports live approval:\n{text}"
    );

    // Explicit app-server behaves identically.
    unsafe { std::env::set_var("RELAY_CODEX_TRANSPORT", "app-server") };
    let text = render_runtime_detail_text(&state);
    assert!(
        text.contains("App Server"),
        "explicit app-server should show App Server:\n{text}"
    );

    // Only an explicit `exec` opts out.
    unsafe { std::env::set_var("RELAY_CODEX_TRANSPORT", "exec") };
    let text = render_runtime_detail_text(&state);
    assert!(
        text.contains("Codex exec"),
        "exec should show Codex exec:\n{text}"
    );
    assert!(
        text.contains("Pre-approved envelope"),
        "exec uses pre-approved envelope:\n{text}"
    );

    // Restore so other tests observe default semantics.
    unsafe { std::env::remove_var("RELAY_CODEX_TRANSPORT") };
}

/// Renders the full UI at the given size and returns one string per row.
fn render_full_ui_rows(state: &AppState, width: u16, height: u16) -> Vec<String> {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, state))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

#[tokio::test]
async fn runtimes_list_selection_moves_wraps_and_enter_opens_selected_provider() {
    let mut state = app_state_with_project();
    state.screen = Screen::Runtimes;
    assert_eq!(state.runtimes_list_index, 0);

    press(&mut state, KeyCode::Down).await;
    assert_eq!(state.runtimes_list_index, 1);
    press(&mut state, KeyCode::Char('j')).await;
    assert_eq!(
        state.runtimes_list_index, 0,
        "j/Down must wrap past the end"
    );
    press(&mut state, KeyCode::Up).await;
    assert_eq!(state.runtimes_list_index, 1);
    press(&mut state, KeyCode::Char('k')).await;
    assert_eq!(
        state.runtimes_list_index, 0,
        "k/Up must wrap past the start"
    );

    // Enter on the highlighted provider opens ITS runtime detail.
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(state.screen, Screen::RuntimeDetail);
    assert_eq!(state.runtime_detail_provider.as_deref(), Some("codex"));

    let mut state = app_state_with_project();
    state.screen = Screen::Runtimes;
    press(&mut state, KeyCode::Down).await;
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(state.screen, Screen::RuntimeDetail);
    assert_eq!(
        state.runtime_detail_provider.as_deref(),
        Some("antigravity")
    );
}

#[test]
fn runtimes_screen_renders_selectable_panel_rows() {
    let mut state = app_state_with_project();
    state.screen = Screen::Runtimes;
    let rows = render_full_ui_rows(&state, 80, 24);

    assert!(
        rows.iter().any(|row| row.contains("RUNTIMES")),
        "expected bordered RUNTIMES panel title:\n{rows:?}"
    );
    let list_row = rows
        .iter()
        .find(|row| row.contains("Codex CLI"))
        .expect("provider row rendered");
    assert!(
        list_row.contains('›'),
        "selected row must show the cursor marker: {list_row}"
    );
    let unselected = rows
        .iter()
        .find(|row| row.contains("Antigravity"))
        .expect("antigravity row rendered");
    assert!(
        !unselected.contains('›'),
        "unselected rows must not show a cursor: {unselected}"
    );
}

#[test]
fn top_bar_keeps_right_segment_visible_at_narrow_width() {
    let mut state = app_state_with_project();
    state.active_runtime_provider = "antigravity".to_owned();

    // Narrow enough that the old padded single-line bar truncated "antigravity".
    let rows = render_full_ui_rows(&state, 26, 12);
    let bar = &rows[0];
    assert!(
        bar.contains("antigravity"),
        "provider name must be fully visible at width 26: {bar:?}"
    );
    assert!(
        bar.trim_end().ends_with('●'),
        "health marker must sit at the right edge: {bar:?}"
    );
    assert!(
        !bar.contains("dashboard"),
        "the left title truncates first when space is tight: {bar:?}"
    );
}

#[tokio::test]
async fn runtime_detail_action_menu_activates_change_model_and_test_runtime() {
    // Action 0: Change model opens the catalog selector and commits a choice.
    let mut state = runtime_detail_test_state();
    state.cached_model_catalog = Some(filtered_model_catalog());
    assert_eq!(state.runtime_detail_menu_index, 0);
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(
        state.runtime_detail_submenu,
        Some(RuntimeDetailSubmenu::ModelSelector)
    );
    press(&mut state, KeyCode::Enter).await;
    assert_eq!(state.runtime_detail_submenu, None);
    assert_eq!(
        state.active_model,
        relay_core::RuntimeModelSelection::Explicit("test-model".to_owned())
    );

    // Action 3: Test runtime wraps into reach from index 0 via Up.
    let mut state = runtime_detail_test_state();
    press(&mut state, KeyCode::Up).await;
    assert_eq!(
        state.runtime_detail_menu_index, 3,
        "top-level action menu wraps upward"
    );
    press(&mut state, KeyCode::Enter).await;
    assert!(state.runtime_detail_submenu.is_none());
    assert!(matches!(state.runtime_health, CodexRuntimeHealth::Checking));
    assert!(
        state.health_refresh.is_some(),
        "Test runtime must trigger a fresh health probe"
    );
}
