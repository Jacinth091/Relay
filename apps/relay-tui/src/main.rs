use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    io::{self, Stdout},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use relay_core::{
    AmbiguityPolicy, BranchState, CoreExecutionEvent, ExecutionLimits, ExecutionReport,
    ExecutionScheduler, GitHubRepositoryMetadata, HandoffTask, ProjectKey, RegisterProject, Relay,
    RepositoryEligibility, RepositoryInspection, RepositorySafetyFailure, SchedulerSnapshot,
    SchedulerTaskStatus, SqliteStore, StoredTask, TaskId, TaskImportCandidate, TaskState,
    default_config_path, default_database_path, list_stored_tasks,
};
use relay_runtime::RuntimeOutputStream;
use relay_runtime_fake::FakeRuntime;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

type Tui = Terminal<CrosstermBackend<Stdout>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let config_path = default_config_path();
    let (relay, registry_writable, feedback) = match Relay::load_from_config_path(&config_path) {
        Ok(relay) => (relay, true, format!("Config: {}", config_path.display())),
        Err(error) => (
            Relay::new(),
            false,
            format!(
                "Config error at {}: {error}\nProject registration is disabled until the config is fixed.",
                config_path.display()
            ),
        ),
    };
    let database_path = default_database_path();
    let (store, durable_tasks, database_feedback) = open_store(&database_path).await;
    let mut state = AppState::new_with_store(
        relay,
        config_path,
        registry_writable,
        database_path,
        store,
        durable_tasks,
        format!("{feedback}\n{database_feedback}"),
    );
    let mut terminal = initialize_terminal()?;

    let run_result = run(&mut terminal, &mut state).await;
    let restore_result = restore_terminal(&mut terminal);

    run_result?;
    restore_result?;
    Ok(())
}

async fn open_store(path: &std::path::Path) -> (Option<SqliteStore>, Vec<StoredTask>, String) {
    match SqliteStore::open(path).await {
        Ok(store) => match list_stored_tasks(&store).await {
            Ok(tasks) => {
                let feedback = format!("Database: {}", path.display());
                (Some(store), tasks, feedback)
            }
            Err(error) => (
                None,
                Vec::new(),
                format!(
                    "Database error at {}: {error}\nTask import is disabled until the database is fixed.",
                    path.display()
                ),
            ),
        },
        Err(error) => (
            None,
            Vec::new(),
            format!(
                "Database error at {}: {error}\nTask import is disabled until the database is fixed.",
                path.display()
            ),
        ),
    }
}

fn initialize_terminal() -> Result<Tui, Box<dyn Error>> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        return Err(error.into());
    }

    let backend = CrosstermBackend::new(stdout);
    match Terminal::new(backend) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
            Err(error.into())
        }
    }
}

fn restore_terminal(terminal: &mut Tui) -> Result<(), Box<dyn Error>> {
    let raw_mode_result = disable_raw_mode();
    let terminal_modes_result = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let cursor_result = terminal.show_cursor();

    raw_mode_result?;
    terminal_modes_result?;
    cursor_result?;
    Ok(())
}

async fn run(terminal: &mut Tui, state: &mut AppState) -> Result<(), Box<dyn Error>> {
    loop {
        state.tick().await;
        terminal.draw(|frame| render(frame, state))?;

        if event::poll(Duration::from_millis(30))? && handle_event(state, event::read()?).await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

fn render(frame: &mut Frame, state: &AppState) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(frame.area());

    match state.screen {
        Screen::Dashboard => render_dashboard(frame, state, areas[0]),
        Screen::Tasks => render_tasks(frame, state, areas[0]),
        Screen::Projects => render_projects(frame, state, areas[0]),
        Screen::AddProject => render_add_project(frame, state, areas[0]),
        Screen::NewTask => render_new_task(frame, state, areas[0]),
        Screen::ImportFile => render_import_file(frame, state, areas[0]),
        Screen::PasteTask => render_paste_task(frame, state, areas[0]),
        Screen::TaskPreview => render_task_preview(frame, state, areas[0]),
        Screen::TaskReview => render_task_review(frame, state, areas[0]),
        Screen::Execution => render_execution(frame, state, areas[0]),
        Screen::Result => render_result(frame, state, areas[0]),
    }

    let help = Paragraph::new(help_text(state));
    frame.render_widget(help, areas[1]);
}

fn render_dashboard(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = format!("{}\n\n{}", dashboard_status(state), state.feedback);
    let dashboard = Paragraph::new(body)
        .wrap(Wrap { trim: true })
        .block(Block::default().title(" Relay ").borders(Borders::ALL));
    frame.render_widget(dashboard, area);
}

fn render_tasks(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = task_selection_listing(state);
    let tasks = Paragraph::new(body)
        .wrap(Wrap { trim: false })
        .block(Block::default().title(" Tasks ").borders(Borders::ALL));
    frame.render_widget(tasks, area);
}

fn render_projects(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = project_listing(&state.relay, &state.repository_inspections, &state.feedback);
    let projects = Paragraph::new(body)
        .wrap(Wrap { trim: false })
        .block(Block::default().title(" Projects ").borders(Borders::ALL));
    frame.render_widget(projects, area);
}

fn render_add_project(frame: &mut Frame, state: &AppState, area: Rect) {
    let form = &state.add_project;
    let body = [
        add_field_line(state, AddProjectField::Key, "Key", &form.key),
        add_field_line(state, AddProjectField::Path, "Path", &form.path),
        add_field_line(
            state,
            AddProjectField::ExpectedRemote,
            "Expected remote",
            &form.expected_remote,
        ),
        add_field_line(
            state,
            AddProjectField::DefaultStack,
            "Default stack",
            &form.default_stack,
        ),
        add_field_line(
            state,
            AddProjectField::GitHubOwner,
            "GitHub owner",
            &form.github_owner,
        ),
        add_field_line(
            state,
            AddProjectField::GitHubRepo,
            "GitHub repo",
            &form.github_repo,
        ),
        String::new(),
        state.feedback.clone(),
    ]
    .join("\n");
    let add_project = Paragraph::new(body).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(" Add Project ")
            .borders(Borders::ALL),
    );
    frame.render_widget(add_project, area);
}

fn render_new_task(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = format!(
        "Import a user-approved Relay handoff.\n\nF: Import File\nV: Paste Task\n\n{}",
        state.feedback
    );
    let new_task = Paragraph::new(body)
        .wrap(Wrap { trim: true })
        .block(Block::default().title(" New Task ").borders(Borders::ALL));
    frame.render_widget(new_task, area);
}

fn render_import_file(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = format!("Path: {}\n\n{}", state.import_file_path, state.feedback);
    let import_file = Paragraph::new(body).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(" Import File ")
            .borders(Borders::ALL),
    );
    frame.render_widget(import_file, area);
}

fn render_paste_task(frame: &mut Frame, state: &AppState, area: Rect) {
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

fn render_task_preview(frame: &mut Frame, state: &AppState, area: Rect) {
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

fn render_task_review(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state
        .selected_stored_task()
        .map(|task| {
            let preflight = state.ui_preflight(task);
            let action = if task.state == TaskState::Ready && preflight == "eligible" {
                "\n\n[R] Run  [B] Back"
            } else if matches!(task.state, TaskState::Done | TaskState::Failed | TaskState::Cancelled) {
                "\n\n[Enter] View result  [B] Back"
            } else if matches!(task.state, TaskState::NeedsClarification | TaskState::NeedsDecision) {
                "\n\nATTENTION REQUIRED: external resolution is required.\n[B] Back"
            } else { "\n\n[B] Back" };
            format!(
                "Task: {}\nProject: {}\nRuntime: Relay Fake Runtime\nProfile: fake/default\nState: {}\nRepository: {}\n\n{}{}",
                task.id,
                task.project_key,
                task.state.as_str(),
                preflight,
                handoff_summary_text(
                    &task.handoff, task.project_key.as_str(), state.relay.resolve_project(task.project_key.as_str()).ok().and_then(|p| p.name())
                ), action
            )
        })
        .unwrap_or_else(|| "No durable task selected.".to_owned());
    let review = Paragraph::new(body).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(" Task Review ")
            .borders(Borders::ALL),
    );
    frame.render_widget(review, area);
}

fn render_execution(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state.selected_task.and_then(|id| state.execution_views.get(&id).map(|view| (id, view))).map(|(id, view)| {
        let task = state.durable_tasks.iter().find(|task| task.id == id);
        let output = view.output.iter().cloned().collect::<Vec<_>>().join("");
        format!("Task: {id}\nProject: {}\nRuntime: Relay Fake Runtime\nScheduler: {}\nPhase: {}\nRepair: {}\nElapsed: {}s\n{}\n\nOutput:\n{}\n\n[C] Cancel  [B] Background / Tasks",
            task.map(|task| task.project_key.as_str()).unwrap_or("unknown"),
            view.scheduler_status.as_ref().map(scheduler_status_text).unwrap_or("not scheduled"),
            view.durable_state.map(|state| state.as_str()).unwrap_or("state pending"), view.repair_attempt,
            view.started_at.map(|started| started.elapsed().as_secs()).unwrap_or(0),
            view.persistence_error.as_ref().map(|error| format!("STATE UNCERTAIN / DURABILITY FAILURE\n{error}")).unwrap_or_else(|| if view.cancellation_requested { "Cancellation requested...".to_owned() } else { String::new() }), output)
    }).unwrap_or_else(|| "No execution selected.".to_owned());
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(Block::default().title(" Execution ").borders(Borders::ALL)),
        area,
    );
}

fn render_result(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state.selected_stored_task().map(|task| {
        let report = state.last_reports.get(&task.id);
        let repository = state.relay.inspect_project_repository(task.project_key.as_str()).ok().map(|i| repository_inspection_line(&i.eligibility, &i.status)).unwrap_or_else(|| "unavailable".to_owned());
        format!("Task: {}\nProject: {}\nRuntime: Relay Fake Runtime\nState: {}\nOutcome: {}\nVerification: {}\nRepairs: {}\nRepository: {}\n\nSummary:\n{}\n\n[T] Tasks  [D] Dashboard  [N] New Task",
            task.id, task.project_key, task.state.as_str(), report.and_then(|r| r.outcome).map(|o| o.as_str()).unwrap_or("not confirmed"),
            if task.state == TaskState::Done { "PASS" } else { "not available" }, state.execution_views.get(&task.id).map(|v| v.repair_attempt).unwrap_or(0), repository,
            report.and_then(|r| r.outcome_summary.as_deref()).unwrap_or("No confirmed execution report."))
    }).unwrap_or_else(|| "No durable task selected.".to_owned());
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(Block::default().title(" Result ").borders(Borders::ALL)),
        area,
    );
}

async fn handle_event(state: &mut AppState, event: Event) -> Result<bool, Box<dyn Error>> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(state, key).await,
        Event::Paste(text) if state.screen == Screen::PasteTask => {
            state.paste_text.push_str(&text);
            Ok(false)
        }
        _ => Ok(false),
    }
}

async fn handle_key(state: &mut AppState, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
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
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
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
                state.screen = Screen::Tasks;
                Ok(false)
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                state.submit_selected_task().await;
                Ok(false)
            }
            KeyCode::Enter => {
                state.open_selected_task();
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
    }
}

fn dashboard_status(state: &AppState) -> String {
    let projects = match state.relay.configured_project_count() {
        0 => "No projects configured.".to_owned(),
        count => format!("{count} projects configured."),
    };
    let tasks = match state.durable_tasks.len() {
        0 => "No durable tasks.".to_owned(),
        1 => "1 durable task.".to_owned(),
        count => format!("{count} durable tasks."),
    };
    let mut execution = vec!["Execution".to_owned()];
    for status in [
        SchedulerTaskStatus::Running,
        SchedulerTaskStatus::QueuedReady,
        SchedulerTaskStatus::QueuedGlobal,
        SchedulerTaskStatus::QueuedProject,
    ] {
        let count = state
            .scheduler_snapshot
            .tasks
            .iter()
            .filter(|task| task.status == status)
            .count();
        execution.push(format!("{}: {count}", scheduler_status_text(&status)));
    }
    for durable in [
        TaskState::NeedsClarification,
        TaskState::NeedsDecision,
        TaskState::Done,
        TaskState::Failed,
        TaskState::Cancelled,
    ] {
        execution.push(format!(
            "{}: {}",
            durable.as_str(),
            state
                .durable_tasks
                .iter()
                .filter(|task| task.state == durable)
                .count()
        ));
    }
    format!("{projects}\n{tasks}\n\n{}", execution.join("\n"))
}

fn scheduler_status_text(status: &SchedulerTaskStatus) -> &'static str {
    match status {
        SchedulerTaskStatus::Running => "RUNNING",
        SchedulerTaskStatus::QueuedReady => "QUEUED / ready for scheduler",
        SchedulerTaskStatus::QueuedGlobal => "WAITING — global execution slot",
        SchedulerTaskStatus::QueuedProject => "WAITING — project execution slot",
    }
}

fn task_selection_listing(state: &AppState) -> String {
    if state.durable_tasks.is_empty() {
        return format!("No durable tasks.\n\n{}", state.feedback);
    }
    let lines = state
        .durable_tasks
        .iter()
        .map(|task| {
            let marker = if state.selected_task == Some(task.id) {
                ">"
            } else {
                " "
            };
            let status = state
                .scheduler_snapshot
                .tasks
                .iter()
                .find(|snapshot| snapshot.task_id == task.id)
                .map(|snapshot| match snapshot.status {
                    SchedulerTaskStatus::Running => "RUNNING".to_owned(),
                    SchedulerTaskStatus::QueuedReady => "QUEUED".to_owned(),
                    SchedulerTaskStatus::QueuedGlobal => "WAIT:GLOBAL".to_owned(),
                    SchedulerTaskStatus::QueuedProject => "WAIT:PROJ".to_owned(),
                })
                .unwrap_or_else(|| task.state.as_str().to_ascii_uppercase());
            format!(
                "{marker} {status:<12} {:<12} {}",
                task.project_key.as_str(),
                task.handoff.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{lines}\n\n{}", state.feedback)
}

fn project_listing(
    relay: &Relay,
    repository_inspections: &BTreeMap<ProjectKey, RepositoryInspection>,
    feedback: &str,
) -> String {
    if relay.configured_project_count() == 0 {
        return format!("No projects configured.\n\n{feedback}");
    }

    let mut lines = Vec::new();
    for project in relay.list_projects() {
        let expected_remote = project
            .expected_remote()
            .map(|remote| remote.as_str())
            .unwrap_or("not configured");
        let default_stack = project.default_stack().unwrap_or("not configured");
        let name = project.name().unwrap_or(project.key().as_str());
        let repository = repository_inspections
            .get(project.key())
            .map(|inspection| {
                repository_inspection_line(&inspection.eligibility, &inspection.status)
            })
            .unwrap_or_else(|| "not inspected".to_owned());
        lines.push(format!(
            "{} ({})\n  Path: {}\n  Expected remote: {}\n  Default stack: {}\n  Repository: {}",
            project.key(),
            name,
            project.local_path().display(),
            expected_remote,
            default_stack,
            repository
        ));
    }

    format!("{}\n\n{feedback}", lines.join("\n\n"))
}

#[cfg(test)]
fn durable_task_listing(tasks: &[StoredTask], feedback: &str) -> String {
    if tasks.is_empty() {
        return format!("No durable tasks.\n\n{feedback}");
    }

    let lines = tasks
        .iter()
        .map(|task| {
            format!(
                "{}\n  Title: {}\n  Project: {}\n  State: {}\n  Submission transport: {}\n\n  Goal: {}\n\n  Requirements:\n{}\n\n  Constraints:\n{}\n\n  Verification:\n{}",
                task.id,
                task.handoff.title,
                task.project_key.as_str(),
                task.state.as_str(),
                task.submission_transport.as_str(),
                task.handoff.task.goal,
                indented_bullet_lines(&task.handoff.task.requirements),
                indented_bullet_lines(&task.handoff.task.constraints),
                indented_bullet_lines(&task.handoff.task.verification)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!("{lines}\n\n{feedback}")
}

fn repository_inspection_line(
    eligibility: &RepositoryEligibility,
    status: &Option<relay_core::RepositoryStatus>,
) -> String {
    match eligibility {
        RepositoryEligibility::Eligible => {
            let Some(status) = status else {
                return "eligible; status unavailable".to_owned();
            };
            format!(
                "eligible; {}; {}",
                branch_state_text(&status.branch_state),
                if status.clean { "clean" } else { "dirty" }
            )
        }
        RepositoryEligibility::NotEligible(reason) => {
            format!("not eligible; {}", safety_failure_text(reason))
        }
    }
}

fn branch_state_text(branch_state: &BranchState) -> String {
    match branch_state {
        BranchState::Branch(branch) => format!("branch {branch}"),
        BranchState::Detached { commit } => format!("detached at {commit}"),
    }
}

fn safety_failure_text(reason: &RepositorySafetyFailure) -> String {
    match reason {
        RepositorySafetyFailure::GitUnavailable => "git unavailable".to_owned(),
        RepositorySafetyFailure::NotGitWorktree => "not a Git worktree".to_owned(),
        RepositorySafetyFailure::MissingExpectedRemote => "missing expected remote".to_owned(),
        RepositorySafetyFailure::MissingRemote { name } => format!("missing remote {name}"),
        RepositorySafetyFailure::GitCommandFailed { operation, .. } => {
            format!("git inspection failed during {operation:?}")
        }
        RepositorySafetyFailure::MalformedExpectedRemote { .. } => {
            "malformed expected remote".to_owned()
        }
        RepositorySafetyFailure::MalformedActualRemote { .. } => {
            "malformed actual remote".to_owned()
        }
        RepositorySafetyFailure::RemoteMismatch { expected, actual } => {
            format!("remote mismatch: expected {expected}, actual {actual}")
        }
        RepositorySafetyFailure::UnexpectedGitOutput { operation, .. } => {
            format!("unexpected git output during {operation:?}")
        }
    }
}

fn help_text(state: &AppState) -> &'static str {
    match state.screen {
        Screen::Dashboard if state.registry_writable => {
            "N: new task  T: tasks  P: projects  A: add project  Q / Esc: quit"
        }
        Screen::Dashboard => "N: new task  T: tasks  P: projects  Q / Esc: quit",
        Screen::Tasks => "Up / Down: select  Enter: open  D / Esc: dashboard  Q: quit",
        Screen::Projects if state.registry_writable => {
            "R: refresh  A: add project  D / Esc: dashboard  Q: quit"
        }
        Screen::Projects => "R: refresh  D / Esc: dashboard  Q: quit",
        Screen::AddProject => "Tab: next field  Enter: register  Esc: projects",
        Screen::NewTask => "F: import file  V: paste task  D / Esc: dashboard  Q: quit",
        Screen::ImportFile => "Enter: validate  Esc: new task",
        Screen::PasteTask => "Paste text, Enter: validate  Ctrl+Enter: newline  Esc: new task",
        Screen::TaskPreview => "I / Enter: import  C / Esc: cancel  Q: quit",
        Screen::TaskReview => "R: run when eligible  B / Esc: tasks  Q: quit",
        Screen::Execution => "C: cancel  B: background / tasks",
        Screen::Result => "T: tasks  D: dashboard  N: new task",
    }
}

fn task_preview_text(candidate: &TaskImportCandidate) -> String {
    format!(
        "Task ID: created after Import\n{}\n\nSubmission transport: {:?}\nValidation: VALID\n\nActions: Import / Cancel",
        handoff_summary_text(
            candidate.handoff(),
            candidate.project_key().as_str(),
            candidate.project_name()
        ),
        candidate.submission_transport()
    )
}

fn handoff_summary_text(
    handoff: &HandoffTask,
    resolved_project_key: &str,
    project_name: Option<&str>,
) -> String {
    format!(
        "Project: {}{}\nTitle: {}\nRequested stack: {}\nGoal: {}\n\nRequirements:\n{}\n\nConstraints:\n{}\n\nVerification:\n{}\n\nAmbiguity policy:\n{}",
        resolved_project_key,
        project_name
            .map(|name| format!(" ({name})"))
            .unwrap_or_default(),
        handoff.title,
        handoff.stack.as_deref().unwrap_or("not requested"),
        handoff.task.goal,
        bullet_lines(&handoff.task.requirements),
        bullet_lines(&handoff.task.constraints),
        bullet_lines(&handoff.task.verification),
        ambiguity_policy_text(handoff.ambiguity_policy),
    )
}

fn bullet_lines(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("* {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
fn indented_bullet_lines(items: &[String]) -> String {
    bullet_lines(items)
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ambiguity_policy_text(policy: AmbiguityPolicy) -> &'static str {
    match policy {
        AmbiguityPolicy::NeedsClarification => "needs_clarification",
    }
}

fn add_field_line(state: &AppState, field: AddProjectField, label: &str, value: &str) -> String {
    let marker = if state.add_field == field { ">" } else { " " };
    format!("{marker} {label}: {value}")
}

#[derive(Debug)]
struct AppState {
    relay: Relay,
    config_path: PathBuf,
    registry_writable: bool,
    database_path: PathBuf,
    store: Option<SqliteStore>,
    durable_tasks: Vec<StoredTask>,
    selected_task: Option<TaskId>,
    scheduler: Option<Arc<ExecutionScheduler<FakeRuntime>>>,
    scheduler_events: Option<broadcast::Receiver<CoreExecutionEvent>>,
    scheduler_snapshot: SchedulerSnapshot,
    scheduler_driver:
        Option<JoinHandle<Result<Vec<ExecutionReport>, relay_core::ExecutionSchedulerError>>>,
    execution_views: HashMap<TaskId, ExecutionViewState>,
    last_reports: HashMap<TaskId, ExecutionReport>,
    preflight_retry_pending: HashSet<TaskId>,
    scheduler_rebind_pending: bool,
    cancel_confirmation: Option<TaskId>,
    screen: Screen,
    add_project: AddProjectForm,
    add_field: AddProjectField,
    import_file_path: String,
    paste_text: String,
    task_candidate: Option<TaskImportCandidate>,
    repository_inspections: BTreeMap<ProjectKey, RepositoryInspection>,
    feedback: String,
}

#[derive(Debug, Default)]
struct ExecutionViewState {
    scheduler_status: Option<SchedulerTaskStatus>,
    durable_state: Option<TaskState>,
    repair_attempt: u32,
    output: VecDeque<String>,
    output_bytes: usize,
    persistence_error: Option<String>,
    cancellation_requested: bool,
    started_at: Option<Instant>,
}

const UI_OUTPUT_BYTE_LIMIT: usize = 16 * 1024;

impl AppState {
    #[cfg(test)]
    fn new(relay: Relay, config_path: PathBuf, registry_writable: bool, feedback: String) -> Self {
        Self::new_with_store(
            relay,
            config_path,
            registry_writable,
            PathBuf::from("relay-state.sqlite3"),
            None,
            Vec::new(),
            feedback,
        )
    }

    fn new_with_store(
        relay: Relay,
        config_path: PathBuf,
        registry_writable: bool,
        database_path: PathBuf,
        store: Option<SqliteStore>,
        durable_tasks: Vec<StoredTask>,
        feedback: String,
    ) -> Self {
        let scheduler = store.as_ref().map(|store| {
            Arc::new(ExecutionScheduler::new(
                relay.clone(),
                store.clone(),
                Arc::new(FakeRuntime::default()),
                ExecutionLimits::default(),
            ))
        });
        let scheduler_events = scheduler.as_ref().map(|scheduler| scheduler.subscribe());
        Self {
            relay,
            config_path,
            registry_writable,
            database_path,
            store,
            durable_tasks,
            selected_task: None,
            scheduler,
            scheduler_events,
            scheduler_snapshot: SchedulerSnapshot { tasks: Vec::new() },
            scheduler_driver: None,
            execution_views: HashMap::new(),
            last_reports: HashMap::new(),
            preflight_retry_pending: HashSet::new(),
            scheduler_rebind_pending: false,
            cancel_confirmation: None,
            screen: Screen::Dashboard,
            add_project: AddProjectForm::default(),
            add_field: AddProjectField::Key,
            import_file_path: String::new(),
            paste_text: String::new(),
            task_candidate: None,
            repository_inspections: BTreeMap::new(),
            feedback,
        }
    }

    fn enter_projects(&mut self) {
        self.refresh_repository_inspections();
        self.screen = Screen::Projects;
    }

    fn enter_tasks(&mut self) {
        if self.selected_task.is_none() {
            self.selected_task = self.durable_tasks.first().map(|task| task.id);
        }
        self.screen = Screen::Tasks;
    }

    fn selected_stored_task(&self) -> Option<&StoredTask> {
        self.selected_task
            .and_then(|id| self.durable_tasks.iter().find(|task| task.id == id))
    }

    fn select_next_task(&mut self) {
        if self.durable_tasks.is_empty() {
            return;
        }
        let current = self
            .selected_task
            .and_then(|id| self.durable_tasks.iter().position(|task| task.id == id))
            .unwrap_or(0);
        self.selected_task = Some(self.durable_tasks[(current + 1) % self.durable_tasks.len()].id);
    }

    fn select_previous_task(&mut self) {
        if self.durable_tasks.is_empty() {
            return;
        }
        let current = self
            .selected_task
            .and_then(|id| self.durable_tasks.iter().position(|task| task.id == id))
            .unwrap_or(0);
        self.selected_task = Some(
            self.durable_tasks[(current + self.durable_tasks.len() - 1) % self.durable_tasks.len()]
                .id,
        );
    }

    fn open_selected_task(&mut self) {
        let Some(task) = self.selected_stored_task() else {
            return;
        };
        let active = self
            .scheduler_snapshot
            .tasks
            .iter()
            .any(|snapshot| snapshot.task_id == task.id);
        self.screen = if active {
            Screen::Execution
        } else if matches!(
            task.state,
            TaskState::Done | TaskState::Failed | TaskState::Cancelled
        ) {
            Screen::Result
        } else {
            Screen::TaskReview
        };
    }

    fn ui_preflight(&self, task: &StoredTask) -> String {
        if task.state != TaskState::Ready {
            return "unavailable: task is not READY".to_owned();
        }
        if self.scheduler.is_none() {
            return "unavailable: local Fake Runtime or store is unavailable".to_owned();
        }
        if !task.handoff.extensions.is_empty() {
            return "unavailable: unsupported task extensions".to_owned();
        }
        match self
            .relay
            .inspect_project_repository(task.project_key.as_str())
        {
            Ok(inspection) if inspection.eligibility == RepositoryEligibility::Eligible => {
                "eligible".to_owned()
            }
            Ok(inspection) => format!(
                "unavailable: {}",
                repository_inspection_line(&inspection.eligibility, &inspection.status)
            ),
            Err(error) => format!("unavailable: project missing ({error})"),
        }
    }

    async fn submit_selected_task(&mut self) {
        self.reap_finished_scheduler_driver().await;
        if self.scheduler_rebind_pending {
            self.rebind_scheduler_if_idle();
        }
        let Some(task) = self.selected_stored_task().cloned() else {
            return;
        };
        if task.state != TaskState::Ready
            || self
                .scheduler_snapshot
                .tasks
                .iter()
                .any(|snapshot| snapshot.task_id == task.id)
            || self
                .execution_views
                .get(&task.id)
                .and_then(|view| view.scheduler_status.as_ref())
                .is_some()
        {
            self.feedback = "Run is available only once for a durable READY task.".to_owned();
            return;
        }
        if self.ui_preflight(&task) != "eligible" {
            self.feedback = format!("Run unavailable: {}", self.ui_preflight(&task));
            return;
        }
        let Some(scheduler) = self.scheduler.clone() else {
            return;
        };
        match scheduler.submit(task.id).await {
            Ok(()) => {
                self.execution_views
                    .entry(task.id)
                    .or_default()
                    .scheduler_status = Some(SchedulerTaskStatus::QueuedReady);
                self.feedback = "Run approved. Queued for the scheduler.".to_owned();
                self.screen = Screen::Execution;
                if self.scheduler_driver.is_none() {
                    self.scheduler_driver =
                        Some(tokio::spawn(
                            async move { scheduler.run_until_idle().await },
                        ));
                }
            }
            Err(error) => self.feedback = format!("Run was not queued: {error}"),
        }
    }

    async fn confirm_cancel(&mut self) {
        let Some(task_id) = self.cancel_confirmation.take() else {
            return;
        };
        let Some(scheduler) = self.scheduler.clone() else {
            return;
        };
        match scheduler.cancel(task_id).await {
            Ok(()) => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .cancellation_requested = true;
                self.feedback = "Cancellation requested...".to_owned();
            }
            Err(error) => self.feedback = format!("Cancellation request failed: {error}"),
        }
    }

    async fn refresh_durable_tasks(&mut self) {
        if let Some(store) = &self.store {
            match list_stored_tasks(store).await {
                Ok(tasks) => self.durable_tasks = tasks,
                Err(error) => {
                    self.feedback = format!("Could not refresh durable task state: {error}")
                }
            }
        }
    }

    fn clear_preflight_retry_state(&mut self) {
        for task_id in self.preflight_retry_pending.drain() {
            if let Some(view) = self.execution_views.get_mut(&task_id) {
                view.scheduler_status = None;
                view.cancellation_requested = false;
            }
        }
    }

    fn rebind_scheduler_if_idle(&mut self) {
        if self.scheduler_driver.is_some() || !self.scheduler_snapshot.tasks.is_empty() {
            self.scheduler_rebind_pending = true;
            return;
        }
        let Some(store) = &self.store else {
            return;
        };
        let scheduler = Arc::new(ExecutionScheduler::new(
            self.relay.clone(),
            store.clone(),
            Arc::new(FakeRuntime::default()),
            ExecutionLimits::default(),
        ));
        self.scheduler_events = Some(scheduler.subscribe());
        self.scheduler = Some(scheduler);
        self.scheduler_snapshot = SchedulerSnapshot { tasks: Vec::new() };
        self.scheduler_rebind_pending = false;
    }

    async fn reap_finished_scheduler_driver(&mut self) {
        let Some(driver) = self.scheduler_driver.as_ref() else {
            return;
        };
        if !driver.is_finished() {
            return;
        }
        let driver = self.scheduler_driver.take().expect("checked above");
        match driver.await {
            Ok(Ok(reports)) => {
                for report in reports {
                    self.last_reports.insert(report.task_id, report);
                }
            }
            Ok(Err(error)) => self.feedback = format!("Scheduler driver failed: {error}"),
            Err(error) => self.feedback = format!("Scheduler driver stopped: {error}"),
        }
        self.refresh_durable_tasks().await;
    }

    async fn tick(&mut self) {
        self.reap_finished_scheduler_driver().await;
        let mut reload = false;
        let mut lagged = false;
        let mut drained = Vec::new();
        if let Some(events) = &mut self.scheduler_events {
            loop {
                match events.try_recv() {
                    Ok(event) => drained.push(event),
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        lagged = true;
                        break;
                    }
                    Err(broadcast::error::TryRecvError::Closed) => {
                        self.feedback = "Execution event stream closed.".to_owned();
                        break;
                    }
                }
            }
        }
        for event in drained {
            reload |= self.apply_execution_event(event);
        }
        if let Some(scheduler) = &self.scheduler {
            self.scheduler_snapshot = scheduler.snapshot().await;
        }
        if lagged {
            self.feedback = "Execution event stream lagged; refreshing canonical state.".to_owned();
            reload = true;
        }
        if reload {
            self.refresh_durable_tasks().await;
            self.clear_preflight_retry_state();
        }
        if self.scheduler_rebind_pending
            && self.scheduler_driver.is_none()
            && self.scheduler_snapshot.tasks.is_empty()
        {
            self.rebind_scheduler_if_idle();
        }
    }

    fn apply_execution_event(&mut self, event: CoreExecutionEvent) -> bool {
        match event {
            CoreExecutionEvent::ExecutionQueued { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .scheduler_status = Some(SchedulerTaskStatus::QueuedReady);
            }
            CoreExecutionEvent::SchedulerStatusChanged { task_id, status } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .scheduler_status = Some(status);
            }
            CoreExecutionEvent::ExecutionStarted { task_id, .. } => {
                let view = self.execution_views.entry(task_id).or_default();
                view.started_at = Some(Instant::now());
                view.durable_state = Some(TaskState::Running);
            }
            CoreExecutionEvent::RuntimeOutput {
                task_id,
                stream,
                bytes,
                ..
            } => self.append_output(task_id, stream, &bytes),
            CoreExecutionEvent::VerificationStarted { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .durable_state = Some(TaskState::Verifying)
            }
            CoreExecutionEvent::RepairStarted {
                task_id, attempt, ..
            } => {
                let view = self.execution_views.entry(task_id).or_default();
                view.durable_state = Some(TaskState::Repairing);
                view.repair_attempt = attempt;
            }
            CoreExecutionEvent::ClarificationNeeded { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .durable_state = Some(TaskState::NeedsClarification)
            }
            CoreExecutionEvent::DecisionNeeded { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .durable_state = Some(TaskState::NeedsDecision)
            }
            CoreExecutionEvent::ExecutionPersistenceFailed { task_id, error, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .persistence_error = Some(error);
                return true;
            }
            CoreExecutionEvent::ExecutionPreflightFailed { task_id, failure } => {
                self.feedback = format!("Execution preflight failed for {task_id}: {failure:?}");
                self.preflight_retry_pending.insert(task_id);
                return true;
            }
            CoreExecutionEvent::ExecutionFinished {
                task_id,
                final_state,
                ..
            } => {
                let view = self.execution_views.entry(task_id).or_default();
                view.cancellation_requested = false;
                view.durable_state = Some(final_state);
                return true;
            }
            CoreExecutionEvent::ExecutionCancelled { task_id, .. }
            | CoreExecutionEvent::TaskStateChanged { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .cancellation_requested = false;
                return true;
            }
            CoreExecutionEvent::ExecutionPreflightStarted { .. }
            | CoreExecutionEvent::RepairAttempted { .. } => {}
        }
        false
    }

    fn append_output(&mut self, task_id: TaskId, stream: RuntimeOutputStream, bytes: &[u8]) {
        let view = self.execution_views.entry(task_id).or_default();
        let safe: String = String::from_utf8_lossy(bytes)
            .chars()
            .map(|character| {
                if character == '\n' || character == '\t' || !character.is_control() {
                    character
                } else {
                    '�'
                }
            })
            .collect();
        let line = format!(
            "[{}] {}",
            match stream {
                RuntimeOutputStream::Stdout => "stdout",
                RuntimeOutputStream::Stderr => "stderr",
            },
            safe
        );
        view.output_bytes += line.len();
        view.output.push_back(line);
        while view.output_bytes > UI_OUTPUT_BYTE_LIMIT {
            if let Some(dropped) = view.output.pop_front() {
                view.output_bytes = view.output_bytes.saturating_sub(dropped.len());
            } else {
                break;
            }
        }
    }

    fn refresh_repository_inspections(&mut self) {
        self.repository_inspections = self
            .relay
            .list_projects()
            .filter_map(|project| {
                self.relay
                    .inspect_project_repository(project.key().as_str())
                    .ok()
                    .map(|inspection| (project.key().clone(), inspection))
            })
            .collect();
    }

    fn try_start_add_project(&mut self) {
        if self.registry_writable {
            self.start_add_project();
        } else {
            self.feedback =
                "Project registration is disabled until the config is fixed.".to_owned();
        }
    }

    fn start_add_project(&mut self) {
        self.screen = Screen::AddProject;
        self.add_project = AddProjectForm::default();
        self.add_field = AddProjectField::Key;
        self.feedback = "Enter trusted local project metadata.".to_owned();
    }

    fn start_new_task(&mut self) {
        self.screen = Screen::NewTask;
        self.task_candidate = None;
        self.feedback =
            "Choose a local ingress method. Import will not execute anything.".to_owned();
    }

    fn start_import_file(&mut self) {
        self.screen = Screen::ImportFile;
        self.import_file_path.clear();
        self.task_candidate = None;
        self.feedback = "Enter a .relay.json, .json, .relay.md, or .md path.".to_owned();
    }

    fn start_paste_task(&mut self) {
        self.screen = Screen::PasteTask;
        self.paste_text.clear();
        self.task_candidate = None;
        self.feedback = "Paste the task, then press Enter to validate.".to_owned();
    }

    fn submit_import_file(&mut self) {
        match self.relay.preview_local_file(self.import_file_path.trim()) {
            Ok(candidate) => {
                self.task_candidate = Some(candidate);
                self.feedback =
                    "Validated local task file. Confirm import to create a Relay task.".to_owned();
                self.screen = Screen::TaskPreview;
            }
            Err(error) => {
                self.task_candidate = None;
                self.feedback = error.to_string();
            }
        }
    }

    fn submit_paste_task(&mut self) {
        match self.relay.preview_local_paste(&self.paste_text) {
            Ok(candidate) => {
                self.task_candidate = Some(candidate);
                self.feedback =
                    "Validated pasted task. Confirm import to create a Relay task.".to_owned();
                self.screen = Screen::TaskPreview;
            }
            Err(error) => {
                self.task_candidate = None;
                self.feedback = error.to_string();
            }
        }
    }

    async fn confirm_task_import(&mut self) {
        let Some(store) = &self.store else {
            self.feedback = format!(
                "Task import is disabled until the database is fixed: {}",
                self.database_path.display()
            );
            return;
        };

        let Some(candidate) = self.task_candidate.take() else {
            self.feedback = "No validated task is ready to import.".to_owned();
            return;
        };

        match self
            .relay
            .confirm_task_import_with_store(store, candidate.clone())
            .await
        {
            Ok(stored) => {
                let task_id = stored.id.to_string();
                self.selected_task = Some(stored.id);
                self.durable_tasks.push(stored);
                self.feedback =
                    format!("Imported Relay task {task_id}. Execution has not started.");
                self.screen = Screen::TaskReview;
            }
            Err(error) => {
                self.task_candidate = Some(candidate);
                self.feedback = format!("Failed to persist Relay task: {error}");
            }
        }
    }

    fn cancel_task_import(&mut self) {
        self.task_candidate = None;
        self.feedback = "Task import canceled. No Relay task was created.".to_owned();
        self.screen = Screen::NewTask;
    }

    fn active_add_field_mut(&mut self) -> &mut String {
        match self.add_field {
            AddProjectField::Key => &mut self.add_project.key,
            AddProjectField::Path => &mut self.add_project.path,
            AddProjectField::ExpectedRemote => &mut self.add_project.expected_remote,
            AddProjectField::DefaultStack => &mut self.add_project.default_stack,
            AddProjectField::GitHubOwner => &mut self.add_project.github_owner,
            AddProjectField::GitHubRepo => &mut self.add_project.github_repo,
        }
    }

    fn submit_add_project(&mut self) -> Result<(), Box<dyn Error>> {
        if !self.registry_writable {
            self.feedback =
                "Project registration is disabled until the config is fixed.".to_owned();
            return Ok(());
        }

        let github = self.add_project.github_metadata()?;
        let request = RegisterProject {
            key: self.add_project.key.clone(),
            name: None,
            local_path: PathBuf::from(self.add_project.path.trim()),
            expected_remote: non_empty_field(&self.add_project.expected_remote),
            default_stack: non_empty_field(&self.add_project.default_stack),
            github,
        };
        let key = request.key.trim().to_owned();

        let mut registry = self.relay.project_registry().clone();
        let result = match registry.register(request) {
            Ok(_) => registry.save(&self.config_path),
            Err(error) => Err(error),
        };

        match result {
            Ok(()) => {
                self.relay = Relay::with_project_registry(registry);
                self.rebind_scheduler_if_idle();
                self.feedback = format!(
                    "Registered project '{key}' in {}",
                    self.config_path.display()
                );
                self.refresh_repository_inspections();
                self.screen = Screen::Projects;
            }
            Err(error) => {
                self.feedback = error.to_string();
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Dashboard,
    Tasks,
    Projects,
    AddProject,
    NewTask,
    ImportFile,
    PasteTask,
    TaskPreview,
    TaskReview,
    Execution,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddProjectField {
    Key,
    Path,
    ExpectedRemote,
    DefaultStack,
    GitHubOwner,
    GitHubRepo,
}

impl AddProjectField {
    fn next(self) -> Self {
        match self {
            Self::Key => Self::Path,
            Self::Path => Self::ExpectedRemote,
            Self::ExpectedRemote => Self::DefaultStack,
            Self::DefaultStack => Self::GitHubOwner,
            Self::GitHubOwner => Self::GitHubRepo,
            Self::GitHubRepo => Self::Key,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Key => Self::GitHubRepo,
            Self::Path => Self::Key,
            Self::ExpectedRemote => Self::Path,
            Self::DefaultStack => Self::ExpectedRemote,
            Self::GitHubOwner => Self::DefaultStack,
            Self::GitHubRepo => Self::GitHubOwner,
        }
    }
}

#[derive(Debug, Clone)]
struct AddProjectForm {
    key: String,
    path: String,
    expected_remote: String,
    default_stack: String,
    github_owner: String,
    github_repo: String,
}

impl AddProjectForm {
    fn github_metadata(&self) -> Result<Option<GitHubRepositoryMetadata>, Box<dyn Error>> {
        let owner = self.github_owner.trim();
        let repo = self.github_repo.trim();

        match (owner.is_empty(), repo.is_empty()) {
            (true, true) => Ok(None),
            (false, false) => Ok(Some(GitHubRepositoryMetadata {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
                default_labels: Vec::new(),
            })),
            _ => Err("GitHub owner and repo must either both be set or both be blank".into()),
        }
    }
}

impl Default for AddProjectForm {
    fn default() -> Self {
        Self {
            key: String::new(),
            path: std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            expected_remote: String::new(),
            default_stack: "chatgpt-codex".to_owned(),
            github_owner: String::new(),
            github_repo: String::new(),
        }
    }
}

fn non_empty_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::ProjectRegistry;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
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
        assert!(!help_text(&reopened).to_ascii_lowercase().contains("run"));
    }

    #[tokio::test]
    async fn multiline_json_paste_event_waits_for_explicit_validation() {
        let mut state = app_state_with_project();

        state.start_paste_task();
        handle_event(&mut state, Event::Paste(task_json()))
            .await
            .unwrap();

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
        handle_event(&mut state, Event::Paste(task_markdown()))
            .await
            .unwrap();

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
    async fn task_review_exposes_explicit_run_action() {
        let mut state = app_state_with_project_with_store().await;
        state.start_paste_task();
        state.paste_text = task_json();
        state.submit_paste_task();
        state.confirm_task_import().await;

        assert!(help_text(&state).contains("run"));
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

        AppState::new(
            Relay::with_project_registry(registry),
            unique_temp_path("config").join("relay.toml"),
            true,
            "ready".to_owned(),
        )
    }

    async fn app_state_with_project_with_store() -> AppState {
        let state = app_state_with_project();
        AppState::new_with_store(
            state.relay,
            state.config_path,
            state.registry_writable,
            unique_temp_path("state").join("relay.sqlite3"),
            Some(SqliteStore::in_memory().await.unwrap()),
            Vec::new(),
            "ready".to_owned(),
        )
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

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("relay-tui-{name}-{nanos}"))
    }
}
