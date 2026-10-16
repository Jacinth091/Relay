use relay_core::TaskState;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    Home,
    New,
    Tasks,
    Projects,
    ProjectAdd,
    Runtimes,
    RuntimeDetail,
    RuntimeUse,
    RuntimeModel,
    RuntimeReasoning,
    RuntimeTest,
    Settings,
    Doctor,
    Updates,
    Help,
    Run,
    Cancel,
    Diff,
    Logs,
    Quit,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Navigation,
    Tasks,
    Projects,
    Runtime,
    System,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandArgumentKind {
    None,
    Provider,
    RuntimeProfile,
    Model,
    Reasoning,
    ReadyTask,
    CancellableTask,
    InspectableTask,
    ProjectPath,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub id: CommandId,
    pub syntax: &'static str,
    pub completion: &'static str,
    pub description: &'static str,
    pub category: CommandCategory,
    pub argument_kind: CommandArgumentKind,
}

pub const COMMAND_CATALOG: &[CommandSpec] = &[
    CommandSpec {
        id: CommandId::Home,
        syntax: "/home",
        completion: "/home",
        description: "Go to home dashboard",
        category: CommandCategory::Navigation,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::New,
        syntax: "/new",
        completion: "/new",
        description: "Create or import a Relay handoff task",
        category: CommandCategory::Tasks,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::Tasks,
        syntax: "/tasks",
        completion: "/tasks",
        description: "Browse durable tasks",
        category: CommandCategory::Tasks,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::Projects,
        syntax: "/projects",
        completion: "/projects",
        description: "Browse trusted projects",
        category: CommandCategory::Projects,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::ProjectAdd,
        syntax: "/project add [path]",
        completion: "/project add ",
        description: "Register a local repository project",
        category: CommandCategory::Projects,
        argument_kind: CommandArgumentKind::ProjectPath,
    },
    CommandSpec {
        id: CommandId::Runtimes,
        syntax: "/runtimes",
        completion: "/runtimes",
        description: "Browse available coding runtimes",
        category: CommandCategory::Runtime,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::RuntimeDetail,
        syntax: "/runtime <provider>",
        completion: "/runtime ",
        description: "View runtime provider details & health",
        category: CommandCategory::Runtime,
        argument_kind: CommandArgumentKind::Provider,
    },
    CommandSpec {
        id: CommandId::RuntimeUse,
        syntax: "/runtime use <profile>",
        completion: "/runtime use ",
        description: "Select global default runtime profile",
        category: CommandCategory::Runtime,
        argument_kind: CommandArgumentKind::RuntimeProfile,
    },
    CommandSpec {
        id: CommandId::RuntimeModel,
        syntax: "/runtime model <model>",
        completion: "/runtime model ",
        description: "Select global default runtime model",
        category: CommandCategory::Runtime,
        argument_kind: CommandArgumentKind::Model,
    },
    CommandSpec {
        id: CommandId::RuntimeReasoning,
        syntax: "/runtime reasoning <effort>",
        completion: "/runtime reasoning ",
        description: "Select global default runtime reasoning effort",
        category: CommandCategory::Runtime,
        argument_kind: CommandArgumentKind::Reasoning,
    },
    CommandSpec {
        id: CommandId::RuntimeTest,
        syntax: "/runtime test <provider>",
        completion: "/runtime test ",
        description: "Recheck provider prerequisites & probe health",
        category: CommandCategory::Runtime,
        argument_kind: CommandArgumentKind::Provider,
    },
    CommandSpec {
        id: CommandId::Settings,
        syntax: "/settings",
        completion: "/settings",
        description: "Open Relay configuration settings",
        category: CommandCategory::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::Doctor,
        syntax: "/doctor",
        completion: "/doctor",
        description: "Run system diagnostics & health check",
        category: CommandCategory::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::Updates,
        syntax: "/updates",
        completion: "/updates",
        description: "Show component versions & update guidance",
        category: CommandCategory::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::Run,
        syntax: "/run <task>",
        completion: "/run ",
        description: "Approve and execute a READY task",
        category: CommandCategory::Tasks,
        argument_kind: CommandArgumentKind::ReadyTask,
    },
    CommandSpec {
        id: CommandId::Cancel,
        syntax: "/cancel <task>",
        completion: "/cancel ",
        description: "Cancel a running or queued task",
        category: CommandCategory::Tasks,
        argument_kind: CommandArgumentKind::CancellableTask,
    },
    CommandSpec {
        id: CommandId::Diff,
        syntax: "/diff [task]",
        completion: "/diff ",
        description: "View read-only repository git diff",
        category: CommandCategory::Tasks,
        argument_kind: CommandArgumentKind::InspectableTask,
    },
    CommandSpec {
        id: CommandId::Logs,
        syntax: "/logs [task]",
        completion: "/logs ",
        description: "View execution logs for a task",
        category: CommandCategory::Tasks,
        argument_kind: CommandArgumentKind::InspectableTask,
    },
    CommandSpec {
        id: CommandId::Help,
        syntax: "/help",
        completion: "/help",
        description: "Show command reference documentation",
        category: CommandCategory::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        id: CommandId::Quit,
        syntax: "/quit",
        completion: "/quit",
        description: "Exit Relay TUI",
        category: CommandCategory::System,
        argument_kind: CommandArgumentKind::None,
    },
];

#[derive(Debug, Clone)]
pub struct ProviderCompletion {
    pub provider_id: String,
    pub display_name: String,
    pub status_note: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProfileCompletion {
    pub profile_id: String,
    pub provider_id: String,
    pub display_name: String,
    pub description: String,
    pub available: bool,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskCompletion {
    pub id_str: String,
    pub short_id: String,
    pub project_key: String,
    pub title: String,
    pub state: TaskState,
    pub eligible_for_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CompletionContext {
    pub providers: Vec<ProviderCompletion>,
    pub profiles: Vec<ProfileCompletion>,
    pub ready_tasks: Vec<TaskCompletion>,
    pub cancellable_tasks: Vec<TaskCompletion>,
    pub inspectable_tasks: Vec<TaskCompletion>,
    pub models: Vec<String>,
    pub reasoning_efforts: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionKind {
    Command,
    Provider,
    RuntimeProfile,
    Model,
    Reasoning,
    Task,
    Path,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommandSuggestion {
    pub display_primary: String,
    pub display_secondary: Option<String>,
    pub completion: String,
    pub description: String,
    pub kind: SuggestionKind,
    pub executable: bool,
    pub disabled_reason: Option<String>,
    pub score: u16,
}

pub fn compute_suggestions(input: &str, context: &CompletionContext) -> Vec<CommandSuggestion> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return Vec::new();
    }

    // Argument completions: check if input matches specific command prefixes
    if let Some(query) = trimmed.strip_prefix("/runtime use ") {
        return compute_profile_suggestions(query, context);
    }
    if let Some(query) = trimmed.strip_prefix("/runtime model ") {
        return compute_model_suggestions(query, context);
    }
    if let Some(query) = trimmed.strip_prefix("/runtime reasoning ") {
        return compute_reasoning_suggestions(query, context);
    }
    if let Some(query) = trimmed.strip_prefix("/runtime test ") {
        return compute_provider_suggestions(query, context, "/runtime test ");
    }
    if let Some(query) = trimmed.strip_prefix("/runtime ") {
        return compute_runtime_subcommand_suggestions(query, context);
    }
    if let Some(query) = trimmed.strip_prefix("/run ") {
        return compute_task_suggestions(query, &context.ready_tasks, "/run ");
    }
    if let Some(query) = trimmed.strip_prefix("/cancel ") {
        return compute_task_suggestions(query, &context.cancellable_tasks, "/cancel ");
    }
    if let Some(query) = trimmed.strip_prefix("/diff ") {
        return compute_task_suggestions(query, &context.inspectable_tasks, "/diff ");
    }
    if let Some(query) = trimmed.strip_prefix("/logs ") {
        return compute_task_suggestions(query, &context.inspectable_tasks, "/logs ");
    }
    if let Some(query) = trimmed.strip_prefix("/project add ") {
        return compute_path_suggestions(query);
    }

    // Otherwise: command completion mode
    let mut suggestions = Vec::new();
    let query = trimmed.to_ascii_lowercase();

    for spec in COMMAND_CATALOG {
        let cmd_token = spec.syntax.split_whitespace().next().unwrap_or(spec.syntax);
        let lower_cmd = cmd_token.to_ascii_lowercase();
        let lower_syntax = spec.syntax.to_ascii_lowercase();

        let score = if lower_cmd == query {
            100
        } else if lower_cmd.starts_with(&query) {
            80 + (20 - lower_cmd.len().min(20)) as u16
        } else if lower_syntax.starts_with(&query) {
            60
        } else if lower_syntax.contains(&query)
            || spec.description.to_ascii_lowercase().contains(&query)
        {
            40
        } else {
            0
        };

        if score > 0 {
            let is_executable = spec.argument_kind == CommandArgumentKind::None;
            suggestions.push(CommandSuggestion {
                display_primary: spec.syntax.to_owned(),
                display_secondary: None,
                completion: spec.completion.to_owned(),
                description: spec.description.to_owned(),
                kind: SuggestionKind::Command,
                executable: is_executable,
                disabled_reason: None,
                score,
            });
        }
    }

    suggestions.sort_by_key(|b| std::cmp::Reverse(b.score));
    suggestions
}

fn compute_profile_suggestions(query: &str, context: &CompletionContext) -> Vec<CommandSuggestion> {
    let lower_q = query.trim().to_ascii_lowercase();
    let default_profiles = vec![
        ProfileCompletion {
            profile_id: "codex/implement".to_owned(),
            provider_id: "codex".to_owned(),
            display_name: "Codex / Implement".to_owned(),
            description: "Code generation and workspace modifications".to_owned(),
            available: true,
            disabled_reason: None,
        },
        ProfileCompletion {
            profile_id: "codex/review".to_owned(),
            provider_id: "codex".to_owned(),
            display_name: "Codex / Review".to_owned(),
            description: "Read-only inspection and code review".to_owned(),
            available: true,
            disabled_reason: None,
        },
        ProfileCompletion {
            profile_id: "codex/deep".to_owned(),
            provider_id: "codex".to_owned(),
            display_name: "Codex / Deep".to_owned(),
            description: "High effort reasoning for complex tasks".to_owned(),
            available: true,
            disabled_reason: None,
        },
        ProfileCompletion {
            profile_id: "antigravity/implement".to_owned(),
            provider_id: "antigravity".to_owned(),
            display_name: "Antigravity / Implement".to_owned(),
            description: "Google Antigravity autonomous edits".to_owned(),
            available: true,
            disabled_reason: None,
        },
        ProfileCompletion {
            profile_id: "antigravity/review".to_owned(),
            provider_id: "antigravity".to_owned(),
            display_name: "Antigravity / Review".to_owned(),
            description: "Read-only planning & inspection mode".to_owned(),
            available: true,
            disabled_reason: None,
        },
        ProfileCompletion {
            profile_id: "antigravity/deep".to_owned(),
            provider_id: "antigravity".to_owned(),
            display_name: "Antigravity / Deep".to_owned(),
            description: "High reasoning effort agent turns".to_owned(),
            available: true,
            disabled_reason: None,
        },
    ];

    let profiles = if context.profiles.is_empty() {
        &default_profiles
    } else {
        &context.profiles
    };

    let mut suggestions = Vec::new();
    for p in profiles {
        let lower_id = p.profile_id.to_ascii_lowercase();
        if lower_q.is_empty()
            || lower_id.contains(&lower_q)
            || p.display_name.to_ascii_lowercase().contains(&lower_q)
        {
            let score = if lower_id.starts_with(&lower_q) {
                90
            } else {
                50
            };
            suggestions.push(CommandSuggestion {
                display_primary: p.profile_id.clone(),
                display_secondary: Some(p.display_name.clone()),
                completion: format!("/runtime use {}", p.profile_id),
                description: p.description.clone(),
                kind: SuggestionKind::RuntimeProfile,
                executable: p.available,
                disabled_reason: p.disabled_reason.clone(),
                score,
            });
        }
    }

    suggestions.sort_by_key(|b| std::cmp::Reverse(b.score));
    suggestions
}

fn compute_provider_suggestions(
    query: &str,
    context: &CompletionContext,
    command_prefix: &str,
) -> Vec<CommandSuggestion> {
    let lower_q = query.trim().to_ascii_lowercase();
    let default_providers = vec![
        ProviderCompletion {
            provider_id: "codex".to_owned(),
            display_name: "Codex".to_owned(),
            status_note: "Codex CLI adapter".to_owned(),
        },
        ProviderCompletion {
            provider_id: "antigravity".to_owned(),
            display_name: "Antigravity".to_owned(),
            status_note: "Google Antigravity CLI adapter".to_owned(),
        },
    ];

    let providers = if context.providers.is_empty() {
        &default_providers
    } else {
        &context.providers
    };

    let mut suggestions = Vec::new();
    for p in providers {
        let lower_id = p.provider_id.to_ascii_lowercase();
        if lower_q.is_empty()
            || lower_id.contains(&lower_q)
            || p.display_name.to_ascii_lowercase().contains(&lower_q)
        {
            let score = if lower_id.starts_with(&lower_q) {
                90
            } else {
                50
            };
            suggestions.push(CommandSuggestion {
                display_primary: p.provider_id.clone(),
                display_secondary: Some(p.display_name.clone()),
                completion: format!("{command_prefix}{}", p.provider_id),
                description: p.status_note.clone(),
                kind: SuggestionKind::Provider,
                executable: true,
                disabled_reason: None,
                score,
            });
        }
    }

    suggestions.sort_by_key(|b| std::cmp::Reverse(b.score));
    suggestions
}

fn compute_runtime_subcommand_suggestions(
    query: &str,
    context: &CompletionContext,
) -> Vec<CommandSuggestion> {
    let lower_q = query.trim().to_ascii_lowercase();
    let subcommands = vec![
        (
            "use",
            "/runtime use ",
            "Switch the default runtime profile",
            CommandArgumentKind::RuntimeProfile,
        ),
        (
            "test",
            "/runtime test ",
            "Recheck provider prerequisites & probe health",
            CommandArgumentKind::Provider,
        ),
        (
            "model",
            "/runtime model ",
            "Select global default runtime model",
            CommandArgumentKind::Model,
        ),
        (
            "reasoning",
            "/runtime reasoning ",
            "Select global default runtime reasoning effort",
            CommandArgumentKind::Reasoning,
        ),
    ];

    let mut suggestions = Vec::new();
    for (name, completion, desc, _) in subcommands {
        if lower_q.is_empty() || name.starts_with(&lower_q) {
            suggestions.push(CommandSuggestion {
                display_primary: format!("/runtime {name}"),
                display_secondary: None,
                completion: completion.to_owned(),
                description: desc.to_owned(),
                kind: SuggestionKind::Command,
                executable: false,
                disabled_reason: None,
                score: 85,
            });
        }
    }

    // Also include providers for direct `/runtime <provider>` navigation
    let provider_suggs = compute_provider_suggestions(query, context, "/runtime ");
    suggestions.extend(provider_suggs);

    suggestions.sort_by_key(|b| std::cmp::Reverse(b.score));
    suggestions
}

fn compute_task_suggestions(
    query: &str,
    tasks: &[TaskCompletion],
    command_prefix: &str,
) -> Vec<CommandSuggestion> {
    let lower_q = query.trim().to_ascii_lowercase();
    let mut suggestions = Vec::new();

    for t in tasks {
        let lower_id = t.id_str.to_ascii_lowercase();
        let lower_short = t.short_id.to_ascii_lowercase();
        let lower_project = t.project_key.to_ascii_lowercase();
        let lower_title = t.title.to_ascii_lowercase();

        let score = if lower_short.starts_with(&lower_q) || lower_id.starts_with(&lower_q) {
            100
        } else if lower_project.starts_with(&lower_q) {
            80
        } else if lower_title.contains(&lower_q) {
            60
        } else if lower_q.is_empty() {
            50
        } else {
            0
        };

        if score > 0 {
            let desc = format!("{} · {}", t.project_key, t.title);
            let is_executable = if command_prefix.starts_with("/run") {
                t.eligible_for_run
            } else if command_prefix.starts_with("/cancel") {
                matches!(t.state, TaskState::Running | TaskState::Ready)
            } else if command_prefix.starts_with("/diff") || command_prefix.starts_with("/logs") {
                true
            } else {
                t.eligible_for_run
            };
            suggestions.push(CommandSuggestion {
                display_primary: t.short_id.clone(),
                display_secondary: Some(t.project_key.clone()),
                completion: format!("{command_prefix}{}", t.short_id),
                description: desc,
                kind: SuggestionKind::Task,
                executable: is_executable,
                disabled_reason: if is_executable {
                    None
                } else {
                    Some(format!("{:?}", t.state))
                },
                score,
            });
        }
    }

    suggestions.sort_by_key(|b| std::cmp::Reverse(b.score));
    suggestions
}

fn compute_path_suggestions(query: &str) -> Vec<CommandSuggestion> {
    let mut suggestions = Vec::new();
    let trimmed = query.trim();
    if trimmed.is_empty() || ".".starts_with(trimmed) {
        suggestions.push(CommandSuggestion {
            display_primary: ". ".to_owned(),
            display_secondary: Some("Current directory".to_owned()),
            completion: "/project add .".to_owned(),
            description: "Add repository in the current working directory".to_owned(),
            kind: SuggestionKind::Path,
            executable: true,
            disabled_reason: None,
            score: 100,
        });
    }
    suggestions
}

fn compute_model_suggestions(query: &str, context: &CompletionContext) -> Vec<CommandSuggestion> {
    let lower_q = query.trim().to_ascii_lowercase();
    let mut suggestions = Vec::new();
    let default_models = vec!["default".to_owned()];

    let models = if context.models.is_empty() {
        &default_models
    } else {
        &context.models
    };

    for m in models {
        let lower_m = m.to_ascii_lowercase();
        if lower_q.is_empty() || lower_m.contains(&lower_q) {
            let score = if lower_m.starts_with(&lower_q) {
                90
            } else {
                50
            };
            suggestions.push(CommandSuggestion {
                display_primary: m.clone(),
                display_secondary: None,
                completion: format!("/runtime model {}", m),
                description: "AI Model".to_owned(),
                kind: SuggestionKind::Model,
                executable: true,
                disabled_reason: None,
                score,
            });
        }
    }
    suggestions.sort_by_key(|b| std::cmp::Reverse(b.score));
    suggestions
}

fn compute_reasoning_suggestions(
    query: &str,
    context: &CompletionContext,
) -> Vec<CommandSuggestion> {
    let lower_q = query.trim().to_ascii_lowercase();
    let mut suggestions = Vec::new();
    let default_efforts = vec![
        "low".to_owned(),
        "medium".to_owned(),
        "high".to_owned(),
        "xhigh".to_owned(),
    ];

    let efforts = if context.reasoning_efforts.is_empty() {
        &default_efforts
    } else {
        &context.reasoning_efforts
    };

    for e in efforts {
        let lower_e = e.to_ascii_lowercase();
        if lower_q.is_empty() || lower_e.contains(&lower_q) {
            let score = if lower_e.starts_with(&lower_q) {
                90
            } else {
                50
            };
            suggestions.push(CommandSuggestion {
                display_primary: e.clone(),
                display_secondary: None,
                completion: format!("/runtime reasoning {}", e),
                description: "Reasoning Effort".to_owned(),
                kind: SuggestionKind::Reasoning,
                executable: true,
                disabled_reason: None,
                score,
            });
        }
    }
    suggestions.sort_by_key(|b| std::cmp::Reverse(b.score));
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_catalog_entries_are_valid() {
        for spec in COMMAND_CATALOG {
            assert!(spec.syntax.starts_with('/'));
            assert!(spec.completion.starts_with('/'));
            assert!(!spec.description.is_empty());
        }
    }

    #[test]
    fn root_slash_returns_all_top_level_commands() {
        let ctx = CompletionContext::default();
        let suggs = compute_suggestions("/", &ctx);
        assert!(!suggs.is_empty());
        assert!(suggs.iter().any(|s| s.completion == "/new"));
        assert!(suggs.iter().any(|s| s.completion == "/tasks"));
    }

    #[test]
    fn prefix_filtering_ranks_exact_and_prefix_matches() {
        let ctx = CompletionContext::default();
        let suggs = compute_suggestions("/ru", &ctx);
        assert!(!suggs.is_empty());
        assert_eq!(suggs[0].completion, "/run ");
    }

    #[test]
    fn contextual_profile_completion_works() {
        let ctx = CompletionContext::default();
        let suggs = compute_suggestions("/runtime use ant", &ctx);
        assert!(!suggs.is_empty());
        assert!(
            suggs
                .iter()
                .any(|s| s.completion == "/runtime use antigravity/implement")
        );
    }

    #[test]
    fn contextual_provider_completion_works() {
        let ctx = CompletionContext::default();
        let suggs = compute_suggestions("/runtime c", &ctx);
        assert!(!suggs.is_empty());
        assert!(suggs.iter().any(|s| s.completion == "/runtime codex"));
    }
}
