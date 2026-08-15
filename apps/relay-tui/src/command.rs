use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayCommand {
    Home,
    New,
    Tasks,
    Projects,
    ProjectAdd { path: Option<PathBuf> },
    Runtimes,
    RuntimeDetail { provider: String },
    RuntimeUse { profile: String },
    RuntimeTest { provider: String },
    RuntimeProfile { profile: String },
    Settings,
    Doctor,
    Help,
    Run { task: String },
    Cancel { task: String },
    Diff { task: Option<String> },
    Logs { task: Option<String> },
    Quit,
}

pub const COMMANDS: &[&str] = &[
    "/home",
    "/new",
    "/tasks",
    "/projects",
    "/project add [path]",
    "/runtimes",
    "/runtime <provider>",
    "/runtime use <profile>",
    "/runtime test <provider>",
    "/runtime profile <profile>",
    "/settings",
    "/doctor",
    "/help",
    "/run <task>",
    "/cancel <task>",
    "/diff [task]",
    "/logs [task]",
    "/quit",
];

pub fn parse_relay_command(input: &str) -> Result<RelayCommand, String> {
    let input = input.trim();
    let (name, argument) = input
        .split_once(' ')
        .map_or((input, ""), |(name, arg)| (name, arg.trim()));
    match name {
        "/home" if argument.is_empty() => Ok(RelayCommand::Home),
        "/new" if argument.is_empty() => Ok(RelayCommand::New),
        "/tasks" if argument.is_empty() => Ok(RelayCommand::Tasks),
        "/projects" if argument.is_empty() => Ok(RelayCommand::Projects),
        "/project" if argument == "add" => Ok(RelayCommand::ProjectAdd { path: None }),
        "/project" if argument.starts_with("add ") && argument[4..].trim().chars().count() > 0 => {
            Ok(RelayCommand::ProjectAdd {
                path: Some(PathBuf::from(argument[4..].trim())),
            })
        }
        "/runtimes" if argument.is_empty() => Ok(RelayCommand::Runtimes),
        "/runtime" if argument.is_empty() => Ok(RelayCommand::Runtimes),
        "/runtime" if argument == "use" => Err("Missing argument for /runtime use".to_owned()),
        "/runtime" if argument.starts_with("use ") && argument[4..].trim().chars().count() > 0 => {
            Ok(RelayCommand::RuntimeUse {
                profile: argument[4..].trim().to_owned(),
            })
        }
        "/runtime" if argument == "test" => Err("Missing argument for /runtime test".to_owned()),
        "/runtime" if argument.starts_with("test ") && argument[5..].trim().chars().count() > 0 => {
            Ok(RelayCommand::RuntimeTest {
                provider: argument[5..].trim().to_owned(),
            })
        }
        "/runtime" if argument == "profile" => {
            Err("Missing argument for /runtime profile".to_owned())
        }
        "/runtime"
            if argument.starts_with("profile ") && argument[8..].trim().chars().count() > 0 =>
        {
            Ok(RelayCommand::RuntimeProfile {
                profile: argument[8..].trim().to_owned(),
            })
        }
        "/runtime" => Ok(RelayCommand::RuntimeDetail {
            provider: argument.to_owned(),
        }),
        "/settings" if argument.is_empty() => Ok(RelayCommand::Settings),
        "/doctor" if argument.is_empty() => Ok(RelayCommand::Doctor),
        "/help" if argument.is_empty() => Ok(RelayCommand::Help),
        "/run" if !argument.is_empty() => Ok(RelayCommand::Run {
            task: argument.to_owned(),
        }),
        "/cancel" if !argument.is_empty() => Ok(RelayCommand::Cancel {
            task: argument.to_owned(),
        }),
        "/diff" => Ok(RelayCommand::Diff {
            task: if argument.is_empty() {
                None
            } else {
                Some(argument.to_owned())
            },
        }),
        "/logs" => Ok(RelayCommand::Logs {
            task: if argument.is_empty() {
                None
            } else {
                Some(argument.to_owned())
            },
        }),
        "/quit" if argument.is_empty() => Ok(RelayCommand::Quit),
        _ => Err("Unknown Relay command. Type /help.".to_owned()),
    }
}

#[allow(dead_code)]
pub fn command_suggestions(input: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .filter(|command| command.starts_with(input.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_registered_commands_only() {
        assert_eq!(
            parse_relay_command("/run abc"),
            Ok(RelayCommand::Run {
                task: "abc".to_owned()
            })
        );
        assert_eq!(
            parse_relay_command("/project add C:/repo"),
            Ok(RelayCommand::ProjectAdd {
                path: Some(PathBuf::from("C:/repo"))
            })
        );
        for input in [
            "git status",
            "rm -rf .",
            "powershell -Command whoami",
            "/foo",
        ] {
            assert!(parse_relay_command(input).is_err());
        }
    }
    #[test]
    fn filters_fixed_command_registry() {
        assert_eq!(
            command_suggestions("/pro"),
            vec!["/projects", "/project add [path]"]
        );
    }
    #[test]
    fn parses_new_runtime_and_diff_commands() {
        assert_eq!(parse_relay_command("/runtimes"), Ok(RelayCommand::Runtimes));
        assert_eq!(
            parse_relay_command("/runtime codex"),
            Ok(RelayCommand::RuntimeDetail {
                provider: "codex".to_owned()
            })
        );
        assert_eq!(
            parse_relay_command("/runtime antigravity"),
            Ok(RelayCommand::RuntimeDetail {
                provider: "antigravity".to_owned()
            })
        );
        assert_eq!(
            parse_relay_command("/runtime use codex/implement"),
            Ok(RelayCommand::RuntimeUse {
                profile: "codex/implement".to_owned()
            })
        );
        assert_eq!(
            parse_relay_command("/runtime test codex"),
            Ok(RelayCommand::RuntimeTest {
                provider: "codex".to_owned()
            })
        );
        assert_eq!(
            parse_relay_command("/runtime profile codex/implement"),
            Ok(RelayCommand::RuntimeProfile {
                profile: "codex/implement".to_owned()
            })
        );
        assert_eq!(parse_relay_command("/runtime"), Ok(RelayCommand::Runtimes));

        assert_eq!(
            parse_relay_command("/diff"),
            Ok(RelayCommand::Diff { task: None })
        );
        assert_eq!(
            parse_relay_command("/diff abc123"),
            Ok(RelayCommand::Diff {
                task: Some("abc123".to_owned())
            })
        );
        assert_eq!(
            parse_relay_command("/logs"),
            Ok(RelayCommand::Logs { task: None })
        );
        assert_eq!(
            parse_relay_command("/logs abc123"),
            Ok(RelayCommand::Logs {
                task: Some("abc123".to_owned())
            })
        );

        assert!(parse_relay_command("/runtime use").is_err());
        assert!(parse_relay_command("/runtime test").is_err());
        assert!(parse_relay_command("/runtime profile").is_err());
    }
    #[test]
    fn suggestions_for_runtime() {
        let suggestions = command_suggestions("/runtime");
        assert!(suggestions.contains(&"/runtime <provider>"));
        assert!(suggestions.contains(&"/runtime use <profile>"));
        assert!(suggestions.contains(&"/runtime test <provider>"));
        assert!(suggestions.contains(&"/runtime profile <profile>"));
        assert!(suggestions.contains(&"/runtimes"));
    }
}
