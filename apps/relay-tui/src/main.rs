use std::{
    error::Error,
    io::{self, Stdout},
    path::PathBuf,
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use relay_core::{GitHubRepositoryMetadata, RegisterProject, Relay, default_config_path};

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> Result<(), Box<dyn Error>> {
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
    let mut state = AppState::new(relay, config_path, registry_writable, feedback);
    let mut terminal = initialize_terminal()?;

    let run_result = run(&mut terminal, &mut state);
    let restore_result = restore_terminal(&mut terminal);

    run_result?;
    restore_result?;
    Ok(())
}

fn initialize_terminal() -> Result<Tui, Box<dyn Error>> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error.into());
    }

    let backend = CrosstermBackend::new(stdout);
    match Terminal::new(backend) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            Err(error.into())
        }
    }
}

fn restore_terminal(terminal: &mut Tui) -> Result<(), Box<dyn Error>> {
    let raw_mode_result = disable_raw_mode();
    let alternate_screen_result = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor_result = terminal.show_cursor();

    raw_mode_result?;
    alternate_screen_result?;
    cursor_result?;
    Ok(())
}

fn run(terminal: &mut Tui, state: &mut AppState) -> Result<(), Box<dyn Error>> {
    loop {
        terminal.draw(|frame| render(frame, state))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && handle_key(state, key)? {
                    return Ok(());
                }
            }
        }
    }
}

fn render(frame: &mut Frame, state: &AppState) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(frame.area());

    match state.screen {
        Screen::Dashboard => render_dashboard(frame, state, areas[0]),
        Screen::Projects => render_projects(frame, state, areas[0]),
        Screen::AddProject => render_add_project(frame, state, areas[0]),
    }

    let help = Paragraph::new(help_text(state));
    frame.render_widget(help, areas[1]);
}

fn render_dashboard(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = format!("{}\n\n{}", dashboard_status(&state.relay), state.feedback);
    let dashboard = Paragraph::new(body)
        .wrap(Wrap { trim: true })
        .block(Block::default().title(" Relay ").borders(Borders::ALL));
    frame.render_widget(dashboard, area);
}

fn render_projects(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = project_listing(&state.relay, &state.feedback);
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

fn handle_key(state: &mut AppState, key: KeyEvent) -> Result<bool, Box<dyn Error>> {
    match state.screen {
        Screen::Dashboard => match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => Ok(true),
            KeyCode::Char('p') | KeyCode::Char('P') => {
                state.screen = Screen::Projects;
                Ok(false)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                state.try_start_add_project();
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
    }
}

fn dashboard_status(relay: &Relay) -> String {
    match relay.configured_project_count() {
        0 => "No projects configured.".to_owned(),
        count => format!("{count} projects configured."),
    }
}

fn project_listing(relay: &Relay, feedback: &str) -> String {
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
        lines.push(format!(
            "{} ({})\n  Path: {}\n  Expected remote: {}\n  Default stack: {}",
            project.key(),
            name,
            project.local_path().display(),
            expected_remote,
            default_stack
        ));
    }

    format!("{}\n\n{feedback}", lines.join("\n\n"))
}

fn help_text(state: &AppState) -> &'static str {
    match state.screen {
        Screen::Dashboard if state.registry_writable => {
            "P: projects  A: add project  Q / Esc: quit"
        }
        Screen::Dashboard => "P: projects  Q / Esc: quit",
        Screen::Projects if state.registry_writable => {
            "A: add project  D / Esc: dashboard  Q: quit"
        }
        Screen::Projects => "D / Esc: dashboard  Q: quit",
        Screen::AddProject => "Tab: next field  Enter: register  Esc: projects",
    }
}

fn add_field_line(state: &AppState, field: AddProjectField, label: &str, value: &str) -> String {
    let marker = if state.add_field == field { ">" } else { " " };
    format!("{marker} {label}: {value}")
}

#[derive(Debug, Clone)]
struct AppState {
    relay: Relay,
    config_path: PathBuf,
    registry_writable: bool,
    screen: Screen,
    add_project: AddProjectForm,
    add_field: AddProjectField,
    feedback: String,
}

impl AppState {
    fn new(relay: Relay, config_path: PathBuf, registry_writable: bool, feedback: String) -> Self {
        Self {
            relay,
            config_path,
            registry_writable,
            screen: Screen::Dashboard,
            add_project: AddProjectForm::default(),
            add_field: AddProjectField::Key,
            feedback,
        }
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
                self.feedback = format!(
                    "Registered project '{key}' in {}",
                    self.config_path.display()
                );
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
    Projects,
    AddProject,
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
        assert_eq!(dashboard_status(&Relay::new()), "No projects configured.");
    }

    #[test]
    fn empty_project_screen_lists_empty_state() {
        assert!(project_listing(&Relay::new(), "ready").contains("No projects configured."));
        assert!(project_listing(&Relay::new(), "ready").contains("ready"));
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
        let listing = project_listing(&relay, "ready");

        assert!(listing.contains("relay (Relay)"));
        assert!(listing.contains("https://github.com/Jacinth091/Relay.git"));
        assert!(listing.contains("chatgpt-codex"));
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

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("relay-tui-{name}-{nanos}"))
    }
}
