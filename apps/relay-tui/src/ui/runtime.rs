use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use relay_runtime_antigravity::AntigravityRuntimeHealth;
use relay_runtime_codex::{CodexRuntime, CodexRuntimeHealth};

use crate::state::{
    AppState, DOCTOR_CHECKS, DoctorCheckStatus, RUNTIME_PROVIDERS, RuntimeDetailSubmenu,
    SETTINGS_ROWS, SettingsRow, reasoning_effort_label,
};
use crate::theme::{Theme, marker_active, marker_failure, marker_success, marker_waiting};
use crate::util::app_execution_limits;

fn panel_block(title: &str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme.muted())
        .title(Span::styled(format!(" {title} "), theme.header()))
}

fn label_value_line(label: &str, value: String, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), theme.muted()),
        Span::styled(value, theme.text()),
    ])
}

/// One provider row on the Runtimes screen: cursor marker, health-colored
/// status dot, bright display name, muted profile id, and a default marker.
fn runtime_row(state: &AppState, provider: &str, selected: bool, theme: &Theme) -> Line<'static> {
    let (name, default_profile, ready, checking, is_default) = match provider {
        "codex" => (
            "Codex CLI",
            "codex/implement",
            state.runtime_health.is_ready(),
            matches!(state.runtime_health, CodexRuntimeHealth::Checking),
            state.active_runtime_provider == "codex",
        ),
        _ => (
            "Antigravity",
            "antigravity/implement",
            state.antigravity_health.is_ready(),
            matches!(state.antigravity_health, AntigravityRuntimeHealth::Checking),
            state.active_runtime_provider == "antigravity",
        ),
    };

    let row_style = if selected {
        theme.selection()
    } else {
        theme.text()
    };
    let dot_style = if ready {
        theme.success()
    } else if checking {
        theme.muted()
    } else {
        theme.error()
    };
    let dot = if ready || checking {
        marker_active()
    } else {
        marker_waiting()
    };
    let profile_id = if is_default {
        state.active_profile_id.clone()
    } else {
        default_profile.to_owned()
    };

    let mut spans = vec![
        Span::styled(if selected { "› " } else { "  " }, row_style),
        Span::styled(format!("{dot} "), dot_style),
        Span::styled(name.to_owned(), row_style),
        Span::styled("   ", theme.muted()),
        Span::styled(profile_id, theme.muted()),
    ];
    if is_default {
        spans.push(Span::styled(" · default", theme.muted()));
    }
    Line::from(spans)
}

pub(crate) fn render_runtimes(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let block = panel_block("RUNTIMES", &theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let selected = state.runtimes_list_index.min(RUNTIME_PROVIDERS.len() - 1);
    let mut lines: Vec<Line> = RUNTIME_PROVIDERS
        .iter()
        .enumerate()
        .map(|(i, provider)| runtime_row(state, provider, i == selected, &theme))
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!("Default profile: {}", state.active_profile_id),
        theme.muted(),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Runtime Detail as stacked bordered panels; tight vertical space lets the
/// layout solver compress panels rather than lose their titles.
pub(crate) fn render_runtime_detail(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();

    if state.runtime_detail_submenu.is_some() {
        render_runtime_submenu(frame, state, area, &theme);
        return;
    }

    let provider = state.runtime_detail_provider.as_deref().unwrap_or("codex");
    let status_lines = status_panel_lines(state, provider, &theme);
    let ai_lines = ai_panel_lines(state, provider, &theme);
    let action_lines = action_menu_lines(state.runtime_detail_menu_index, &theme);
    let capability_lines = capability_panel_lines(&theme);
    let limits_lines = limits_panel_lines(&theme);

    // Length constraints per panel (content + border); any leftover height is
    // absorbed by the trailing filler so panel titles stay pinned to content.
    let mut constraints: Vec<Constraint> =
        [&status_lines, &ai_lines, &action_lines, &capability_lines]
            .iter()
            .map(|lines| Constraint::Length(lines.len() as u16 + 2))
            .collect();
    constraints.push(Constraint::Fill(1));

    let panels = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for ((title, lines), panel_area) in [
        ("STATUS", &status_lines),
        ("AI", &ai_lines),
        ("ACTIONS", &action_lines),
        ("CAPABILITIES", &capability_lines),
        ("LIMITS", &limits_lines),
    ]
    .into_iter()
    .zip(panels.iter())
    {
        render_panel(frame, title, lines, *panel_area, &theme);
    }
}

fn render_panel(frame: &mut Frame, title: &str, lines: &[Line], area: Rect, theme: &Theme) {
    let block = panel_block(title, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false }),
        inner,
    );
}

fn status_panel_lines(state: &AppState, provider: &str, theme: &Theme) -> Vec<Line<'static>> {
    match provider {
        "codex" => codex_status_lines(state, theme),
        "antigravity" => vec![label_value_line(
            "Status",
            state.antigravity_health.display_status().to_owned(),
            theme,
        )],
        other => vec![Line::from(Span::styled(
            format!("Unknown runtime provider: {other}"),
            theme.error(),
        ))],
    }
}

fn codex_status_lines(state: &AppState, theme: &Theme) -> Vec<Line<'static>> {
    let executable = CodexRuntime::default()
        .resolve_executable()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "no supported executable resolved".to_owned());
    let mut lines = vec![label_value_line("Executable", executable, theme)];

    match &state.runtime_health {
        CodexRuntimeHealth::Checking => {
            lines.push(Line::from(Span::styled(
                "Health: probe pending; Run remains blocked until verification completes",
                theme.muted(),
            )));
        }
        CodexRuntimeHealth::Available { version, .. } => {
            lines.push(label_value_line("Version", version.clone(), theme));
            lines.push(Line::from(Span::styled(
                "✓ Codex authentication and exec compatibility",
                theme.success(),
            )));
        }
        CodexRuntimeHealth::NotAuthenticated { version, error } => {
            lines.push(label_value_line(
                "Version",
                version
                    .clone()
                    .unwrap_or_else(|| "version unavailable".to_owned()),
                theme,
            ));
            lines.push(Line::from(Span::styled(
                format!("Authentication unavailable: {error}"),
                theme.warning(),
            )));
        }
        CodexRuntimeHealth::Incompatible { reason, .. } => {
            lines.push(Line::from(Span::styled(
                format!("Compatibility unavailable: {reason}"),
                theme.warning(),
            )));
        }
        CodexRuntimeHealth::NotInstalled { error }
        | CodexRuntimeHealth::DiscoveryFailed { error } => {
            lines.push(Line::from(Span::styled(error.clone(), theme.error())));
        }
    }

    let account = state
        .cached_codex_account
        .clone()
        .unwrap_or_else(|| "Managed Codex login (no secrets displayed)".to_owned());
    lines.push(label_value_line("Authentication", account, theme));

    // Mirrors relay-runtime-codex transport selection: app-server is the
    // DEFAULT; only an explicit `exec` opts out.
    let app_server = std::env::var("RELAY_CODEX_TRANSPORT").as_deref() != Ok("exec");
    let (interface, approval) = if app_server {
        ("App Server", "Live approval ✓")
    } else {
        ("Codex exec", "Pre-approved envelope")
    };
    lines.push(Line::from(vec![
        Span::styled("Interface: ", theme.muted()),
        Span::styled(interface.to_owned(), theme.text()),
        Span::styled(format!(" — {approval}"), theme.muted()),
    ]));
    lines
}

fn ai_panel_lines(state: &AppState, provider: &str, theme: &Theme) -> Vec<Line<'static>> {
    match provider {
        "codex" => vec![
            label_value_line("Profile", state.active_profile_id.clone(), theme),
            label_value_line(
                "Model",
                state
                    .active_model
                    .as_option()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "provider default".to_owned()),
                theme,
            ),
            label_value_line(
                "Reasoning",
                state
                    .active_reasoning_effort
                    .as_option()
                    .map(|effort| format!("{effort:?}"))
                    .unwrap_or_else(|| "profile default".to_owned()),
                theme,
            ),
        ],
        _ => vec![
            label_value_line("Profile", "antigravity/implement".to_owned(), theme),
            label_value_line("Model", "Default".to_owned(), theme),
            label_value_line("Effort", "High".to_owned(), theme),
        ],
    }
}

const RUNTIME_ACTIONS: [&str; 4] = [
    "Change model",
    "Change reasoning",
    "Change profile",
    "Test runtime",
];

fn action_menu_lines(selected: usize, theme: &Theme) -> Vec<Line<'static>> {
    RUNTIME_ACTIONS
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == selected {
                theme.selection()
            } else {
                theme.text()
            };
            Line::from(vec![
                Span::styled(if i == selected { "› " } else { "  " }, style),
                Span::styled((*item).to_owned(), style),
            ])
        })
        .collect()
}

enum CapabilityState {
    Available,
    Configured,
    Unavailable,
}

fn capability_panel_lines(theme: &Theme) -> Vec<Line<'static>> {
    [
        ("Workspace read", CapabilityState::Available),
        ("Workspace write", CapabilityState::Configured),
        ("Commands", CapabilityState::Configured),
        ("Network", CapabilityState::Unavailable),
        ("Commit", CapabilityState::Unavailable),
        ("Push", CapabilityState::Unavailable),
    ]
    .iter()
    .map(|(label, state)| {
        let (marker, style, suffix) = match state {
            CapabilityState::Available => (marker_success(), theme.success(), ""),
            CapabilityState::Configured => ("·", theme.muted(), " configured"),
            CapabilityState::Unavailable => (marker_failure(), theme.error(), ""),
        };
        Line::from(vec![
            Span::styled(format!("  {label:<16}"), theme.text()),
            Span::styled(marker, style),
            Span::styled(suffix.to_owned(), theme.muted()),
        ])
    })
    .collect()
}

fn limits_panel_lines(theme: &Theme) -> Vec<Line<'static>> {
    let limits = app_execution_limits();
    vec![
        label_value_line(
            "Duration",
            format!("{} min", limits.max_duration.as_secs() / 60),
            theme,
        ),
        label_value_line("Repairs", format!("{}", limits.max_repair_attempts), theme),
    ]
}

fn render_runtime_submenu(frame: &mut Frame, state: &AppState, area: Rect, theme: &Theme) {
    let submenu = match state.runtime_detail_submenu {
        Some(submenu) => submenu,
        None => return,
    };
    let selected = state.runtime_detail_menu_index;

    let (title, entries): (&str, Vec<String>) = match submenu {
        RuntimeDetailSubmenu::ModelSelector => (
            "MODEL",
            state
                .cached_model_catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .available_models
                        .iter()
                        .map(|model| model.id.clone())
                        .collect()
                })
                .unwrap_or_default(),
        ),
        RuntimeDetailSubmenu::ReasoningSelector => (
            "REASONING",
            state
                .reasoning_options_for_model(None)
                .iter()
                .map(|effort| reasoning_effort_label(*effort).to_owned())
                .collect(),
        ),
        RuntimeDetailSubmenu::ProfileSelector => (
            "PROFILE",
            AppState::provider_profiles(
                state.runtime_detail_provider.as_deref().unwrap_or("codex"),
            )
            .iter()
            .map(|profile| format!("{} — {}", profile.id, profile.purpose))
            .collect(),
        ),
    };

    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::from(Span::styled(
            match submenu {
                RuntimeDetailSubmenu::ModelSelector => "(No models available or loading...)",
                RuntimeDetailSubmenu::ReasoningSelector => "(No supported reasoning efforts)",
                RuntimeDetailSubmenu::ProfileSelector => "(No profiles available)",
            },
            theme.muted(),
        ))]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let style = if i == selected {
                    theme.selection()
                } else {
                    theme.text()
                };
                Line::from(vec![
                    Span::styled(if i == selected { "› " } else { "  " }, style),
                    Span::styled(entry.clone(), style),
                ])
            })
            .collect()
    };

    let block = panel_block(title, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub(crate) fn render_settings(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();
    let form = &state.settings_form;

    let mut lines: Vec<Line> = Vec::new();
    let mut current_section: Option<&'static str> = None;
    for row in SETTINGS_ROWS {
        if Some(row.section()) != current_section {
            if current_section.is_some() {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(row.section(), theme.header())));
            current_section = Some(row.section());
        }
        settings_row_line(&mut lines, state, row);
    }

    // Reserved status line at the panel bottom: inline error (theme error
    // color) or muted "saved" feedback; blank when neither applies.
    lines.push(match (&form.error, form.saved) {
        (Some(error), _) => Line::from(Span::styled(error.clone(), theme.error())),
        (None, true) => Line::from(Span::styled("Saved.", theme.muted())),
        (None, false) => Line::default(),
    });

    let block = panel_block("SETTINGS", &theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// One Settings row across any section: "›" cursor on the selected row,
/// muted label / bright value; a selected read-only row appends its policy
/// hint inline instead of an edit affordance.
fn settings_row_line(lines: &mut Vec<Line<'static>>, state: &AppState, row: SettingsRow) {
    let theme = Theme::default();
    let form = &state.settings_form;
    let selected = form.selected == row;
    let marker_style = if selected {
        theme.selection()
    } else {
        theme.muted()
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

    match row.editable_field() {
        Some(field) => {
            let value = if selected && form.editing {
                format!("{}_", form.buffer)
            } else {
                state.settings_field_value(field)
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "› " } else { "  " }, marker_style),
                Span::styled(format!("{}: ", field.label()), label_style),
                Span::styled(value, value_style),
            ]));
        }
        None => {
            let locked = row.locked();
            let mut spans = vec![
                Span::styled(
                    if selected {
                        if locked { "# " } else { "› " }
                    } else {
                        "  "
                    },
                    marker_style,
                ),
                Span::styled(format!("{}: ", row.label()), label_style),
                Span::styled(row.readonly_value(state).unwrap_or_default(), value_style),
            ];
            if locked {
                spans.push(Span::styled("  [locked]", theme.warning()));
            }
            if selected && let Some(hint) = row.hint() {
                spans.push(Span::styled(format!("  — {hint}"), theme.muted()));
            }
            lines.push(Line::from(spans));
        }
    }
}

pub(crate) fn render_doctor(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();

    let check_lines: Vec<Line> = DOCTOR_CHECKS
        .iter()
        .enumerate()
        .map(|(i, check)| doctor_check_line(state, i, check, &theme))
        .collect();
    let banner = doctor_banner_line(state, &theme);
    let transport_lines: Vec<Line> = vec![Line::from(vec![
        Span::styled("Relay Direct / MCP: ", theme.muted()),
        Span::styled("implemented (`relay serve-mcp`)", theme.text()),
    ])];

    let panels = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(check_lines.len() as u16 + 2),
            Constraint::Length(1),
            Constraint::Length(transport_lines.len() as u16 + 2),
            Constraint::Fill(1),
        ])
        .split(area);

    let block = panel_block("DIAGNOSTICS", &theme);
    let inner = block.inner(panels[0]);
    frame.render_widget(block, panels[0]);
    frame.render_widget(
        Paragraph::new(check_lines).wrap(Wrap { trim: false }),
        inner,
    );

    frame.render_widget(banner, panels[1]);

    let block = panel_block("SUBMISSION TRANSPORTS", &theme);
    let inner = block.inner(panels[2]);
    frame.render_widget(block, panels[2]);
    frame.render_widget(
        Paragraph::new(transport_lines).wrap(Wrap { trim: false }),
        inner,
    );
}

/// One Doctor check row: status glyph colored by state-only rules
/// (✓ success / × error / … checking), bright name, muted detail.
fn doctor_check_line(state: &AppState, index: usize, check: &str, theme: &Theme) -> Line<'static> {
    let name = match check {
        "git" => "Git",
        "config" => "Config",
        "sqlite" => "SQLite",
        "codex" => "Codex CLI",
        _ => "Antigravity",
    };
    let status = state.doctor_check_status(check);
    let (glyph, glyph_style, detail) = match &status {
        DoctorCheckStatus::Pending => ("…", theme.muted(), "checking".to_owned()),
        DoctorCheckStatus::Ok(detail) => (marker_success(), theme.success(), detail.clone()),
        DoctorCheckStatus::Failed(detail) => (marker_failure(), theme.error(), detail.clone()),
    };
    let selected = index == state.doctor_selected_index.min(DOCTOR_CHECKS.len() - 1);
    let name_style = if selected {
        theme.selection()
    } else {
        theme.text()
    };
    Line::from(vec![
        Span::styled(
            if selected { "› " } else { "  " },
            if selected {
                theme.selection()
            } else {
                theme.muted()
            },
        ),
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled(format!("{name:<12}"), name_style),
        Span::styled(detail, theme.muted()),
    ])
}

/// Overall diagnostics banner between the panels: warning while probes are
/// pending, otherwise a ready summary colored by outcome.
fn doctor_banner_line(state: &AppState, theme: &Theme) -> Paragraph<'static> {
    let pending = state.health_refresh.is_some()
        || DOCTOR_CHECKS
            .iter()
            .any(|check| matches!(state.doctor_check_status(check), DoctorCheckStatus::Pending));
    let failed = !pending
        && DOCTOR_CHECKS.iter().any(|check| {
            matches!(
                state.doctor_check_status(check),
                DoctorCheckStatus::Failed(_)
            )
        });
    let (text, style) = if pending {
        ("Checking system diagnostics…", theme.warning())
    } else if failed {
        ("Diagnostics ready — attention required", theme.warning())
    } else {
        ("Diagnostics ready — all checks passed", theme.success())
    };
    Paragraph::new(Line::from(Span::styled(text.to_owned(), style)))
}

/// Version reported by a probed runtime health, if the CLI answered
/// `--version` at any point during discovery.
fn codex_version(health: &CodexRuntimeHealth) -> Option<String> {
    match health {
        CodexRuntimeHealth::Available { version, .. }
        | CodexRuntimeHealth::Incompatible { version, .. } => Some(version.clone()),
        CodexRuntimeHealth::NotAuthenticated { version, .. } => version.clone(),
        _ => None,
    }
}

fn antigravity_version(health: &AntigravityRuntimeHealth) -> Option<String> {
    match health {
        AntigravityRuntimeHealth::Available { version }
        | AntigravityRuntimeHealth::Incompatible { version, .. }
        | AntigravityRuntimeHealth::AuthUnverifiable { version } => Some(version.clone()),
        AntigravityRuntimeHealth::NotAuthenticated { version, .. } => version.clone(),
        _ => None,
    }
}

/// Updates screen: local component versions plus honest update guidance.
/// Relay never phones home; every value comes from local probes already in
/// app state, so "refresh versions" just re-runs those probes.
pub(crate) fn render_updates(frame: &mut Frame, state: &AppState, area: Rect) {
    let theme = Theme::default();

    let relay_lines = vec![
        label_value_line("Relay", format!("v{}", env!("CARGO_PKG_VERSION")), &theme),
        label_value_line("Config", state.config_path.display().to_string(), &theme),
        label_value_line(
            "Local state",
            state.database_path.display().to_string(),
            &theme,
        ),
    ];

    let component_line = |name: &str, version: Option<String>, status: String| {
        let value = match version {
            Some(version) => format!("{version} — {status}"),
            None => status,
        };
        Line::from(vec![
            Span::styled(format!("{name:<14}"), theme.text()),
            Span::styled(value, theme.muted()),
        ])
    };
    let component_lines = vec![
        component_line(
            "Codex CLI",
            codex_version(&state.runtime_health),
            state.runtime_health.display_status().to_owned(),
        ),
        component_line(
            "Antigravity",
            antigravity_version(&state.antigravity_health),
            state.antigravity_health.display_status().to_owned(),
        ),
        component_line(
            "Git",
            match &state.git_health {
                crate::state::ToolHealth::Checking => None,
                crate::state::ToolHealth::Available(version) => Some(version.clone()),
                crate::state::ToolHealth::Unavailable(_) => None,
            },
            match &state.git_health {
                crate::state::ToolHealth::Checking => "checking".to_owned(),
                crate::state::ToolHealth::Available(_) => "git available".to_owned(),
                crate::state::ToolHealth::Unavailable(error) => error.clone(),
            },
        ),
    ];

    let guidance_lines: Vec<Line> = [
        "Relay does not check for updates online; press r to re-probe local components.",
        "Relay: pull this repository, then cargo install --path apps/relay-tui.",
        "Codex CLI: update through your install channel (npm/brew), verify with /doctor.",
        "Antigravity CLI: update via its installer, verify with /doctor.",
    ]
    .into_iter()
    .map(|text| Line::from(Span::styled(text.to_owned(), theme.muted())))
    .collect();

    let panels = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(relay_lines.len() as u16 + 2),
            Constraint::Length(component_lines.len() as u16 + 2),
            Constraint::Fill(1),
        ])
        .split(area);

    let block = panel_block("INSTALLED", &theme);
    let inner = block.inner(panels[0]);
    frame.render_widget(block, panels[0]);
    frame.render_widget(
        Paragraph::new(relay_lines).wrap(Wrap { trim: false }),
        inner,
    );

    let block = panel_block("COMPONENTS", &theme);
    let inner = block.inner(panels[1]);
    frame.render_widget(block, panels[1]);
    frame.render_widget(
        Paragraph::new(component_lines).wrap(Wrap { trim: false }),
        inner,
    );

    let block = panel_block("HOW TO UPDATE", &theme);
    let inner = block.inner(panels[2]);
    frame.render_widget(block, panels[2]);
    frame.render_widget(
        Paragraph::new(guidance_lines).wrap(Wrap { trim: false }),
        inner,
    );
}
