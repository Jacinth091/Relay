use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use relay_runtime_codex::{CodexRuntime, CodexRuntimeHealth};

use crate::state::{
    AppState, RuntimeDetailSubmenu, SETTINGS_FIELDS, SettingsField, git_health_marker,
};
use crate::theme::Theme;
use crate::util::app_execution_limits;

pub(crate) fn render_runtimes(frame: &mut Frame, state: &AppState, area: Rect) {
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

pub(crate) fn render_runtime_detail(frame: &mut Frame, state: &AppState, area: Rect) {
    let provider = state.runtime_detail_provider.as_deref().unwrap_or("codex");

    if let Some(submenu) = state.runtime_detail_submenu {
        let body = match submenu {
            RuntimeDetailSubmenu::ModelSelector => {
                let mut lines = vec!["Select Model:\n".to_owned()];
                if let Some(catalog) = &state.cached_model_catalog {
                    for (i, m) in catalog.available_models.iter().enumerate() {
                        let cursor = if i == state.runtime_detail_menu_index {
                            ">"
                        } else {
                            " "
                        };
                        lines.push(format!("{cursor} {}", m.id));
                    }
                } else {
                    lines.push("  (No models available or loading...)".to_owned());
                }
                lines.join("\n")
            }
            RuntimeDetailSubmenu::ReasoningSelector => {
                let mut lines = vec!["Select Reasoning Effort:\n".to_owned()];
                // Ideally this would be filtered by `supported_reasoning_efforts` of the selected model.
                // For simplicity, showing standard efforts.
                let efforts = ["low", "medium", "high", "xhigh"];
                for (i, e) in efforts.iter().enumerate() {
                    let cursor = if i == state.runtime_detail_menu_index {
                        ">"
                    } else {
                        " "
                    };
                    lines.push(format!("{cursor} {e}"));
                }
                lines.join("\n")
            }
        };
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
        return;
    }

    let menu_items = [
        "Change model",
        "Change reasoning",
        "Change profile",
        "Test runtime",
    ];
    let mut menu_lines = String::new();
    for (i, item) in menu_items.iter().enumerate() {
        let cursor = if i == state.runtime_detail_menu_index {
            ">"
        } else {
            " "
        };
        menu_lines.push_str(&format!("{cursor} {item}\n"));
    }

    let body = match provider {
        "codex" => {
            let health_detail =
                codex_health_detail(&state.runtime_health, &CodexRuntime::default());
            let account_info = state
                .cached_codex_account
                .as_deref()
                .unwrap_or("Managed Codex login (no secrets displayed)");

            let interface =
                if std::env::var("RELAY_CODEX_TRANSPORT").ok().as_deref() == Some("app-server") {
                    "App Server"
                } else {
                    "Codex exec"
                };
            let approval = if interface == "App Server" {
                "Live approval ✓"
            } else {
                "Pre-approved envelope"
            };
            format!(
                "Runtime / Codex\n\nStatus\n{}\n\nAuthentication\n  {}\n\nInterface\n  {}\n\nActive profile\n  {}\n\nAI\n  Model: {}\n  Reasoning: {}\n\n{}\nPermissions\n  {}\n\nCapabilities\n  Workspace read: ✓\n  Workspace write: Configured\n  Commands: Configured\n  Network: ×\n  Commit: ×\n  Push: ×\n\nLimits\n  Duration: {} min\n  Repairs: {}",
                health_detail,
                account_info,
                interface,
                state.active_profile_id,
                state.active_model.as_option().unwrap_or("provider default"),
                state
                    .active_reasoning_effort
                    .as_option()
                    .map(|effort| format!("{effort:?}"))
                    .unwrap_or_else(|| "profile default".to_owned()),
                menu_lines,
                approval,
                app_execution_limits().max_duration.as_secs() / 60,
                app_execution_limits().max_repair_attempts
            )
        }
        "antigravity" => {
            format!(
                "Runtime / Antigravity\n\nStatus\n  {}\n\nActive profile\n  antigravity/implement\n\nAI\n  Model: Default\n  Effort: High\n\n{}\nCapabilities\n  Workspace read: ✓\n  Workspace write: Configured\n  Commands: Configured\n  Network: ×\n  Commit: ×\n  Push: ×",
                state.antigravity_health.display_status(),
                menu_lines
            )
        }
        _ => format!("Unknown runtime provider: {provider}"),
    };
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

pub(crate) fn render_settings(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let form = &state.settings_form;

    let mut lines: Vec<Line> = vec![Line::from(Span::styled("Execution", theme.header()))];

    for field in SETTINGS_FIELDS {
        settings_field_line(&mut lines, state, field);
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Safety", theme.header())));
    for entry in [
        "Explicit Run approval: On",
        "Automatic commit: Off",
        "Automatic push: Off",
    ] {
        lines.push(Line::from(Span::styled(
            format!("  {entry}"),
            theme.muted(),
        )));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Storage", theme.header())));
    lines.push(Line::from(Span::styled(
        format!("  Config: {}", state.config_path.display()),
        theme.muted(),
    )));
    lines.push(Line::from(Span::styled(
        format!("  SQLite: {}", state.database_path.display()),
        theme.muted(),
    )));

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Runtime", theme.header())));
    lines.push(Line::from(Span::styled(
        format!(
            "  Provider: {} · Profile: {} · Status: {}",
            state.active_runtime_provider,
            state.active_profile_id,
            state.runtime_health.display_status()
        ),
        theme.muted(),
    )));

    // Reserved status line at the panel bottom: inline error (theme error
    // color) or muted "saved" feedback; blank when neither applies.
    lines.push(match (&form.error, form.saved) {
        (Some(error), _) => Line::from(Span::styled(error.clone(), theme.error())),
        (None, true) => Line::from(Span::styled("Saved.", theme.muted())),
        (None, false) => Line::default(),
    });

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.muted())
                .title(" SETTINGS "),
        ),
        area,
    );
}

fn settings_field_line(lines: &mut Vec<Line<'static>>, state: &AppState, field: SettingsField) {
    let theme = Theme::default();
    let form = &state.settings_form;
    let selected = form.selected == field;
    let marker = if selected { "> " } else { "  " };
    let value = if selected && form.editing {
        format!("{}_", form.buffer)
    } else {
        state.settings_field_value(field)
    };
    let label_style = if selected {
        theme.selection()
    } else {
        theme.muted()
    };
    let value_style = if selected {
        theme.selection()
    } else {
        theme.text()
    };
    lines.push(Line::from(vec![
        Span::raw(marker),
        Span::styled(format!("{}: ", field.label()), label_style),
        Span::styled(value, value_style),
    ]));
}

pub(crate) fn render_doctor(frame: &mut Frame, state: &AppState, area: Rect) {
    let status_header = if state.health_refresh.is_some() {
        "Relay Doctor — [Checking system diagnostics...]"
    } else {
        "Relay Doctor — [Diagnostics ready]"
    };

    let body = format!(
        "{status_header}\n\nCurrent execution requirements\n\n{}\n{} Relay config\n{} SQLite\n{}\n  {}\n{}\n  {}\n\nFuture integrations\n{}\nGitHub Issue ingress: not implemented until M10\nRelay Direct / MCP: not implemented until M12",
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
        CodexRuntimeHealth::Available { version, .. } => {
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
