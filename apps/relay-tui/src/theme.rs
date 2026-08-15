//! Semantic terminal theme for Relay.
//!
//! Provides consistent color roles, status markers, and styling helpers across all screens.

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

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

impl Theme {
    pub fn accent(&self) -> Style {
        Style::default().fg(self.accent)
    }

    pub fn success(&self) -> Style {
        Style::default().fg(self.success)
    }

    pub fn warning(&self) -> Style {
        Style::default().fg(self.warning)
    }

    pub fn error(&self) -> Style {
        Style::default().fg(self.error)
    }

    pub fn muted(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn text(&self) -> Style {
        Style::default().fg(self.text)
    }

    pub fn header(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn selection(&self) -> Style {
        Style::default()
            .fg(self.selection)
            .add_modifier(Modifier::BOLD)
    }

    pub fn diff_added(&self) -> Style {
        Style::default().fg(self.diff_added)
    }

    pub fn diff_removed(&self) -> Style {
        Style::default().fg(self.diff_removed)
    }
}

pub fn style_accent() -> Style {
    Theme::default().accent()
}

pub fn style_success() -> Style {
    Theme::default().success()
}

pub fn style_warning() -> Style {
    Theme::default().warning()
}

pub fn style_error() -> Style {
    Theme::default().error()
}

pub fn style_muted() -> Style {
    Theme::default().muted()
}

pub fn style_header() -> Style {
    Theme::default().header()
}

pub fn style_selection() -> Style {
    Theme::default().selection()
}

pub fn style_diff_added() -> Style {
    Theme::default().diff_added()
}

pub fn style_diff_removed() -> Style {
    Theme::default().diff_removed()
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

/// Formats git diff patches into styled ratatui Line elements.
pub fn format_diff_lines(diff_text: &str, theme: &Theme) -> Vec<Line<'static>> {
    diff_text
        .lines()
        .map(|line| {
            if line.starts_with("+++") || line.starts_with("---") {
                Line::from(Span::styled(line.to_owned(), theme.muted()))
            } else if line.starts_with('+') {
                Line::from(Span::styled(line.to_owned(), theme.diff_added()))
            } else if line.starts_with('-') {
                Line::from(Span::styled(line.to_owned(), theme.diff_removed()))
            } else if line.starts_with("@@") {
                Line::from(Span::styled(line.to_owned(), theme.accent()))
            } else if line.starts_with("diff --git") || line.starts_with("index ") {
                Line::from(Span::styled(line.to_owned(), theme.header()))
            } else {
                Line::from(Span::raw(line.to_owned()))
            }
        })
        .collect()
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

    #[test]
    fn formats_diff_lines_with_semantic_styles() {
        let theme = Theme::default();
        let diff = "diff --git a/file.rs b/file.rs\n@@ -1,2 +1,2 @@\n-old line\n+new line";
        let lines = format_diff_lines(diff, &theme);
        assert_eq!(lines.len(), 4);
    }
}
