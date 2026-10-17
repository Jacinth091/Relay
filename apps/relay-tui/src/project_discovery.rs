use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProject {
    pub repository_root: PathBuf,
    pub suggested_name: String,
    pub suggested_key: String,
    pub remote: Option<String>,
    pub github_owner: Option<String>,
    pub github_repo: Option<String>,
    pub suggested_stack: Option<String>,
}

pub fn discover_project(path: &Path) -> Result<DiscoveredProject, String> {
    let root = git_output(path, &["rev-parse", "--show-toplevel"])?;
    let repository_root = PathBuf::from(root.trim());
    let remote = git_output(&repository_root, &["remote", "get-url", "origin"])
        .ok()
        .map(|value| value.trim().to_owned());
    let name = repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_owned();
    let (github_owner, github_repo) = remote
        .as_deref()
        .and_then(parse_github_remote)
        .unwrap_or((None, None));
    Ok(DiscoveredProject {
        repository_root,
        suggested_name: name.clone(),
        suggested_key: suggest_key(&name),
        remote,
        github_owner,
        github_repo,
        suggested_stack: Some("chatgpt-codex".to_owned()),
    })
}

fn git_output(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| format!("Git discovery failed: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!("no Git repository detected at {}", path.display()))
    }
}

fn parse_github_remote(remote: &str) -> Option<(Option<String>, Option<String>)> {
    let value = remote.trim_end_matches('/').trim_end_matches(".git");
    let path = value
        .strip_prefix("https://github.com/")
        .or_else(|| value.strip_prefix("http://github.com/"))
        .or_else(|| value.strip_prefix("git@github.com:"))
        .or_else(|| value.strip_prefix("ssh://git@github.com/"))?;
    let mut parts = path.split('/');
    Some((
        Some(parts.next()?.to_owned()),
        Some(parts.next()?.to_owned()),
    ))
}

fn suggest_key(name: &str) -> String {
    name.chars()
        .fold(
            (String::new(), false),
            |(mut output, mut separator), character| {
                if character.is_ascii_alphanumeric() {
                    if separator && !output.is_empty() {
                        output.push('-');
                    }
                    output.push(character.to_ascii_lowercase());
                    separator = false;
                } else {
                    separator = true;
                }
                (output, separator)
            },
        )
        .0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_keys() {
        assert_eq!(suggest_key("My Project"), "my-project");
        assert_eq!(suggest_key("relay_runtime"), "relay-runtime");
    }
    #[test]
    fn parses_github_forms() {
        for remote in [
            "https://github.com/example/relay.git",
            "http://github.com/example/relay.git",
            "git@github.com:example/relay.git",
            "ssh://git@github.com/example/relay.git",
        ] {
            assert_eq!(
                parse_github_remote(remote),
                Some((Some("example".to_owned()), Some("relay".to_owned())))
            );
        }
        assert_eq!(
            parse_github_remote("ssh://git@example.internal/team/project.git"),
            None
        );
    }

    #[test]
    fn discovers_root_remote_and_metadata_without_registry_write() {
        let path = std::env::temp_dir().join(format!("relay-discovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args([
                    "remote",
                    "add",
                    "origin",
                    "git@github.com:example/relay.git"
                ])
                .status()
                .unwrap()
                .success()
        );
        let discovered = discover_project(&path).unwrap();
        // `git rev-parse --show-toplevel` may return a differently-spelled but
        // equivalent path (forward slashes, and on Windows CI runners the long
        // form of an 8.3 short name in `%TEMP%`), so compare canonical forms.
        assert_eq!(
            discovered.repository_root.canonicalize().unwrap(),
            path.canonicalize().unwrap()
        );
        assert_eq!(
            discovered.suggested_key,
            path.file_name().unwrap().to_string_lossy()
        );
        assert_eq!(discovered.github_owner.as_deref(), Some("example"));
        assert_eq!(discovered.github_repo.as_deref(), Some("relay"));
        let _ = std::fs::remove_dir_all(&path);
    }
}
