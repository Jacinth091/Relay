mod bootstrap;
mod cli;
mod command;
mod command_completion;
mod commands;
mod diff;
mod events;
mod input;
mod markdown;
mod project_discovery;
mod runtime_settings;
mod state;
mod theme;
mod ui;
mod util;

#[cfg(test)]
mod tests;

pub(crate) use input::handle_key;
pub(crate) use state::AppState;
pub(crate) use ui::render;

use std::{
    error::Error,
    io::{self, BufRead, Stdout},
};

use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use relay_core::{Relay, default_config_path, default_database_path};

type Tui = Terminal<CrosstermBackend<Stdout>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let command = match cli::parse_cli(std::env::args_os()) {
        Ok(command) => command,
        Err(error) => error.exit(),
    };
    if let cli::CliCommand::ServeMcp = command {
        return serve_mcp().await;
    }

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
    let (store, durable_tasks, database_feedback) = bootstrap::open_store(&database_path).await;
    let mut state = AppState::new_with_store(
        relay,
        config_path,
        registry_writable,
        database_path,
        store,
        durable_tasks,
        format!("{feedback}\n{database_feedback}"),
    );
    state.refresh_durable_tasks().await;
    state.begin_health_refresh(false);
    let mut terminal = initialize_terminal()?;

    let run_result = run(&mut terminal, &mut state).await;
    let restore_result = restore_terminal(&mut terminal);

    run_result?;
    restore_result?;
    Ok(())
}

async fn serve_mcp() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
    }
    Ok(())
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
