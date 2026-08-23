//! Responsive terminal event loop, key event filter, and event pump for Relay TUI.
//!
//! Normalizes physical keyboard event semantics (filtering KeyEventKind::Release and Repeat)
//! and coalesces render frame updates using tokio::select!.

use std::error::Error;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use futures::StreamExt;
use ratatui::Terminal;
use tokio::time::interval;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputContext {
    CommandPrompt,
    TextEditor,
    Selection,
    Confirmation,
    PassiveView,
}

/// Key event disposition filter.
///
/// Ensures physical key releases never mutate state or trigger commands,
/// and context-sensitive key repeats operate safely.
pub fn should_dispatch_key(key: &KeyEvent, context: InputContext) -> bool {
    if std::env::var_os("RELAY_TUI_INPUT_TRACE").is_some() {
        eprintln!(
            "[INPUT TRACE] kind={:?} code={:?} modifiers={:?} context={:?}",
            key.kind, key.code, key.modifiers, context
        );
    }

    // 1. Release events are ALWAYS ignored
    if key.kind == KeyEventKind::Release {
        return false;
    }

    // 2. Press events are ALWAYS dispatched
    if key.kind == KeyEventKind::Press {
        return true;
    }

    // 3. Repeat events are context-sensitive
    if key.kind == KeyEventKind::Repeat {
        return match key.code {
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::PageUp
            | KeyCode::PageDown => true,
            KeyCode::Backspace | KeyCode::Delete => true,
            // Printable characters in text entry ignore repeat to prevent OS key repeat floods
            KeyCode::Char(_)
                if context == InputContext::TextEditor
                    || context == InputContext::CommandPrompt =>
            {
                false
            }
            _ => false,
        };
    }

    true
}

pub async fn run_event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut crate::AppState,
) -> Result<(), Box<dyn Error>> {
    let mut reader = EventStream::new();
    let mut frame_interval = interval(Duration::from_millis(33)); // ~30 FPS frame clock (~33ms)
    // Draw initial frame
    terminal.draw(|frame| crate::render(frame, state))?;

    loop {
        state.poll_health_refresh().await;
        state.tick().await;

        tokio::select! {
            _ = frame_interval.tick() => {
                // A 30 FPS frame clock keeps health probes, scheduler state,
                // and asynchronous permission prompts visible even when no
                // key event arrives. Rendering is bounded and avoids stale
                // TUI state after background work completes.
                terminal.draw(|frame| crate::render(frame, state))?;
            }
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        let context = state.input_context();
                        if should_dispatch_key(&key, context)
                            && crate::handle_key(state, key).await?
                        {
                            break;
                        }
                    }
                    Some(Ok(Event::Paste(pasted))) => {
                        state.handle_paste(&pasted);
                    }
                    Some(Ok(Event::Resize(_, _))) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn make_key(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn release_events_are_always_ignored() {
        let press = make_key(KeyCode::Char('a'), KeyEventKind::Press);
        let release = make_key(KeyCode::Char('a'), KeyEventKind::Release);

        assert!(should_dispatch_key(&press, InputContext::CommandPrompt));
        assert!(!should_dispatch_key(&release, InputContext::CommandPrompt));
        assert!(!should_dispatch_key(&release, InputContext::Selection));
        assert!(!should_dispatch_key(&release, InputContext::PassiveView));
    }

    #[test]
    fn repeat_printable_chars_ignored_in_text_entry() {
        let repeat = make_key(KeyCode::Char('a'), KeyEventKind::Repeat);

        assert!(!should_dispatch_key(&repeat, InputContext::CommandPrompt));
        assert!(!should_dispatch_key(&repeat, InputContext::TextEditor));
    }

    #[test]
    fn repeat_navigation_allowed_in_selection() {
        let repeat_down = make_key(KeyCode::Down, KeyEventKind::Repeat);
        let repeat_up = make_key(KeyCode::Up, KeyEventKind::Repeat);

        assert!(should_dispatch_key(&repeat_down, InputContext::Selection));
        assert!(should_dispatch_key(&repeat_up, InputContext::Selection));
    }

    #[test]
    fn repeat_backspace_allowed() {
        let repeat_bs = make_key(KeyCode::Backspace, KeyEventKind::Repeat);

        assert!(should_dispatch_key(&repeat_bs, InputContext::CommandPrompt));
        assert!(should_dispatch_key(&repeat_bs, InputContext::TextEditor));
    }

    #[test]
    fn repeat_enter_and_esc_ignored() {
        let repeat_enter = make_key(KeyCode::Enter, KeyEventKind::Repeat);
        let repeat_esc = make_key(KeyCode::Esc, KeyEventKind::Repeat);

        assert!(!should_dispatch_key(
            &repeat_enter,
            InputContext::Confirmation
        ));
        assert!(!should_dispatch_key(&repeat_esc, InputContext::PassiveView));
    }
}
