use std::collections::BTreeMap;
use std::time::Duration;

use relay_core::{
    AmbiguityPolicy, BranchState, ExecutionLimits, HandoffTask, ProjectKey, Relay,
    RepositoryEligibility, RepositoryInspection, RepositorySafetyFailure, SchedulerTaskStatus,
    StoredTask, TaskId, TaskImportCandidate, TaskState, TrustedProject,
    inspect_trusted_project_repository,
};

use crate::state::AppState;

const CODEX_DEFAULT_MAX_DURATION_SECS: u64 = 30 * 60;
const CODEX_MAX_DURATION_SECS: u64 = 4 * 60 * 60;

pub(crate) fn parse_codex_duration(value: Option<&str>) -> Result<Duration, String> {
    let seconds = match value {
        None => CODEX_DEFAULT_MAX_DURATION_SECS,
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "RELAY_CODEX_MAX_DURATION_SECS must be a positive integer".to_owned())?,
    };
    if seconds == 0 || seconds > CODEX_MAX_DURATION_SECS {
        return Err(format!(
            "RELAY_CODEX_MAX_DURATION_SECS must be between 1 and {CODEX_MAX_DURATION_SECS}"
        ));
    }
    Ok(Duration::from_secs(seconds))
}

#[cfg(not(test))]
pub(crate) fn production_codex_execution_limits() -> ExecutionLimits {
    ExecutionLimits {
        max_duration: parse_codex_duration(
            std::env::var("RELAY_CODEX_MAX_DURATION_SECS")
                .ok()
                .as_deref(),
        )
        .unwrap_or_else(|_| Duration::from_secs(CODEX_DEFAULT_MAX_DURATION_SECS)),
        ..ExecutionLimits::default()
    }
}

pub(crate) fn app_execution_limits() -> ExecutionLimits {
    #[cfg(test)]
    {
        ExecutionLimits::default()
    }
    #[cfg(not(test))]
    {
        production_codex_execution_limits()
    }
}

pub(crate) fn visible_scroll(body: &str, viewport_height: u16) -> u16 {
    body.lines()
        .count()
        .saturating_sub(viewport_height as usize)
        .min(u16::MAX as usize) as u16
}

#[cfg(test)]
pub(crate) fn dashboard_status(state: &AppState) -> String {
    let projects = match state.relay.configured_project_count() {
        0 => "No projects configured.".to_owned(),
        count => format!("{count} projects configured."),
    };
    let tasks = match state.durable_tasks.len() {
        0 => "No durable tasks.".to_owned(),
        1 => "1 durable task.".to_owned(),
        count => format!("{count} durable tasks."),
    };
    let mut execution = vec!["Execution".to_owned()];
    for status in [
        SchedulerTaskStatus::Running,
        SchedulerTaskStatus::QueuedReady,
        SchedulerTaskStatus::QueuedGlobal,
        SchedulerTaskStatus::QueuedProject,
    ] {
        let count = state
            .scheduler_snapshot
            .tasks
            .iter()
            .filter(|task| task.status == status)
            .count();
        execution.push(format!("{}: {count}", scheduler_status_text(&status)));
    }
    for durable in [
        TaskState::NeedsClarification,
        TaskState::NeedsDecision,
        TaskState::Done,
        TaskState::Failed,
        TaskState::Cancelled,
    ] {
        execution.push(format!(
            "{}: {}",
            durable.as_str(),
            state
                .durable_tasks
                .iter()
                .filter(|task| task.state == durable)
                .count()
        ));
    }
    format!("{projects}\n{tasks}\n\n{}", execution.join("\n"))
}

pub(crate) fn activity_home(state: &AppState) -> String {
    if state.relay.configured_project_count() == 0 {
        return format!(
            "Relay                                      {}\n\nWelcome to Relay.\n\nNo projects are configured yet.\nRelay needs a trusted local Git repository before it can run implementation tasks.\n\nTry:\n  /project add\n  /help\n\n{}",
            state.runtime_health.display_status(),
            state.feedback
        );
    }

    let mut lines = vec![format!(
        "Relay                                      {}",
        state.runtime_health.display_status()
    )];

    // Escalation inbox: work waiting on a human decision.
    let escalated: Vec<&StoredTask> = state
        .durable_tasks
        .iter()
        .filter(|task| {
            matches!(
                task.state,
                TaskState::NeedsClarification | TaskState::NeedsDecision
            )
        })
        .collect();
    lines.push("\nESCALATION INBOX".to_owned());
    if escalated.is_empty() {
        lines.push("  No escalations waiting.".to_owned());
    } else {
        for task in &escalated {
            let kind = if task.state == TaskState::NeedsClarification {
                "CLARIFICATION"
            } else {
                "DECISION"
            };
            lines.push(format!(
                "  ! {kind}  {}  {}",
                task.project_key, task.handoff.title
            ));
            lines.push(format!("      {} — packet visible via /tasks", task.id));
        }
    }

    // Distribution: where approved work went.
    lines.push("\nDISTRIBUTION".to_owned());
    let distribution = distribution_rows(state);
    if distribution.is_empty() {
        lines.push("  No approved work yet.".to_owned());
    } else {
        for row in distribution {
            lines.push(format!("  {row}"));
        }
    }

    // Queue summary (de-emphasized; below escalations and distribution).
    let active = state
        .scheduler_snapshot
        .tasks
        .iter()
        .filter(|task| matches!(task.status, SchedulerTaskStatus::Running))
        .count();
    let waiting = state
        .scheduler_snapshot
        .tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                SchedulerTaskStatus::QueuedGlobal | SchedulerTaskStatus::QueuedProject
            )
        })
        .count();
    let attention = escalated.len();
    if active + waiting + attention == 0 {
        lines.push("\nNo active tasks.".to_owned());
    } else {
        lines.push(format!(
            "\n{active} active · {waiting} waiting · {attention} needs attention"
        ));
    }
    for task in state
        .durable_tasks
        .iter()
        .filter(|task| !matches!(task.state, TaskState::Done | TaskState::Cancelled))
        .take(6)
    {
        lines.push(format!(
            "\n{} {}\n  {}",
            if matches!(
                task.state,
                TaskState::NeedsClarification | TaskState::NeedsDecision
            ) {
                "!"
            } else {
                "●"
            },
            task.project_key,
            task.handoff.title
        ));
        lines.push(format!("  {}", task.state.as_str().to_uppercase()));
    }
    lines.push(format!("\n{}", state.feedback));
    lines.join("\n")
}

pub(crate) fn distribution_rows(state: &AppState) -> Vec<String> {
    let profile_for_task = |task_id: TaskId| -> String {
        state
            .execution_records
            .get(&task_id)
            .map(|record| record.runtime_profile.clone())
            .unwrap_or_else(|| state.active_profile_id.clone())
    };

    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for snapshot in state
        .scheduler_snapshot
        .tasks
        .iter()
        .filter(|snapshot| matches!(snapshot.status, SchedulerTaskStatus::Running))
    {
        counts
            .entry(profile_for_task(snapshot.task_id))
            .or_default()
            .0 += 1;
    }
    for task in state
        .durable_tasks
        .iter()
        .filter(|task| matches!(task.state, TaskState::Done))
    {
        counts.entry(profile_for_task(task.id)).or_default().1 += 1;
    }

    counts
        .into_iter()
        .filter(|(_, (running, completed))| running + completed > 0)
        .map(|(profile, (running, completed))| {
            format!(
                "{} ({profile}): {running} running · {completed} completed",
                provider_label_for_profile(&profile)
            )
        })
        .collect()
}

pub(crate) fn provider_label_for_profile(profile: &str) -> String {
    match profile.split('/').next().unwrap_or(profile) {
        "codex" => "Codex".to_owned(),
        "antigravity" => "Antigravity".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

pub(crate) fn scheduler_status_text(status: &SchedulerTaskStatus) -> &'static str {
    match status {
        SchedulerTaskStatus::Running => "RUNNING",
        SchedulerTaskStatus::WaitingPermission => "WAITING FOR PERMISSION",
        SchedulerTaskStatus::QueuedReady => "QUEUED",
        SchedulerTaskStatus::QueuedGlobal => "WAITING (global)",
        SchedulerTaskStatus::QueuedProject => "WAITING (project)",
    }
}

pub(crate) fn task_selection_listing(state: &AppState) -> String {
    if state.durable_tasks.is_empty() {
        return format!("No durable tasks.\n\n{}", state.feedback);
    }
    let lines = state
        .durable_tasks
        .iter()
        .map(|task| {
            let marker = if state.selected_task == Some(task.id) {
                ">"
            } else {
                " "
            };
            let status = state
                .scheduler_snapshot
                .tasks
                .iter()
                .find(|snapshot| snapshot.task_id == task.id)
                .map(|snapshot| match snapshot.status {
                    SchedulerTaskStatus::Running => "RUNNING".to_owned(),
                    SchedulerTaskStatus::WaitingPermission => "WAIT:PERMISSION".to_owned(),
                    SchedulerTaskStatus::QueuedReady => "QUEUED".to_owned(),
                    SchedulerTaskStatus::QueuedGlobal => "WAIT:GLOBAL".to_owned(),
                    SchedulerTaskStatus::QueuedProject => "WAIT:PROJ".to_owned(),
                })
                .unwrap_or_else(|| task.state.as_str().to_ascii_uppercase());
            format!(
                "{marker} {status:<12} {:<12} {}",
                task.project_key.as_str(),
                task.handoff.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{lines}\n\n{}", state.feedback)
}

pub(crate) fn project_listing(
    relay: &Relay,
    repository_inspections: &BTreeMap<ProjectKey, RepositoryInspection>,
    feedback: &str,
) -> String {
    if relay.configured_project_count() == 0 {
        return format!("No projects configured.\n\n{feedback}");
    }

    let mut lines = Vec::new();
    for project in relay.list_projects() {
        let expected_remote = project
            .expected_remote()
            .map(|remote| remote.as_str())
            .unwrap_or("not configured");
        let default_stack = project.default_stack().unwrap_or("not configured");
        let name = project.name().unwrap_or(project.key().as_str());
        let repository = repository_inspections
            .get(project.key())
            .map(|inspection| {
                repository_inspection_line(&inspection.eligibility, &inspection.status)
            })
            .unwrap_or_else(|| "not inspected".to_owned());
        lines.push(format!(
            "{} ({})\n  Path: {}\n  Expected remote: {}\n  Default stack: {}\n  Repository: {}",
            project.key(),
            name,
            project.local_path().display(),
            expected_remote,
            default_stack,
            repository
        ));
    }

    format!("{}\n\n{feedback}", lines.join("\n\n"))
}

#[cfg(test)]
pub(crate) fn durable_task_listing(tasks: &[StoredTask], feedback: &str) -> String {
    if tasks.is_empty() {
        return format!("No durable tasks.\n\n{feedback}");
    }

    let lines = tasks
        .iter()
        .map(|task| {
            format!(
                "{}\n  Title: {}\n  Project: {}\n  State: {}\n  Submission transport: {}\n\n  Goal: {}\n\n  Requirements:\n{}\n\n  Constraints:\n{}\n\n  Verification:\n{}",
                task.id,
                task.handoff.title,
                task.project_key.as_str(),
                task.state.as_str(),
                task.submission_transport.as_str(),
                task.handoff.task.goal,
                indented_bullet_lines(&task.handoff.task.requirements),
                indented_bullet_lines(&task.handoff.task.constraints),
                indented_bullet_lines(&task.handoff.task.verification)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    format!("{lines}\n\n{feedback}")
}

/// Blocking git inspection for a single trusted project; intended to run on
/// a blocking thread via `spawn_blocking`.
pub(crate) fn repository_preflight_status(project: &TrustedProject) -> String {
    let inspection = inspect_trusted_project_repository(project);
    if inspection.eligibility == RepositoryEligibility::Eligible {
        "eligible".to_owned()
    } else {
        format!(
            "unavailable: {}",
            repository_inspection_line(&inspection.eligibility, &inspection.status)
        )
    }
}

pub(crate) fn repository_inspection_line(
    eligibility: &RepositoryEligibility,
    status: &Option<relay_core::RepositoryStatus>,
) -> String {
    match eligibility {
        RepositoryEligibility::Eligible => {
            let Some(status) = status else {
                return "eligible; status unavailable".to_owned();
            };
            format!(
                "eligible; {}; {}",
                branch_state_text(&status.branch_state),
                if status.clean { "clean" } else { "dirty" }
            )
        }
        RepositoryEligibility::NotEligible(reason) => {
            format!("not eligible; {}", safety_failure_text(reason))
        }
    }
}

pub(crate) fn branch_state_text(branch_state: &BranchState) -> String {
    match branch_state {
        BranchState::Branch(branch) => format!("branch {branch}"),
        BranchState::Detached { commit } => format!("detached at {commit}"),
    }
}

pub(crate) fn safety_failure_text(reason: &RepositorySafetyFailure) -> String {
    match reason {
        RepositorySafetyFailure::GitUnavailable => "git unavailable".to_owned(),
        RepositorySafetyFailure::NotGitWorktree => "not a Git worktree".to_owned(),
        RepositorySafetyFailure::MissingExpectedRemote => "missing expected remote".to_owned(),
        RepositorySafetyFailure::MissingRemote { name } => format!("missing remote {name}"),
        RepositorySafetyFailure::GitCommandFailed { operation, .. } => {
            format!("git inspection failed during {operation:?}")
        }
        RepositorySafetyFailure::MalformedExpectedRemote { .. } => {
            "malformed expected remote".to_owned()
        }
        RepositorySafetyFailure::MalformedActualRemote { .. } => {
            "malformed actual remote".to_owned()
        }
        RepositorySafetyFailure::RemoteMismatch { expected, actual } => {
            format!("remote mismatch: expected {expected}, actual {actual}")
        }
        RepositorySafetyFailure::UnexpectedGitOutput { operation, .. } => {
            format!("unexpected git output during {operation:?}")
        }
    }
}

pub(crate) fn task_preview_text(candidate: &TaskImportCandidate) -> String {
    format!(
        "Task ID: created after Import\n{}\n\nSubmission transport: {:?}\nValidation: VALID\n\nActions: Import / Cancel",
        handoff_summary_text(
            candidate.handoff(),
            candidate.project_key().as_str(),
            candidate.project_name()
        ),
        candidate.submission_transport()
    )
}

pub(crate) fn handoff_summary_text(
    handoff: &HandoffTask,
    resolved_project_key: &str,
    project_name: Option<&str>,
) -> String {
    format!(
        "Project: {}{}\nTitle: {}\nRequested stack: {}\nGoal: {}\n\nRequirements:\n{}\n\nConstraints:\n{}\n\nVerification:\n{}\n\nAmbiguity policy:\n{}",
        resolved_project_key,
        project_name
            .map(|name| format!(" ({name})"))
            .unwrap_or_default(),
        handoff.title,
        handoff.stack.as_deref().unwrap_or("not requested"),
        handoff.task.goal,
        bullet_lines(&handoff.task.requirements),
        bullet_lines(&handoff.task.constraints),
        bullet_lines(&handoff.task.verification),
        ambiguity_policy_text(handoff.ambiguity_policy),
    )
}

pub(crate) fn bullet_lines(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("* {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
pub(crate) fn indented_bullet_lines(items: &[String]) -> String {
    bullet_lines(items)
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn ambiguity_policy_text(policy: AmbiguityPolicy) -> &'static str {
    match policy {
        AmbiguityPolicy::NeedsClarification => "needs_clarification",
    }
}
