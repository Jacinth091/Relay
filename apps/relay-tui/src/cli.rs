use std::ffi::OsString;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "relay", version, about = "Relay deterministic handoff engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

impl Cli {
    pub fn command(&self) -> CliCommand {
        self.command.clone().unwrap_or(CliCommand::RunTui)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum CliCommand {
    RunTui,
    ServeMcp,
    Daemon {
        #[arg(long, default_value = "relay-daemon-primary")]
        lease_id: String,
        #[arg(long, default_value_t = 15)]
        lease_ttl: u64,
    },
}

pub fn parse_cli<I, T>(args: I) -> Result<CliCommand, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(args).map(|cli| cli.command())
}

#[cfg(test)]
mod tests {
    use super::CliCommand;
    use super::parse_cli;

    fn args(items: &[&str]) -> Vec<String> {
        let mut argv = vec!["relay".to_string()];
        argv.extend(items.iter().map(|item| (*item).to_string()));
        argv
    }

    #[test]
    fn no_arguments_defaults_to_run_tui() {
        assert_eq!(parse_cli(args(&[])).unwrap(), CliCommand::RunTui);
    }

    #[test]
    fn serve_mcp_subcommand_routes_to_serve_mcp() {
        assert_eq!(
            parse_cli(args(&["serve-mcp"])).unwrap(),
            CliCommand::ServeMcp
        );
    }

    #[test]
    fn daemon_subcommand_defaults() {
        assert_eq!(
            parse_cli(args(&["daemon"])).unwrap(),
            CliCommand::Daemon {
                lease_id: "relay-daemon-primary".to_owned(),
                lease_ttl: 15,
            }
        );
    }

    #[test]
    fn daemon_subcommand_with_custom_flags() {
        assert_eq!(
            parse_cli(args(&[
                "daemon",
                "--lease-id",
                "custom-lease",
                "--lease-ttl",
                "30"
            ]))
            .unwrap(),
            CliCommand::Daemon {
                lease_id: "custom-lease".to_owned(),
                lease_ttl: 30,
            }
        );
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(parse_cli(args(&["frobnicate"])).is_err());
    }

    #[test]
    fn unknown_flag_is_rejected() {
        assert!(parse_cli(args(&["--bogus"])).is_err());
    }

    #[test]
    fn unknown_flag_after_subcommand_is_rejected() {
        assert!(parse_cli(args(&["serve-mcp", "--bogus"])).is_err());
    }

    #[test]
    fn help_flag_is_an_error_result_but_not_a_command() {
        let result = parse_cli(args(&["--help"]));
        assert!(result.is_err());
    }
}
