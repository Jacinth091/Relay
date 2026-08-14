use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayCommand {
    Home,
    New,
    Tasks,
    Projects,
    ProjectAdd { path: Option<PathBuf> },
    Runtime,
    Settings,
    Doctor,
    Help,
    Run { task: String },
    Cancel { task: String },
    Quit,
}

pub const COMMANDS: &[&str] = &[
    "/home",
    "/new",
    "/tasks",
    "/projects",
    "/project add [path]",
    "/runtime",
    "/settings",
    "/doctor",
    "/help",
    "/run <task>",
    "/cancel <task>",
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
        "/runtime" if argument.is_empty() => Ok(RelayCommand::Runtime),
        "/settings" if argument.is_empty() => Ok(RelayCommand::Settings),
        "/doctor" if argument.is_empty() => Ok(RelayCommand::Doctor),
        "/help" if argument.is_empty() => Ok(RelayCommand::Help),
        "/run" if !argument.is_empty() => Ok(RelayCommand::Run {
            task: argument.to_owned(),
        }),
        "/cancel" if !argument.is_empty() => Ok(RelayCommand::Cancel {
            task: argument.to_owned(),
        }),
        "/quit" if argument.is_empty() => Ok(RelayCommand::Quit),
        _ => Err("Unknown Relay command. Type /help.".to_owned()),
    }
}

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
}
