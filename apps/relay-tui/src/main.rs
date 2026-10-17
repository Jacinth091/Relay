mod bootstrap;
mod cli;
mod command;
mod command_completion;
mod commands;
mod diff;
mod events;
mod input;
mod markdown;
mod mcp;
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
    io::{self, Stdout},
};

use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use relay_core::{
    DEFAULT_LEASE_ID, DEFAULT_LEASE_TTL, Relay, connect_or_embed, default_config_path,
    default_database_path,
};

type Tui = Terminal<CrosstermBackend<Stdout>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let command = match cli::parse_cli(std::env::args_os()) {
        Ok(command) => command,
        Err(error) => error.exit(),
    };
    match command {
        cli::CliCommand::ServeMcp => return serve_mcp().await,
        cli::CliCommand::Daemon {
            lease_id,
            lease_ttl,
        } => return run_daemon(lease_id, lease_ttl).await,
        cli::CliCommand::RunTui => {}
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

    let (daemon, embedded_daemon, daemon_feedback) = match &store {
        Some(store) => match connect_or_embed(
            store.clone(),
            relay.clone(),
            DEFAULT_LEASE_ID,
            DEFAULT_LEASE_TTL,
        )
        .await
        {
            Ok((handle, embedded)) => (Some(handle), embedded, String::new()),
            Err(error) => (
                None,
                None,
                format!(
                    "Daemon error: {error}\nTask import is disabled until the daemon is reachable."
                ),
            ),
        },
        None => (None, None, String::new()),
    };

    let mut combined_feedback = format!("{feedback}\n{database_feedback}");
    if !daemon_feedback.is_empty() {
        combined_feedback.push('\n');
        combined_feedback.push_str(&daemon_feedback);
    }

    let mut state = AppState::new_with_store(
        relay,
        config_path,
        registry_writable,
        database_path,
        store,
        durable_tasks,
        combined_feedback,
    );
    state.attach_daemon(daemon);
    state.refresh_durable_tasks().await;
    state.begin_health_refresh(false);
    let mut terminal = initialize_terminal()?;

    let run_result = run(&mut terminal, &mut state).await;
    let restore_result = restore_terminal(&mut terminal);

    if let Some(embedded) = embedded_daemon {
        embedded.shutdown().await;
    }

    run_result?;
    restore_result?;
    Ok(())
}

async fn serve_mcp() -> Result<(), Box<dyn Error>> {
    // stdout carries MCP protocol messages only; diagnostics go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let config_path = default_config_path();
    let relay = match Relay::load_from_config_path(&config_path) {
        Ok(relay) => relay,
        Err(error) => {
            eprintln!(
                "relay: config error at {}: {error}\nMCP submissions fail closed until projects are registered.",
                config_path.display()
            );
            Relay::new()
        }
    };

    let database_path = default_database_path();
    let store = relay_core::SqliteStore::open(&database_path).await.map_err(|error| {
        eprintln!(
            "relay: database error at {}: {error}\nserve-mcp cannot confirm durability and must exit.",
            database_path.display()
        );
        error
    })?;

    let (daemon, embedded_daemon) = connect_or_embed(
        store.clone(),
        relay.clone(),
        DEFAULT_LEASE_ID,
        DEFAULT_LEASE_TTL,
    )
    .await
    .map_err(|error| {
        eprintln!("relay: failed to connect to or embed the Relay daemon: {error}");
        error
    })?;

    mcp::run(relay, store, daemon).await?;

    if let Some(embedded) = embedded_daemon {
        embedded.shutdown().await;
    }
    Ok(())
}

async fn run_daemon(lease_id: String, lease_ttl_secs: u64) -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let config_path = default_config_path();
    let relay = match Relay::load_from_config_path(&config_path) {
        Ok(relay) => relay,
        Err(error) => {
            eprintln!(
                "relay: config error at {}: {error}\nDaemon started with empty project registry.",
                config_path.display()
            );
            Relay::new()
        }
    };

    let database_path = default_database_path();
    let store = relay_core::SqliteStore::open(&database_path).await.map_err(|error| {
        eprintln!(
            "relay: database error at {}: {error}\ndaemon cannot confirm durability and must exit.",
            database_path.display()
        );
        error
    })?;

    let ttl = std::time::Duration::from_secs(lease_ttl_secs);
    let (daemon, mut embedded) = relay_core::run_full_daemon(store, relay, &lease_id, ttl)
        .await
        .map_err(|error| {
            eprintln!("relay: failed to start daemon: {error}");
            error
        })?;

    tracing::info!(
        lease_id = %lease_id,
        fencing_token = daemon.fencing_token(),
        "Relay daemon running and serving requests"
    );

    // Exit on either an explicit shutdown signal or loss of authoritative
    // ownership of the daemon lease: a daemon that no longer owns the lease must
    // not keep running (and must release its transport) while a successor serves
    // the same database.
    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal?;
            tracing::info!("Relay daemon received shutdown signal");
        }
        () = embedded.wait_for_shutdown() => {
            tracing::warn!(
                "Relay daemon lost authoritative ownership of its lease; shutting down"
            );
        }
    }
    embedded.shutdown().await;

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
