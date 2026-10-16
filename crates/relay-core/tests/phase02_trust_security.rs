//! Phase 02 Security Tests: Project, Checkout, and Trust Foundation
//!
//! Enforces:
//! 1. Cloned project claiming another ProjectId cannot silently hijack registration.
//! 2. Repo-supplied path rejected as trusted path authority.
//! 3. Remote mismatch fails closed.
//! 4. Repository policy can restrict but cannot broaden local hard policy.
//! 5. Repo cannot invoke arbitrary trusted executable action by string unless permitted.

use std::{fs, path::PathBuf};

use relay_core::{
    Checkout, CheckoutId, CheckoutTrustState, PortableConfigError, PortableRepositoryConfig,
    ProjectId, ProjectIdentityError, RemoteFingerprint, TrustedProjectRegistration,
};

fn unique_temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relay-phase02-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn cloned_project_claiming_another_project_id_cannot_silently_hijack_registration() {
    let repo_dir = unique_temp_repo("hijack-prevention");
    let relay_dir = repo_dir.join(".relay");
    fs::create_dir_all(&relay_dir).unwrap();

    // Attacker repository claims it is "victim-finance-app"
    fs::write(
        relay_dir.join("project.toml"),
        r#"
[project]
schema_version = 1
project_id = "victim-finance-app"
name = "Victim App Claim"
"#,
    )
    .unwrap();

    let loaded = PortableRepositoryConfig::load_from_dir(&repo_dir)
        .unwrap()
        .expect("loaded .relay configuration");

    // Local machine has registered "my-sandbox-project" for this checkout
    let local_pid = ProjectId::parse("my-sandbox-project").unwrap();
    let registration = TrustedProjectRegistration::new(local_pid.clone(), None, None, None);

    let checkout = Checkout::new(
        CheckoutId::parse("co-sandbox").unwrap(),
        local_pid,
        repo_dir.clone(),
        None,
        None,
        CheckoutTrustState::Trusted,
    )
    .unwrap();

    // Attempting to validate should fail closed with IdentityMismatch
    let result = loaded.validate_and_classify(&registration, &checkout);
    match result {
        Err(PortableConfigError::Identity(ProjectIdentityError::IdentityMismatch {
            declared,
            registered,
        })) => {
            assert_eq!(declared, "victim-finance-app");
            assert_eq!(registered, "my-sandbox-project");
        }
        other => panic!("expected IdentityMismatch, got {other:?}"),
    }

    let _ = fs::remove_dir_all(repo_dir);
}

#[test]
fn repo_supplied_path_rejected_as_trusted_path_authority() {
    let repo_dir = unique_temp_repo("path-injection");
    let relay_dir = repo_dir.join(".relay");
    fs::create_dir_all(&relay_dir).unwrap();

    // Attempt to inject machine-local paths into project.toml
    fs::write(
        relay_dir.join("project.toml"),
        r#"
[project]
schema_version = 1
project_id = "test-project"
name = "Path Injection Attack"
local_path = "C:\\Windows\\System32"
canonical_root = "/etc"
"#,
    )
    .unwrap();

    // The parser uses serde(deny_unknown_fields), so arbitrary path fields fail closed immediately
    let load_result = PortableRepositoryConfig::load_from_dir(&repo_dir);
    assert!(
        load_result.is_err(),
        "unrecognized path fields in portable config must be rejected"
    );

    let _ = fs::remove_dir_all(repo_dir);
}

#[test]
fn remote_mismatch_fails_closed() {
    let repo_dir = unique_temp_repo("remote-mismatch");

    let pid = ProjectId::parse("verified-repo").unwrap();
    let expected = RemoteFingerprint::new("https://github.com/legitimate-org/relay.git");

    let checkout = Checkout::new(
        CheckoutId::parse("co-verified").unwrap(),
        pid,
        repo_dir.clone(),
        Some(expected),
        None,
        CheckoutTrustState::Trusted,
    )
    .unwrap();

    // Correct remote passes
    assert!(
        checkout
            .verify_remote("https://github.com/legitimate-org/relay")
            .is_ok()
    );

    // Attacker's fork or spoofed remote fails closed
    let mismatch = checkout.verify_remote("https://github.com/attacker-org/relay.git");
    match mismatch {
        Err(ProjectIdentityError::RemoteMismatch { expected, actual }) => {
            assert!(expected.contains("legitimate-org"));
            assert!(actual.contains("attacker-org"));
        }
        other => panic!("expected RemoteMismatch, got {other:?}"),
    }

    let _ = fs::remove_dir_all(repo_dir);
}

#[test]
fn repository_policy_can_restrict_but_cannot_broaden_local_hard_policy() {
    let repo_dir = unique_temp_repo("policy-clamping");
    let relay_dir = repo_dir.join(".relay");
    fs::create_dir_all(&relay_dir).unwrap();

    // Repo declares policy restrictions
    fs::write(
        relay_dir.join("policies.toml"),
        r#"
[restrictions]
max_duration_seconds = 180
max_repair_attempts = 1
disallowed_tools = ["bash", "sh", "powershell"]
"#,
    )
    .unwrap();

    let loaded = PortableRepositoryConfig::load_from_dir(&repo_dir)
        .unwrap()
        .expect("loaded .relay configuration");

    // Local hard policy ceilings
    let local_duration_ceiling = 600u64;
    let local_repair_ceiling = 3u32;

    // 1. Repository policy can restrict/lower the bounds:
    assert_eq!(
        loaded.clamp_duration(local_duration_ceiling),
        180,
        "repo restriction must tighten duration"
    );
    assert_eq!(
        loaded.clamp_repair_attempts(local_repair_ceiling),
        1,
        "repo restriction must tighten repair attempts"
    );

    // 2. Repository policy cannot broaden beyond local policy if local is stricter:
    let strict_local_duration = 60u64;
    assert_eq!(
        loaded.clamp_duration(strict_local_duration),
        60,
        "stricter local ceiling must never be expanded by repo policy"
    );

    let _ = fs::remove_dir_all(repo_dir);
}

#[test]
fn repo_cannot_invoke_arbitrary_trusted_executable_action_by_string_unless_permitted() {
    let repo_dir = unique_temp_repo("tool-restriction");
    let relay_dir = repo_dir.join(".relay");
    fs::create_dir_all(&relay_dir).unwrap();

    fs::write(
        relay_dir.join("policies.toml"),
        r#"
[restrictions]
disallowed_tools = ["sudo", "curl", "arbitrary_script"]
"#,
    )
    .unwrap();

    let loaded = PortableRepositoryConfig::load_from_dir(&repo_dir)
        .unwrap()
        .expect("loaded .relay configuration");

    assert!(!loaded.is_tool_allowed("sudo"));
    assert!(!loaded.is_tool_allowed("SUDO"));
    assert!(!loaded.is_tool_allowed("curl"));
    assert!(!loaded.is_tool_allowed("arbitrary_script"));
    assert!(loaded.is_tool_allowed("cargo"));
    assert!(loaded.is_tool_allowed("git"));

    let _ = fs::remove_dir_all(repo_dir);
}
