//! Responsive terminal event loop and event pump for Relay TUI.
//!
//! Replaces fixed poll/sleep loops with an event-driven `tokio::select!` loop
//! over Crossterm terminal events, background task signals, and dirty frame ticks.

use std::error::Error;
use std::time::Duration;

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::Terminal;
use tokio::time::interval;

pub async fn run_event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut crate::AppState,
) -> Result<(), Box<dyn Error>> {
    let mut reader = EventStream::new();
    let mut frame_interval = interval(Duration::from_millis(33)); // ~30 FPS frame clock
    let mut dirty = true;

    loop {
        state.poll_health_refresh().await;
        state.tick().await;

        if dirty {
            terminal.draw(|frame| crate::render(frame, state))?;
            dirty = false;
        }

        tokio::select! {
            _ = frame_interval.tick() => {
                if state.has_active_execution() {
                    dirty = true;
                }
            }
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        if crate::handle_key(state, key).await? {
                            break;
                        }
                        dirty = true;
                    }
                    Some(Ok(Event::Paste(pasted))) => {
                        // Bulk paste handled in a single state update without character-by-character redraw
                        state.handle_paste(&pasted);
                        dirty = true;
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        dirty = true;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        return Err(Box::new(error));
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}
