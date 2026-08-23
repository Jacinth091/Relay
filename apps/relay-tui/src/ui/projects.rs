use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::state::{AddProjectField, AppState};
use crate::util::project_listing;

pub(crate) fn render_projects(frame: &mut Frame, state: &AppState, area: Rect) {
    let body = project_listing(&state.relay, &state.repository_inspections, &state.feedback);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
}

pub(crate) fn render_add_project(frame: &mut Frame, state: &AppState, area: Rect) {
    let form = &state.add_project;
    if let Some(discovered) = &state.discovered_project {
        let body = format!(
            "Add Project\n\nDetected repository\n\nGit root   {}\nOrigin     {}\n\nWill register\nKey        {}\nPath       {}\nRemote     {}\nGitHub     {}/{}\nStack      {}\n\n✓ Git worktree detected\n{}",
            discovered.repository_root.display(),
            discovered.remote.as_deref().unwrap_or("not configured"),
            form.key,
            form.path,
            form.expected_remote,
            form.github_owner,
            form.github_repo,
            form.default_stack,
            state.feedback
        );
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), area);
        return;
    }
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

fn add_field_line(state: &AppState, field: AddProjectField, label: &str, value: &str) -> String {
    let marker = if state.add_field == field { ">" } else { " " };
    format!("{marker} {label}: {value}")
}
