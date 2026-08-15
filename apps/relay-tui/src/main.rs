mod command;
mod command_completion;
mod project_discovery;

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    io::{self, Stdout},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use command::{RelayCommand, parse_relay_command};
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use project_discovery::{DiscoveredProject, discover_project};
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
mod events;
mod theme;

use relay_runtime::{
    CancellationToken, Runtime, RuntimeCapabilities, RuntimeIdentity, RuntimeOutputStream,
    RuntimeRequest, RuntimeStartFuture, sanitize_terminal_output,
};
use relay_runtime_antigravity::{AntigravityRuntime, AntigravityRuntimeHealth};
use relay_runtime_codex::{CodexRuntime, CodexRuntimeHealth};
use relay_runtime_fake::FakeRuntime;
use theme::*;
use tokio::process::Command as TokioCommand;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::timeout;

type Tui = Terminal<CrosstermBackend<Stdout>>;

const CODEX_DEFAULT_MAX_DURATION_SECS: u64 = 30 * 60;
const CODEX_MAX_DURATION_SECS: u64 = 4 * 60 * 60;

fn parse_codex_duration(value: Option<&str>) -> Result<Duration, String> {
    let seconds = match value {
        None => CODEX_DEFAULT_MAX_DURATION_SECS,
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "RELAY_CODEX_MAX_DURATION_SECS must be a positive integer".to_owned())?,
    };
    if seconds == 0 || seconds > CODEX_MAX_DURATION_SECS {
        return Err(format!(
            "RELAY_CODEX_MAX_DURATION_SECS must be between 1 and {CODEX_MAX_DURATION_SECS}"
        ));
    }
    Ok(Duration::from_secs(seconds))
}

#[cfg(not(test))]
fn production_codex_execution_limits() -> ExecutionLimits {
    ExecutionLimits {
        max_duration: parse_codex_duration(
            std::env::var("RELAY_CODEX_MAX_DURATION_SECS")
                .ok()
                .as_deref(),
        )
        .unwrap_or_else(|_| Duration::from_secs(CODEX_DEFAULT_MAX_DURATION_SECS)),
        ..ExecutionLimits::default()
    }
}

fn app_execution_limits() -> ExecutionLimits {
    #[cfg(test)]
    {
        ExecutionLimits::default()
    }
    #[cfg(not(test))]
    {
        production_codex_execution_limits()
    }
}

async fn probe_version(executable: &str, args: &[&str]) -> Result<String, String> {
    let output = timeout(Duration::from_secs(3), {
        let mut command = TokioCommand::new(executable);
        command.args(args).kill_on_drop(true).output()
    })
    .await
    .map_err(|_| format!("{executable} probe timed out"))?
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("{executable} exited with {}", output.status));
    }
    let text = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    Ok(if text.is_empty() {
        "available".to_owned()
    } else {
        text.chars().take(160).collect()
    })
}

async fn collect_health(active_provider: String, full_probe: bool) -> HealthSnapshot {
    let git_health = match probe_version("git", &["--version"]).await {
        Ok(version) => ToolHealth::Available(version),
        Err(error) => ToolHealth::Unavailable(error),
    };
    let runtime_health = if full_probe || active_provider == "codex" {
        CodexRuntime::default().discover().await
    } else {
        CodexRuntimeHealth::Checking
    };
    let antigravity_health = if full_probe || active_provider == "antigravity" {
        AntigravityRuntime::default().discover().await
    } else {
        AntigravityRuntimeHealth::Checking
    };
    let github_cli_health = match probe_version("gh", &["--version"]).await {
        Ok(version) => {
            let auth = timeout(Duration::from_secs(3), {
                let mut command = TokioCommand::new("gh");
                command
                    .args(["auth", "status", "--active", "--hostname", "github.com"])
                    .kill_on_drop(true)
                    .output()
            })
            .await;
            match auth {
                Ok(Ok(output)) if output.status.success() => {
                    format!("available and authenticated ({version})")
                }
                Ok(Ok(_)) => format!("available but not authenticated ({version})"),
                Ok(Err(error)) => format!("available but auth check failed ({error})"),
                Err(_) => format!("available but auth check timed out ({version})"),
            }
        }
        Err(_) => "not installed (optional M10 prerequisite)".to_owned(),
    };
    HealthSnapshot {
        git_health,
        runtime_health,
        antigravity_health,
        github_cli_health,
    }
}

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
    state.begin_health_refresh(false);
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
    events::run_event_loop(terminal, state).await
}

fn render(frame: &mut Frame, state: &AppState) {
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

fn render_dashboard(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = activity_home(state);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

fn render_tasks(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = task_selection_listing(state);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

fn render_projects(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = project_listing(&state.relay, &state.repository_inspections, &state.feedback);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

fn render_runtimes(frame: &mut Frame, state: &AppState, area: Rect) {
    let mut lines = vec!["Runtimes\n".to_owned()];

    // Codex
    let codex_marker = if state.runtime_health.is_ready() {
        "●"
    } else {
        "○"
    };
    let codex_selected = if state.active_runtime_provider == "codex" {
        "> "
    } else {
        "  "
    };
    lines.push(format!("{codex_selected}Codex CLI"));
    lines.push(format!(
        "  {codex_marker} {}",
        state.runtime_health.display_status()
    ));
    lines.push(format!(
        "  Profile: {}",
        if state.active_runtime_provider == "codex" {
            &state.active_profile_id
        } else {
            "codex/implement"
        }
    ));
    lines.push(String::new());

    // Antigravity
    let agy_marker = if state.antigravity_health.is_ready() {
        "●"
    } else {
        "○"
    };
    let agy_selected = if state.active_runtime_provider == "antigravity" {
        "> "
    } else {
        "  "
    };
    lines.push(format!("{agy_selected}Antigravity"));
    lines.push(format!(
        "  {agy_marker} {}",
        state.antigravity_health.display_status()
    ));
    lines.push(format!(
        "  Profile: {}",
        if state.active_runtime_provider == "antigravity" {
            &state.active_profile_id
        } else {
            "antigravity/implement"
        }
    ));
    lines.push(String::new());

    lines.push(format!("\nDefault: {}", state.active_profile_id));

    let body = lines.join("\n");
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

fn render_runtime_detail(frame: &mut Frame, state: &AppState, area: Rect) {
    let provider = state.runtime_detail_provider.as_deref().unwrap_or("codex");
    let body = match provider {
        "codex" => {
            let health_detail =
                codex_health_detail(&state.runtime_health, &CodexRuntime::default());
            format!(
                "Runtime / Codex\n\nStatus\n{}\n\nActive profile\n  {}\n\nAI\n  Model: Default\n  Reasoning: High\n\nCapabilities\n  Workspace read: ✓\n  Workspace write: ✓\n  Commands: ✓\n  Network: ×\n  Commit: ×\n  Push: ×\n\nLimits\n  Duration: {} min\n  Repairs: {}",
                health_detail,
                state.active_profile_id,
                app_execution_limits().max_duration.as_secs() / 60,
                app_execution_limits().max_repair_attempts
            )
        }
        "antigravity" => {
            format!(
                "Runtime / Antigravity\n\nStatus\n  {}\n\nActive profile\n  antigravity/implement\n\nAI\n  Model: Default\n  Effort: High\n\nCapabilities\n  Workspace read: ✓\n  Workspace write: ✓\n  Commands: ✓\n  Network: ×\n  Commit: ×\n  Push: ×",
                state.antigravity_health.display_status()
            )
        }
        _ => format!("Unknown runtime provider: {provider}"),
    };
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

fn render_diff(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let lines = match state.diff_output.as_deref() {
        Some(diff_text) if !diff_text.is_empty() => format_diff_lines(diff_text, &theme),
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

fn render_logs(frame: &mut Frame, state: &AppState, area: Rect) {
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

fn render_settings(frame: &mut Frame, state: &AppState, area: Rect) {
    let limits = app_execution_limits();
    let body = format!(
        "Settings\n\nExecution\n  Global parallelism: {}\n  Per-project parallelism: {}\n  Repair attempts: {}\n  Maximum execution: {} minutes\n\nSafety\n  Explicit Run approval: On\n  Automatic commit: Off\n  Automatic push: Off\n\nStorage\n  Config: {}\n  SQLite: {}\n\nRuntime\n  Provider: {}\n  Profile: {}\n  Status: {}",
        limits.max_parallel,
        limits.per_project_max_parallel,
        limits.max_repair_attempts,
        limits.max_duration.as_secs() / 60,
        state.config_path.display(),
        state.database_path.display(),
        state.active_runtime_provider,
        state.active_profile_id,
        state.runtime_health.display_status()
    );
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: true }), area);
}

fn render_doctor(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = format!(
        "Relay Doctor\n\nCurrent execution requirements\n\n{}\n{} Relay config\n{} SQLite\n{}\n  {}\n{}\n  {}\n\nFuture integrations\n{}\nGitHub Issue ingress: not implemented until M10\nRelay Direct / MCP: not implemented until M12\n\nNo secrets or environment data are displayed.",
        git_health_marker(&state.git_health),
        if state.registry_writable { "✓" } else { "×" },
        if state.store.is_some() { "✓" } else { "×" },
        if state.runtime_health.is_ready() {
            "✓ Codex CLI"
        } else {
            "× Codex CLI"
        },
        codex_health_detail(&state.runtime_health, &CodexRuntime::default()),
        if state.antigravity_health.is_ready() {
            "✓ Antigravity"
        } else {
            "× Antigravity"
        },
        state.antigravity_health.display_status(),
        state.github_cli_health
    );
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: true }), area);
}

fn codex_health_detail(health: &CodexRuntimeHealth, runtime: &CodexRuntime) -> String {
    let executable = runtime
        .resolve_executable()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "no supported executable resolved".to_owned());
    match health {
        CodexRuntimeHealth::Checking => {
            "  health probe pending; Run remains blocked until verification completes".to_owned()
        }
        CodexRuntimeHealth::Available { version } => {
            format!("  {executable}\n  {version}\n✓ Codex authentication and exec compatibility")
        }
        CodexRuntimeHealth::NotAuthenticated { version, error } => format!(
            "  {executable}\n  {}\n  authentication unavailable: {error}",
            version.as_deref().unwrap_or("version unavailable")
        ),
        CodexRuntimeHealth::Incompatible { reason, .. } => {
            format!("  {executable}\n  compatibility unavailable: {reason}")
        }
        CodexRuntimeHealth::NotInstalled { error }
        | CodexRuntimeHealth::DiscoveryFailed { error } => format!("  {executable}\n  {error}"),
    }
}

fn render_add_project(frame: &mut Frame, state: &AppState, area: Rect) {
    let form = &state.add_project;
    if let Some(discovered) = &state.discovered_project {
        let body = format!(
            "Add Project\n\nDetected repository\n\nGit root   {}\nOrigin     {}\n\nWill register\nKey        {}\nPath       {}\nRemote     {}\nGitHub     {}/{}\nStack      {}\n\n✓ Git worktree detected\n{}\n\nPress Enter to add this repository. Tab/arrow keys edit details. Esc cancels.",
            discovered.repository_root.display(),
            discovered.remote.as_deref().unwrap_or("not configured"),
            form.key,
            form.path,
            form.expected_remote,
            form.github_owner,
            form.github_repo,
            form.default_stack,
            state.feedback
        );
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
        return;
    }
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
        "New Task\n\nImport a user-approved Relay handoff.\n\n{}\n{}\n\nUse Up/Down to navigate, Enter to select, Esc to cancel.\n\n{}",
        option_paste, option_file, state.feedback
    );
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
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
            } else if matches!(
                task.state,
                TaskState::Done | TaskState::Failed | TaskState::Cancelled
            ) {
                "\n\n[Enter] View result  [B] Back"
            } else if matches!(
                task.state,
                TaskState::NeedsClarification | TaskState::NeedsDecision
            ) {
                "\n\nATTENTION REQUIRED: external resolution is required.\n[B] Back"
            } else {
                "\n\n[B] Back"
            };
            let requested_stack = task.handoff.stack.as_deref().unwrap_or("not requested");
            let effective_stack = state
                .effective_stack(task)
                .unwrap_or("missing or unsupported");
            format!(
                "Task: {}\nProject: {}\nRequested stack: {}\nEffective stack: {}\nRuntime profile: {}\nRuntime availability: {}\nState: {}\nRepository: {}\n\n{}{}",
                task.id,
                task.project_key,
                requested_stack,
                effective_stack,
                AppRuntime::default().identity().profile_id,
                state.runtime_health.display_status(),
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
                action
            )
        })
        .unwrap_or_else(|| "No durable task selected.".to_owned());
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

fn render_execution(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state.selected_task.and_then(|id| state.execution_views.get(&id).map(|view| (id, view))).map(|(id, view)| {
        let task = state.durable_tasks.iter().find(|task| task.id == id);
        let output = view.output.iter().cloned().collect::<Vec<_>>().join("");
        format!("Task: {id}\nProject: {}\nRuntime: {}\nScheduler: {}\nPhase: {}\nRepair: {}\nElapsed: {}s\n{}\n\nOutput:\n{}\n\nTry:\n  /cancel\n  /diff\n  /logs\n  /tasks",
            task.map(|task| task.project_key.as_str()).unwrap_or("unknown"),
            state.runtime_label(),
            view.scheduler_status.as_ref().map(scheduler_status_text).unwrap_or("not scheduled"),
            view.durable_state.map(|state| state.as_str()).unwrap_or("state pending"), view.repair_attempt,
            view.started_at.map(|started| started.elapsed().as_secs()).unwrap_or(0),
            view.persistence_error.as_ref().map(|error| format!("STATE UNCERTAIN / DURABILITY FAILURE\n{error}")).unwrap_or_else(|| if view.cancellation_requested { "Cancellation requested...".to_owned() } else { String::new() }), output)
    }).unwrap_or_else(|| "No execution selected.".to_owned());
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

fn render_result(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = state.selected_stored_task().map(|task| {
        let report = state.last_reports.get(&task.id);
        let repository = state.relay.inspect_project_repository(task.project_key.as_str()).ok().map(|i| repository_inspection_line(&i.eligibility, &i.status)).unwrap_or_else(|| "unavailable".to_owned());
        format!("Task: {}\nProject: {}\nRuntime: {}\nState: {}\nOutcome: {}\nVerification: {}\nRepairs: {}\nRepository: {}\n\nSummary:\n{}\n\nTry:\n  /diff\n  /logs\n  /tasks\n  /new",
            task.id, task.project_key, state.runtime_label(), task.state.as_str(), report.and_then(|r| r.outcome).map(|o| o.as_str()).unwrap_or("not confirmed"),
            if task.state == TaskState::Done { "PASS" } else { "not available" }, state.execution_views.get(&task.id).map(|v| v.repair_attempt).unwrap_or(0), repository,
            report.and_then(|r| r.outcome_summary.as_deref()).unwrap_or("No confirmed execution report."))
    }).unwrap_or_else(|| "No durable task selected.".to_owned());
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

async fn handle_key(state: &mut AppState, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
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
        Screen::Diff => match key.code {
            KeyCode::Up => {
                state.diff_scroll = state.diff_scroll.saturating_sub(1);
                Ok(false)
            }
            KeyCode::Down => {
                state.diff_scroll = state.diff_scroll.saturating_add(1);
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
            KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                state.screen = Screen::Dashboard;
                Ok(false)
            }
            _ => Ok(false),
        },
        Screen::Runtimes | Screen::RuntimeDetail | Screen::Settings | Screen::Doctor => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('D') => {
                    state.screen = Screen::Dashboard;
                    Ok(false)
                }
                _ => Ok(false),
            }
        }
    }
}

fn command_mode_available(screen: Screen) -> bool {
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

async fn execute_relay_command(state: &mut AppState, input: &str) -> Result<bool, Box<dyn Error>> {
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
        RelayCommand::Help => {
            state.screen = Screen::Dashboard;
            state.feedback = command::COMMANDS.join("\n");
        }
        RelayCommand::Quit => return Ok(true),
        RelayCommand::Run { task } => state.run_task_selector(&task).await,
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
        RelayCommand::RuntimeUse { profile } => {
            if profile.starts_with("antigravity/") {
                state.active_runtime_provider = "antigravity".to_owned();
                state.active_profile_id = profile.clone();
            } else {
                state.active_runtime_provider = "codex".to_owned();
                state.active_profile_id = profile.clone();
            }
        }
        RelayCommand::RuntimeTest { .. } => state.begin_health_refresh(true),
        RelayCommand::Diff { task } => {
            if let Some(t) = task {
                state.select_task_by_selector(&t);
            }
            let diff = match state.selected_stored_task() {
                Some(task) => match state.relay.resolve_project(task.project_key.as_str()) {
                    Ok(project) => {
                        match std::process::Command::new("git")
                            .arg("diff")
                            .current_dir(project.local_path())
                            .output()
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
            state.screen = Screen::Logs;
        }
    }
    Ok(false)
}

#[cfg(test)]
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

fn activity_home(state: &AppState) -> String {
    if state.relay.configured_project_count() == 0 {
        return format!(
            "Relay                                      {}\n\nWelcome to Relay.\n\nNo projects are configured yet.\nRelay needs a trusted local Git repository before it can run implementation tasks.\n\nTry:\n  /project add\n  /help\n\n{}",
            state.runtime_health.display_status(),
            state.feedback
        );
    }
    let active = state
        .scheduler_snapshot
        .tasks
        .iter()
        .filter(|task| matches!(task.status, SchedulerTaskStatus::Running))
        .count();
    let waiting = state
        .scheduler_snapshot
        .tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                SchedulerTaskStatus::QueuedGlobal | SchedulerTaskStatus::QueuedProject
            )
        })
        .count();
    let attention = state
        .durable_tasks
        .iter()
        .filter(|task| {
            matches!(
                task.state,
                TaskState::NeedsClarification | TaskState::NeedsDecision
            )
        })
        .count();
    let mut lines = vec![format!(
        "Relay                                      {}",
        state.runtime_health.display_status()
    )];
    if active + waiting + attention == 0 {
        lines.push("\nNo active tasks.".to_owned());
    } else {
        lines.push(format!(
            "\n{active} active · {waiting} waiting · {attention} needs attention"
        ));
    }
    for task in state
        .durable_tasks
        .iter()
        .filter(|task| !matches!(task.state, TaskState::Done | TaskState::Cancelled))
        .take(6)
    {
        lines.push(format!(
            "\n{} {}\n  {}",
            if matches!(
                task.state,
                TaskState::NeedsClarification | TaskState::NeedsDecision
            ) {
                "!"
            } else {
                "●"
            },
            task.project_key,
            task.handoff.title
        ));
        lines.push(format!("  {}", task.state.as_str().to_uppercase()));
    }
    lines.push(format!("\n{}", state.feedback));
    lines.join("\n")
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

/// TUI-local production/test selection. Core remains generic over `Runtime`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
enum AppRuntime {
    Codex(CodexRuntime),
    Antigravity(AntigravityRuntime),
    Fake(FakeRuntime),
}

impl Default for AppRuntime {
    fn default() -> Self {
        #[cfg(test)]
        {
            Self::Fake(FakeRuntime::default())
        }
        #[cfg(not(test))]
        {
            Self::Codex(CodexRuntime::default())
        }
    }
}

impl Runtime for AppRuntime {
    fn identity(&self) -> RuntimeIdentity {
        match self {
            Self::Codex(runtime) => runtime.identity(),
            Self::Antigravity(runtime) => runtime.identity(),
            Self::Fake(runtime) => runtime.identity(),
        }
    }
    fn capabilities(&self) -> RuntimeCapabilities {
        match self {
            Self::Codex(runtime) => runtime.capabilities(),
            Self::Antigravity(runtime) => runtime.capabilities(),
            Self::Fake(runtime) => runtime.capabilities(),
        }
    }
    fn start(
        &self,
        request: RuntimeRequest,
        cancellation: CancellationToken,
    ) -> RuntimeStartFuture {
        match self {
            Self::Codex(runtime) => runtime.start(request, cancellation),
            Self::Antigravity(runtime) => runtime.start(request, cancellation),
            Self::Fake(runtime) => runtime.start(request, cancellation),
        }
    }
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
    scheduler: Option<Arc<ExecutionScheduler<AppRuntime>>>,
    runtime_health: CodexRuntimeHealth,
    git_health: ToolHealth,
    github_cli_health: String,
    antigravity_health: AntigravityRuntimeHealth,
    active_runtime_provider: String,
    active_profile_id: String,
    runtime_detail_provider: Option<String>,
    diff_output: Option<String>,
    diff_scroll: u16,
    logs_scroll: u16,
    health_refresh: Option<JoinHandle<HealthSnapshot>>,
    command_input: String,
    command_mode: bool,
    command_selected_index: usize,
    #[allow(dead_code)]
    command_scroll_offset: usize,
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
    discovered_project: Option<DiscoveredProject>,
    add_field: AddProjectField,
    import_file_path: String,
    paste_text: String,
    task_candidate: Option<TaskImportCandidate>,
    new_task_option: usize,
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

#[derive(Debug)]
struct HealthSnapshot {
    git_health: ToolHealth,
    runtime_health: CodexRuntimeHealth,
    antigravity_health: AntigravityRuntimeHealth,
    github_cli_health: String,
}

const UI_OUTPUT_BYTE_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolHealth {
    Checking,
    Available(String),
    Unavailable(String),
}

fn git_health_marker(health: &ToolHealth) -> &'static str {
    match health {
        ToolHealth::Checking => "… Git — checking",
        ToolHealth::Available(_) => "✓ Git",
        ToolHealth::Unavailable(_) => "× Git",
    }
}

impl AppState {
    fn begin_health_refresh(&mut self, full_probe: bool) {
        if let Some(task) = self.health_refresh.take() {
            task.abort();
        }
        if full_probe || self.active_runtime_provider == "codex" {
            self.runtime_health = CodexRuntimeHealth::Checking;
        }
        if full_probe || self.active_runtime_provider == "antigravity" {
            self.antigravity_health = AntigravityRuntimeHealth::Checking;
        }
        self.git_health = ToolHealth::Checking;
        self.github_cli_health = "checking (optional M10 prerequisite)".to_owned();
        let provider = self.active_runtime_provider.clone();
        self.health_refresh = Some(tokio::spawn(collect_health(provider, full_probe)));
    }

    async fn poll_health_refresh(&mut self) {
        let Some(task) = self.health_refresh.as_ref() else {
            return;
        };
        if !task.is_finished() {
            return;
        }
        let task = self.health_refresh.take().expect("health task exists");
        if let Ok(snapshot) = task.await {
            self.git_health = snapshot.git_health;
            if snapshot.runtime_health != CodexRuntimeHealth::Checking {
                self.runtime_health = snapshot.runtime_health;
            }
            if snapshot.antigravity_health != AntigravityRuntimeHealth::Checking {
                self.antigravity_health = snapshot.antigravity_health;
            }
            self.github_cli_health = snapshot.github_cli_health;
        }
    }

    fn start_detected_project(&mut self, path: Option<PathBuf>) {
        let path = path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        match discover_project(&path) {
            Ok(discovered) => {
                self.start_add_project();
                self.add_project.path = discovered.repository_root.display().to_string();
                self.add_project.key = discovered.suggested_key.clone();
                self.add_project.expected_remote = discovered.remote.clone().unwrap_or_default();
                self.add_project.default_stack = discovered
                    .suggested_stack
                    .clone()
                    .unwrap_or_else(|| "chatgpt-codex".to_owned());
                self.add_project.github_owner = discovered.github_owner.clone().unwrap_or_default();
                self.add_project.github_repo = discovered.github_repo.clone().unwrap_or_default();
                self.discovered_project = Some(discovered);
                self.feedback = "Detected repository. Review the suggested values and press Enter to add it, or edit details first.".to_owned();
            }
            Err(error) => {
                self.start_add_project();
                self.feedback = format!(
                    "Repository detection failed: {error}\nAdvanced setup is available for manual registration."
                );
            }
        }
    }

    fn select_task_by_selector(&mut self, selector: &str) -> bool {
        let matches = self
            .durable_tasks
            .iter()
            .filter(|task| task.id.to_string().starts_with(selector))
            .map(|task| task.id)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [task_id] => {
                self.selected_task = Some(*task_id);
                true
            }
            [] => {
                self.feedback = "No Relay task matches that selector.".to_owned();
                false
            }
            _ => {
                self.feedback =
                    "Task selector is ambiguous; provide more of the TaskId.".to_owned();
                false
            }
        }
    }

    async fn run_task_selector(&mut self, selector: &str) {
        if self.select_task_by_selector(selector) {
            self.submit_selected_task().await;
        }
    }

    fn build_completion_context(&self) -> command_completion::CompletionContext {
        let providers = vec![
            command_completion::ProviderCompletion {
                provider_id: "codex".to_owned(),
                display_name: "Codex".to_owned(),
                status_note: self.runtime_health.display_status().to_owned(),
            },
            command_completion::ProviderCompletion {
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity".to_owned(),
                status_note: self.antigravity_health.display_status().to_owned(),
            },
        ];

        let profiles = vec![
            command_completion::ProfileCompletion {
                profile_id: "codex/implement".to_owned(),
                provider_id: "codex".to_owned(),
                display_name: "Codex / Implement".to_owned(),
                description: "Code generation and workspace edits".to_owned(),
                available: true,
                disabled_reason: None,
            },
            command_completion::ProfileCompletion {
                profile_id: "codex/review".to_owned(),
                provider_id: "codex".to_owned(),
                display_name: "Codex / Review".to_owned(),
                description: "Read-only inspection & review".to_owned(),
                available: true,
                disabled_reason: None,
            },
            command_completion::ProfileCompletion {
                profile_id: "codex/deep".to_owned(),
                provider_id: "codex".to_owned(),
                display_name: "Codex / Deep".to_owned(),
                description: "High reasoning effort agent turns".to_owned(),
                available: true,
                disabled_reason: None,
            },
            command_completion::ProfileCompletion {
                profile_id: "antigravity/implement".to_owned(),
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity / Implement".to_owned(),
                description: "Google Antigravity autonomous edits".to_owned(),
                available: true,
                disabled_reason: None,
            },
            command_completion::ProfileCompletion {
                profile_id: "antigravity/review".to_owned(),
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity / Review".to_owned(),
                description: "Read-only planning & inspection mode".to_owned(),
                available: true,
                disabled_reason: None,
            },
            command_completion::ProfileCompletion {
                profile_id: "antigravity/deep".to_owned(),
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity / Deep".to_owned(),
                description: "High effort agent turns".to_owned(),
                available: true,
                disabled_reason: None,
            },
        ];

        let mut ready_tasks = Vec::new();
        let mut cancellable_tasks = Vec::new();
        let mut inspectable_tasks = Vec::new();

        for task in &self.durable_tasks {
            let id_str = task.id.to_string();
            let short_id = if id_str.len() >= 8 {
                id_str[..8].to_owned()
            } else {
                id_str.clone()
            };

            let item = command_completion::TaskCompletion {
                id_str: id_str.clone(),
                short_id,
                project_key: task.project_key.to_string(),
                title: task.handoff.title.clone(),
                state: task.state,
                eligible_for_run: task.state == TaskState::Ready,
            };

            if task.state == TaskState::Ready {
                ready_tasks.push(item.clone());
            }
            if matches!(task.state, TaskState::Running | TaskState::Verifying)
                || self
                    .execution_views
                    .get(&task.id)
                    .map(|v| v.scheduler_status.is_some())
                    .unwrap_or(false)
            {
                cancellable_tasks.push(item.clone());
            }
            inspectable_tasks.push(item);
        }

        command_completion::CompletionContext {
            providers,
            profiles,
            ready_tasks,
            cancellable_tasks,
            inspectable_tasks,
        }
    }

    fn get_current_suggestions(&self) -> Vec<command_completion::CommandSuggestion> {
        let ctx = self.build_completion_context();
        command_completion::compute_suggestions(&self.command_input, &ctx)
    }

    fn cancel_task_selector(&mut self, selector: &str) {
        let matches = self
            .durable_tasks
            .iter()
            .filter(|task| task.id.to_string().starts_with(selector))
            .map(|task| task.id)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [task_id] => {
                self.selected_task = Some(*task_id);
                self.cancel_confirmation = Some(*task_id);
                self.screen = Screen::Execution;
                self.feedback = "Cancel execution? Press Y to confirm.".to_owned();
            }
            [] => self.feedback = "No Relay task matches that selector.".to_owned(),
            _ => {
                self.feedback = "Task selector is ambiguous; provide more of the TaskId.".to_owned()
            }
        }
    }
    fn runtime_label(&self) -> String {
        let identity = AppRuntime::default().identity();
        format!(
            "{}\nProfile: {}\nStatus: {}",
            identity.display_name,
            identity.profile_id,
            self.runtime_health.display_status()
        )
    }
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
                Arc::new(AppRuntime::default()),
                app_execution_limits(),
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
            runtime_health: CodexRuntimeHealth::Checking,
            git_health: ToolHealth::Checking,
            github_cli_health: "not checked (optional M10 prerequisite)".to_owned(),
            antigravity_health: AntigravityRuntimeHealth::Checking,
            active_runtime_provider: "codex".to_owned(),
            active_profile_id: "codex/implement".to_owned(),
            runtime_detail_provider: None,
            diff_output: None,
            diff_scroll: 0,
            logs_scroll: 0,
            health_refresh: None,
            command_input: String::new(),
            command_mode: false,
            command_selected_index: 0,
            command_scroll_offset: 0,
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
            discovered_project: None,
            add_field: AddProjectField::Key,
            import_file_path: String::new(),
            paste_text: String::new(),
            task_candidate: None,
            new_task_option: 0,
            repository_inspections: BTreeMap::new(),
            feedback,
        }
    }

    fn handle_paste(&mut self, text: &str) {
        if self.command_mode {
            self.command_input.push_str(text);
        } else if self.screen == Screen::PasteTask {
            self.paste_text.push_str(text);
        }
    }

    fn has_active_execution(&self) -> bool {
        self.scheduler_driver.is_some() || matches!(self.screen, Screen::Execution)
    }

    pub fn input_context(&self) -> events::InputContext {
        if self.command_mode {
            events::InputContext::CommandPrompt
        } else {
            match self.screen {
                Screen::PasteTask | Screen::ImportFile => events::InputContext::TextEditor,
                Screen::AddProject => {
                    if self.discovered_project.is_some() {
                        events::InputContext::Selection
                    } else {
                        events::InputContext::TextEditor
                    }
                }
                Screen::Tasks
                | Screen::Projects
                | Screen::NewTask
                | Screen::Runtimes
                | Screen::RuntimeDetail => events::InputContext::Selection,
                Screen::TaskReview => events::InputContext::Confirmation,
                Screen::Dashboard
                | Screen::TaskPreview
                | Screen::Execution
                | Screen::Result
                | Screen::Diff
                | Screen::Logs
                | Screen::Settings
                | Screen::Doctor => events::InputContext::PassiveView,
            }
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
            return "unavailable: local runtime or store is unavailable".to_owned();
        }
        if !self.runtime_health.is_ready() {
            return format!("unavailable: {}", self.runtime_health.display_status());
        }
        match self.effective_stack(task) {
            Ok("chatgpt-codex") => {}
            Ok(stack) => {
                return format!("unavailable: unsupported runtime stack '{stack}'");
            }
            Err(error) => return format!("unavailable: {error}"),
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

    fn effective_stack<'a>(&'a self, task: &'a StoredTask) -> Result<&'a str, String> {
        let project = self
            .relay
            .resolve_project(task.project_key.as_str())
            .map_err(|_| "project is not registered".to_owned())?;
        task.handoff
            .stack
            .as_deref()
            .or_else(|| project.default_stack())
            .ok_or_else(|| "no runtime stack is configured".to_owned())
    }

    async fn submit_selected_task(&mut self) {
        self.reap_finished_scheduler_driver().await;
        if self.scheduler_rebind_pending {
            self.rebind_scheduler_if_idle();
        }
        if self.scheduler_rebind_pending {
            self.feedback =
                "Scheduler refresh pending; new Run approvals resume after active work is idle."
                    .to_owned();
            return;
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
        let max_duration = std::cmp::min(
            std::time::Duration::from_secs(1800),
            scheduler.limits().max_duration,
        );
        let max_repair_attempts = std::cmp::min(2, scheduler.limits().max_repair_attempts);
        let policy = relay_core::ExecutionRunPolicy {
            runtime_profile_id: self.active_profile_id.clone(),
            max_duration,
            max_repair_attempts,
        };
        match scheduler.submit_with_policy(task.id, policy).await {
            Ok(()) => {
                self.execution_views
                    .entry(task.id)
                    .or_default()
                    .scheduler_status = Some(SchedulerTaskStatus::QueuedReady);
                self.feedback = "Run approved. Queued for the scheduler.".to_owned();
                self.screen = Screen::Execution;
                self.ensure_scheduler_driver(scheduler).await;
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
            Arc::new(AppRuntime::default()),
            app_execution_limits(),
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

    async fn ensure_scheduler_driver(&mut self, scheduler: Arc<ExecutionScheduler<AppRuntime>>) {
        self.reap_finished_scheduler_driver().await;
        self.scheduler_snapshot = scheduler.snapshot().await;
        self.start_scheduler_driver_if_queued(scheduler);
    }

    fn start_scheduler_driver_if_queued(&mut self, scheduler: Arc<ExecutionScheduler<AppRuntime>>) {
        let has_queued_work = self.scheduler_snapshot.tasks.iter().any(|task| {
            matches!(
                task.status,
                SchedulerTaskStatus::QueuedReady
                    | SchedulerTaskStatus::QueuedGlobal
                    | SchedulerTaskStatus::QueuedProject
            )
        });
        if self.scheduler_driver.is_none() && !self.scheduler_rebind_pending && has_queued_work {
            self.scheduler_driver = Some(tokio::spawn(
                async move { scheduler.run_until_idle().await },
            ));
        }
    }

    async fn tick(&mut self) {
        self.poll_health_refresh().await;
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
        if let Some(scheduler) = self.scheduler.clone() {
            self.start_scheduler_driver_if_queued(scheduler);
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
        self.discovered_project = None;
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
    Runtimes,
    RuntimeDetail,
    Diff,
    Logs,
    Settings,
    Doctor,
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
        process::Command,
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
            std::future::pending::<Result<Vec<ExecutionReport>, relay_core::ExecutionSchedulerError>>().await
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
        assert!(state.feedback.contains("Scheduler refresh pending"));
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

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("relay-tui-{name}-{nanos}"))
    }
}
