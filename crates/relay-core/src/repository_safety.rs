use std::{
    fmt, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{ProjectKey, TrustedProject};

const DEFAULT_REMOTE_NAME: &str = "origin";

/// Read-only repository preflight result for a trusted Relay project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInspection {
    pub project_key: ProjectKey,
    pub local_path: PathBuf,
    pub normalized_expected_remote: Option<NormalizedRemote>,
    pub normalized_actual_remote: Option<NormalizedRemote>,
    pub status: Option<RepositoryStatus>,
    pub eligibility: RepositoryEligibility,
}

impl RepositoryInspection {
    pub fn is_eligible(&self) -> bool {
        matches!(self.eligibility, RepositoryEligibility::Eligible)
    }

    fn eligible(
        project: &TrustedProject,
        expected: NormalizedRemote,
        actual: NormalizedRemote,
        status: RepositoryStatus,
    ) -> Self {
        Self {
            project_key: project.key().clone(),
            local_path: project.local_path().to_path_buf(),
            normalized_expected_remote: Some(expected),
            normalized_actual_remote: Some(actual),
            status: Some(status),
            eligibility: RepositoryEligibility::Eligible,
        }
    }

    fn not_eligible(project: &TrustedProject, reason: RepositorySafetyFailure) -> Self {
        Self::not_eligible_with_remotes(project, None, None, reason)
    }

    fn not_eligible_with_remotes(
        project: &TrustedProject,
        expected: Option<NormalizedRemote>,
        actual: Option<NormalizedRemote>,
        reason: RepositorySafetyFailure,
    ) -> Self {
        Self {
            project_key: project.key().clone(),
            local_path: project.local_path().to_path_buf(),
            normalized_expected_remote: expected,
            normalized_actual_remote: actual,
            status: None,
            eligibility: RepositoryEligibility::NotEligible(reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryEligibility {
    Eligible,
    NotEligible(RepositorySafetyFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositorySafetyFailure {
    GitUnavailable,
    NotGitWorktree,
    MissingExpectedRemote,
    MissingRemote {
        name: String,
    },
    GitCommandFailed {
        operation: GitInspectionOperation,
        exit_code: Option<i32>,
    },
    MalformedExpectedRemote {
        remote: String,
    },
    MalformedActualRemote {
        remote: String,
    },
    RemoteMismatch {
        expected: NormalizedRemote,
        actual: NormalizedRemote,
    },
    UnexpectedGitOutput {
        operation: GitInspectionOperation,
        output: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitInspectionOperation {
    Version,
    WorktreeCheck,
    RemoteUrl,
    BranchName,
    HeadCommit,
    StatusPorcelain,
}

/// Normalized remote identity used only for comparison.
///
/// Hosts are lowercased because DNS names are case-insensitive. Repository
/// paths are lowercased only for `github.com`, matching GitHub's owner/repo
/// behavior. For all other hosts, path casing is preserved to avoid assuming a
/// self-hosted Git service's repository identity rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRemote {
    host: String,
    repository_path: String,
}

impl NormalizedRemote {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn repository_path(&self) -> &str {
        &self.repository_path
    }

    pub fn identity(&self) -> String {
        format!("{}/{}", self.host, self.repository_path)
    }
}

impl fmt::Display for NormalizedRemote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.host, self.repository_path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryStatus {
    pub branch_state: BranchState,
    pub clean: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchState {
    Branch(String),
    Detached { commit: String },
}

pub fn inspect_trusted_project_repository(project: &TrustedProject) -> RepositoryInspection {
    if let Err(reason) = ensure_git_available() {
        return RepositoryInspection::not_eligible(project, reason);
    }

    if let Err(reason) = ensure_worktree(project.local_path()) {
        return RepositoryInspection::not_eligible(project, reason);
    }

    let Some(expected_remote) = project.expected_remote() else {
        return RepositoryInspection::not_eligible(
            project,
            RepositorySafetyFailure::MissingExpectedRemote,
        );
    };

    let expected = match normalize_remote(expected_remote.as_str()) {
        Ok(remote) => remote,
        Err(()) => {
            return RepositoryInspection::not_eligible(
                project,
                RepositorySafetyFailure::MalformedExpectedRemote {
                    remote: expected_remote.as_str().to_owned(),
                },
            );
        }
    };

    let actual_remote = match git_output(
        Some(project.local_path()),
        GitInspectionOperation::RemoteUrl,
        ["config", "--get", "remote.origin.url"],
    ) {
        Ok(remote) if !remote.trim().is_empty() => remote,
        Ok(_) | Err(GitOutputError::CommandFailed { .. }) => {
            return RepositoryInspection::not_eligible_with_remotes(
                project,
                Some(expected),
                None,
                RepositorySafetyFailure::MissingRemote {
                    name: DEFAULT_REMOTE_NAME.to_owned(),
                },
            );
        }
        Err(error) => {
            return RepositoryInspection::not_eligible_with_remotes(
                project,
                Some(expected),
                None,
                error.into_failure(GitInspectionOperation::RemoteUrl),
            );
        }
    };

    let actual = match normalize_remote(&actual_remote) {
        Ok(remote) => remote,
        Err(()) => {
            return RepositoryInspection::not_eligible_with_remotes(
                project,
                Some(expected),
                None,
                RepositorySafetyFailure::MalformedActualRemote {
                    remote: actual_remote.trim().to_owned(),
                },
            );
        }
    };

    if expected != actual {
        return RepositoryInspection::not_eligible_with_remotes(
            project,
            Some(expected.clone()),
            Some(actual.clone()),
            RepositorySafetyFailure::RemoteMismatch { expected, actual },
        );
    }

    let status = match inspect_status(project.local_path()) {
        Ok(status) => status,
        Err(reason) => {
            return RepositoryInspection::not_eligible_with_remotes(
                project,
                Some(expected),
                Some(actual),
                reason,
            );
        }
    };

    RepositoryInspection::eligible(project, expected, actual, status)
}

fn ensure_git_available() -> Result<(), RepositorySafetyFailure> {
    let output = git_output(None, GitInspectionOperation::Version, ["--version"])
        .map_err(|_| RepositorySafetyFailure::GitUnavailable)?;

    if output.starts_with("git version ") {
        Ok(())
    } else {
        Err(RepositorySafetyFailure::GitUnavailable)
    }
}

fn ensure_worktree(path: &Path) -> Result<(), RepositorySafetyFailure> {
    match git_output(
        Some(path),
        GitInspectionOperation::WorktreeCheck,
        ["rev-parse", "--is-inside-work-tree"],
    ) {
        Ok(output) if output == "true" => Ok(()),
        Ok(output) if output == "false" => Err(RepositorySafetyFailure::NotGitWorktree),
        Ok(output) => Err(RepositorySafetyFailure::UnexpectedGitOutput {
            operation: GitInspectionOperation::WorktreeCheck,
            output,
        }),
        Err(GitOutputError::CommandFailed { .. }) => Err(RepositorySafetyFailure::NotGitWorktree),
        Err(error) => Err(error.into_failure(GitInspectionOperation::WorktreeCheck)),
    }
}

fn inspect_status(path: &Path) -> Result<RepositoryStatus, RepositorySafetyFailure> {
    let branch = git_output(
        Some(path),
        GitInspectionOperation::BranchName,
        ["branch", "--show-current"],
    )
    .map_err(|error| error.into_failure(GitInspectionOperation::BranchName))?;

    let branch_state = if branch.is_empty() {
        let commit = git_output(
            Some(path),
            GitInspectionOperation::HeadCommit,
            ["rev-parse", "--short", "HEAD"],
        )
        .map_err(|error| error.into_failure(GitInspectionOperation::HeadCommit))?;
        if commit.is_empty() || commit.chars().any(char::is_whitespace) {
            return Err(RepositorySafetyFailure::UnexpectedGitOutput {
                operation: GitInspectionOperation::HeadCommit,
                output: commit,
            });
        }
        BranchState::Detached { commit }
    } else {
        BranchState::Branch(branch)
    };

    let porcelain = git_output(
        Some(path),
        GitInspectionOperation::StatusPorcelain,
        ["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .map_err(|error| error.into_failure(GitInspectionOperation::StatusPorcelain))?;

    Ok(RepositoryStatus {
        branch_state,
        clean: porcelain.is_empty(),
    })
}

fn normalize_remote(remote: &str) -> Result<NormalizedRemote, ()> {
    let trimmed = remote.trim();
    if trimmed.is_empty()
        || trimmed
            .chars()
            .any(|char| char.is_control() || char.is_whitespace())
    {
        return Err(());
    }

    if let Some((scheme, rest)) = trimmed.split_once("://") {
        return normalize_scheme_remote(&scheme.to_ascii_lowercase(), rest);
    }

    normalize_scp_like_remote(trimmed)
}

fn normalize_scheme_remote(scheme: &str, rest: &str) -> Result<NormalizedRemote, ()> {
    if !matches!(scheme, "https" | "ssh") {
        return Err(());
    }

    let (authority, path) = rest.split_once('/').ok_or(())?;
    let host = match scheme {
        "https" if authority.contains('@') => return Err(()),
        "ssh" => authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host),
        _ => authority,
    };

    build_normalized_remote(host, path)
}

fn normalize_scp_like_remote(remote: &str) -> Result<NormalizedRemote, ()> {
    let (authority, path) = remote.split_once(':').ok_or(())?;
    if authority.len() == 1 || authority.contains('/') || authority.contains('\\') {
        return Err(());
    }

    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    build_normalized_remote(host, path)
}

fn build_normalized_remote(host: &str, path: &str) -> Result<NormalizedRemote, ()> {
    let host = host.trim().trim_matches(|char| char == '[' || char == ']');
    if host.is_empty() {
        return Err(());
    }

    let host = host.to_ascii_lowercase();
    let mut repository_path = path.trim_matches('/').to_owned();
    if repository_path.ends_with(".git") {
        repository_path.truncate(repository_path.len() - ".git".len());
    }

    if repository_path.is_empty()
        || repository_path.contains('\\')
        || repository_path.contains('?')
        || repository_path.contains('#')
    {
        return Err(());
    }

    let segments = repository_path.split('/').collect::<Vec<_>>();
    if segments.len() < 2
        || segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        return Err(());
    }

    if host == "github.com" {
        repository_path = repository_path.to_ascii_lowercase();
    }

    Ok(NormalizedRemote {
        host,
        repository_path,
    })
}

#[derive(Debug)]
enum GitOutputError {
    Io(io::Error),
    CommandFailed { exit_code: Option<i32> },
    MalformedUtf8,
}

impl GitOutputError {
    fn into_failure(self, operation: GitInspectionOperation) -> RepositorySafetyFailure {
        match self {
            Self::Io(error) if error.kind() == io::ErrorKind::NotFound => {
                RepositorySafetyFailure::GitUnavailable
            }
            Self::Io(_) | Self::MalformedUtf8 => RepositorySafetyFailure::UnexpectedGitOutput {
                operation,
                output: String::new(),
            },
            Self::CommandFailed { exit_code } => RepositorySafetyFailure::GitCommandFailed {
                operation,
                exit_code,
            },
        }
    }
}

fn git_output<const N: usize>(
    path: Option<&Path>,
    _operation: GitInspectionOperation,
    args: [&str; N],
) -> Result<String, GitOutputError> {
    let mut command = Command::new("git");

    if let Some(path) = path {
        command.arg("-C").arg(path);
    }
    command.args(args);

    let output = command.output().map_err(GitOutputError::Io)?;
    if !output.status.success() {
        return Err(GitOutputError::CommandFailed {
            exit_code: output.status.code(),
        });
    }

    String::from_utf8(output.stdout)
        .map(|output| output.trim().to_owned())
        .map_err(|_| GitOutputError::MalformedUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectRegistry, RegisterProject};
    use std::{
        env, fs,
        process::{Command, Output},
        time::{SystemTime, UNIX_EPOCH},
    };

    const EXPECTED_HTTPS: &str = "https://github.com/Jacinth091/Relay.git";
    const EXPECTED_NORMALIZED: &str = "github.com/jacinth091/relay";

    #[test]
    fn valid_normal_git_repository_is_eligible() {
        let repo = temp_git_repo("valid-normal");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);
        commit_empty(&repo);

        let inspection = inspect_project(&repo, Some(EXPECTED_HTTPS));

        assert!(inspection.is_eligible());
        assert_eq!(
            inspection.normalized_expected_remote.unwrap().identity(),
            EXPECTED_NORMALIZED
        );
        assert_eq!(
            inspection.normalized_actual_remote.unwrap().identity(),
            EXPECTED_NORMALIZED
        );
        assert_eq!(
            inspection.status.unwrap(),
            RepositoryStatus {
                branch_state: BranchState::Branch(current_branch(&repo)),
                clean: true,
            }
        );
    }

    #[test]
    fn non_git_directory_fails_closed() {
        let dir = temp_dir("non-git");
        fs::write(dir.join(".git"), "not a git repository").unwrap();

        let inspection = inspect_project(&dir, Some(EXPECTED_HTTPS));

        assert_eq!(
            inspection.eligibility,
            RepositoryEligibility::NotEligible(RepositorySafetyFailure::NotGitWorktree)
        );
    }

    #[test]
    fn missing_expected_remote_fails_closed() {
        let repo = temp_git_repo("missing-expected");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);
        commit_empty(&repo);

        let inspection = inspect_project(&repo, None);

        assert_eq!(
            inspection.eligibility,
            RepositoryEligibility::NotEligible(RepositorySafetyFailure::MissingExpectedRemote)
        );
    }

    #[test]
    fn missing_origin_remote_fails_closed() {
        let repo = temp_git_repo("missing-origin");
        commit_empty(&repo);

        let inspection = inspect_project(&repo, Some(EXPECTED_HTTPS));

        assert_eq!(
            inspection.eligibility,
            RepositoryEligibility::NotEligible(RepositorySafetyFailure::MissingRemote {
                name: "origin".to_owned(),
            })
        );
    }

    #[test]
    fn exact_remote_match_is_eligible() {
        let repo = temp_git_repo("exact-match");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);
        commit_empty(&repo);

        assert!(inspect_project(&repo, Some(EXPECTED_HTTPS)).is_eligible());
    }

    #[test]
    fn https_and_ssh_github_remotes_are_equivalent() {
        let repo = temp_git_repo("ssh-equivalent");
        git(
            &repo,
            [
                "remote",
                "add",
                "origin",
                "git@github.com:Jacinth091/Relay.git",
            ],
        );
        commit_empty(&repo);

        assert!(inspect_project(&repo, Some(EXPECTED_HTTPS)).is_eligible());
    }

    #[test]
    fn ssh_url_github_remote_is_equivalent() {
        let repo = temp_git_repo("ssh-url-equivalent");
        git(
            &repo,
            [
                "remote",
                "add",
                "origin",
                "ssh://git@github.com/Jacinth091/Relay.git",
            ],
        );
        commit_empty(&repo);

        assert!(inspect_project(&repo, Some(EXPECTED_HTTPS)).is_eligible());
    }

    #[test]
    fn git_suffix_equivalence_is_eligible() {
        let repo = temp_git_repo("suffix-equivalent");
        git(
            &repo,
            [
                "remote",
                "add",
                "origin",
                "https://github.com/Jacinth091/Relay",
            ],
        );
        commit_empty(&repo);

        assert!(inspect_project(&repo, Some(EXPECTED_HTTPS)).is_eligible());
    }

    #[test]
    fn wrong_repository_fails_closed() {
        let repo = temp_git_repo("wrong-repository");
        git(
            &repo,
            [
                "remote",
                "add",
                "origin",
                "https://github.com/Jacinth091/Other.git",
            ],
        );
        commit_empty(&repo);

        let inspection = inspect_project(&repo, Some(EXPECTED_HTTPS));

        assert!(matches!(
            inspection.eligibility,
            RepositoryEligibility::NotEligible(RepositorySafetyFailure::RemoteMismatch { .. })
        ));
    }

    #[test]
    fn wrong_host_fails_closed() {
        let repo = temp_git_repo("wrong-host");
        git(
            &repo,
            [
                "remote",
                "add",
                "origin",
                "https://gitlab.com/Jacinth091/Relay.git",
            ],
        );
        commit_empty(&repo);

        let inspection = inspect_project(&repo, Some(EXPECTED_HTTPS));

        assert!(matches!(
            inspection.eligibility,
            RepositoryEligibility::NotEligible(RepositorySafetyFailure::RemoteMismatch { .. })
        ));
    }

    #[test]
    fn malformed_expected_remote_fails_closed() {
        let repo = temp_git_repo("malformed-expected");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);
        commit_empty(&repo);

        let inspection = inspect_project(&repo, Some("github.com/Jacinth091/Relay"));

        assert_eq!(
            inspection.eligibility,
            RepositoryEligibility::NotEligible(RepositorySafetyFailure::MalformedExpectedRemote {
                remote: "github.com/Jacinth091/Relay".to_owned(),
            })
        );
    }

    #[test]
    fn malformed_actual_remote_fails_closed() {
        let repo = temp_git_repo("malformed-actual");
        git(&repo, ["remote", "add", "origin", "not a remote"]);
        commit_empty(&repo);

        let inspection = inspect_project(&repo, Some(EXPECTED_HTTPS));

        assert_eq!(
            inspection.eligibility,
            RepositoryEligibility::NotEligible(RepositorySafetyFailure::MalformedActualRemote {
                remote: "not a remote".to_owned(),
            })
        );
    }

    #[test]
    fn unborn_repository_branch_status_is_reported() {
        let repo = temp_git_repo("unborn-status");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);

        let inspection = inspect_project(&repo, Some(EXPECTED_HTTPS));

        assert!(inspection.is_eligible());
        assert_eq!(
            inspection.status.unwrap(),
            RepositoryStatus {
                branch_state: BranchState::Branch(current_branch(&repo)),
                clean: true,
            }
        );
    }

    #[test]
    fn path_with_spaces_is_inspected() {
        let repo = temp_git_repo("path with spaces");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);
        commit_empty(&repo);

        assert!(inspect_project(&repo, Some(EXPECTED_HTTPS)).is_eligible());
    }

    #[test]
    fn shell_metacharacters_in_path_remain_inert() {
        let repo = temp_git_repo("path ; $(relay) & data");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);
        commit_empty(&repo);

        assert!(inspect_project(&repo, Some(EXPECTED_HTTPS)).is_eligible());
    }

    #[test]
    fn dirty_repository_status_is_reported_without_rejection() {
        let repo = temp_git_repo("dirty-status");
        git(&repo, ["remote", "add", "origin", EXPECTED_HTTPS]);
        commit_empty(&repo);
        fs::write(repo.join("dirty.txt"), "local change").unwrap();

        let inspection = inspect_project(&repo, Some(EXPECTED_HTTPS));

        assert!(inspection.is_eligible());
        assert!(!inspection.status.unwrap().clean);
    }

    #[test]
    fn non_github_path_case_is_preserved() {
        let upper = normalize_remote("https://git.example.test/Team/Relay.git").unwrap();
        let lower = normalize_remote("https://git.example.test/team/relay.git").unwrap();

        assert_ne!(upper, lower);
        assert_eq!(upper.identity(), "git.example.test/Team/Relay");
    }

    fn inspect_project(path: &Path, expected_remote: Option<&str>) -> RepositoryInspection {
        let mut registry = ProjectRegistry::empty();
        registry
            .register(RegisterProject {
                key: "relay".to_owned(),
                name: None,
                local_path: path.to_path_buf(),
                expected_remote: expected_remote.map(str::to_owned),
                default_stack: None,
                github: None,
            })
            .unwrap();
        let project = registry.get("relay").unwrap();

        inspect_trusted_project_repository(project)
    }

    fn temp_git_repo(name: &str) -> PathBuf {
        let repo = temp_dir(name);
        git(&repo, ["init"]);
        repo
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path = unique_temp_path(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("relay-repository-safety-{name}-{nanos}"))
    }

    fn commit_empty(repo: &Path) {
        git(
            repo,
            [
                "-c",
                "user.name=Relay Test",
                "-c",
                "user.email=relay@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
    }

    fn current_branch(repo: &Path) -> String {
        stdout(git_output_for_test(repo, ["branch", "--show-current"]))
    }

    fn git<const N: usize>(repo: &Path, args: [&str; N]) {
        let output = git_output_for_test(repo, args);
        assert!(
            output.status.success(),
            "git failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output_for_test<const N: usize>(repo: &Path, args: [&str; N]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap()
    }

    fn stdout(output: Output) -> String {
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}
