use std::{
    error::Error,
    io::{self, Stdout},
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
};
use relay_core::Relay;

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> Result<(), Box<dyn Error>> {
    let relay = Relay::new();
    let mut terminal = initialize_terminal()?;

    let run_result = run(&mut terminal, &relay);
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

fn run(terminal: &mut Tui, relay: &Relay) -> Result<(), Box<dyn Error>> {
    loop {
        terminal.draw(|frame| render_dashboard(frame, relay))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press
                    && matches!(
                        key.code,
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc
                    )
                {
                    return Ok(());
                }
            }
        }
    }
}

fn render_dashboard(frame: &mut Frame, relay: &Relay) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(frame.area());

    let dashboard = Paragraph::new(dashboard_status(relay))
        .block(Block::default().title(" Relay ").borders(Borders::ALL));
    frame.render_widget(dashboard, areas[0]);

    let help = Paragraph::new("Q / Esc: quit");
    frame.render_widget(help, areas[1]);
}

fn dashboard_status(relay: &Relay) -> String {
    match relay.configured_project_count() {
        0 => "No projects configured.".to_owned(),
        count => format!("{count} projects configured."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_relay_maps_to_expected_dashboard_message() {
        assert_eq!(dashboard_status(&Relay::new()), "No projects configured.");
    }
}
