//! Semantic terminal theme for Relay.
//!
//! Provides consistent color roles, status markers, and styling helpers across all screens.

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};

/// Semantic color palette for Relay.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub muted: Color,
    pub text: Color,
    pub selection: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            muted: Color::DarkGray,
            text: Color::Reset,
            selection: Color::Cyan,
            diff_added: Color::Green,
            diff_removed: Color::Red,
        }
    }
}

pub fn style_accent() -> Style {
    Style::default().fg(Color::Cyan)
}

pub fn style_success() -> Style {
    Style::default().fg(Color::Green)
}

pub fn style_warning() -> Style {
    Style::default().fg(Color::Yellow)
}

pub fn style_error() -> Style {
    Style::default().fg(Color::Red)
}

pub fn style_muted() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn style_header() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn style_selection() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn style_diff_added() -> Style {
    Style::default().fg(Color::Green)
}

pub fn style_diff_removed() -> Style {
    Style::default().fg(Color::Red)
}

/// Semantic status markers with text representation
pub fn marker_active() -> &'static str {
    "●"
}

pub fn marker_waiting() -> &'static str {
    "◌"
}

pub fn marker_attention() -> &'static str {
    "!"
}

pub fn marker_success() -> &'static str {
    "✓"
}

pub fn marker_failure() -> &'static str {
    "×"
}

pub fn marker_unknown() -> &'static str {
    "?"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_default_roles() {
        let theme = Theme::default();
        assert_eq!(theme.accent, Color::Cyan);
        assert_eq!(theme.success, Color::Green);
        assert_eq!(theme.warning, Color::Yellow);
        assert_eq!(theme.error, Color::Red);
    }
}
