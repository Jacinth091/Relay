//! Wave B (R12-02) Run-path cutover acceptance tests.
//!
//! Enforces the acceptance criteria for cutting the production Run path over to
//! the v2 orchestration engine:
//! 1. A durable `Ready` handoff reaches `COMPLETE` entirely through the daemon,
//!    the v2 orchestrator, and a `RuntimeBackplane` (no hand-driven phases).
//! 2. Process exit success alone never completes a role: the attempt must
//!    publish a typed result object, and a malformed or missing result cannot
//!    advance it.
//! 3. A verifier FAIL enters bounded remediation and a fresh generation reaches
//!    `COMPLETE`.
//! 4. A restarted daemon reconciles against the runtime handle the attempt was
//!    actually launched as, never a guessed one.
//! 5. Approving a Run is one durable fact: replaying the approval converges on
//!    exactly one run and one start event, and a daemon that lost its lease
//!    cannot start or drive anything.
//!
//! `daemon_runtime` (and therefore the workflow driver) is Windows-only, so
//! these tests are compiled only there. Linux portability is tracked in #17.

#![cfg(windows)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use relay_core::{
    ApprovedIntent, ArtifactStore, CheckoutId, CommandStatus, ConfigSnapshot, DaemonCommand,
    DaemonRequest, DaemonResponse, DaemonServer, ExecutionBudget, ExecutionGrant, ExecutionTarget,
    OrchestratorError, OrchestratorV2, PhaseContext, ProjectId, ProjectKey, ProjectRegistry,
    RegisterProject, Relay, RoleAttemptState, RoleDefinitionId, RoleInstance, SchedulerV2,
    SchedulerV2Config, SqliteStore, StoredTask, SubmissionTransport, TaskId, TaskState,
    WorkflowDriverContext, WorkflowRun, WorkflowRunContextResolver, WorkflowRunId,
    WorkflowRunState, adapt_v1_handoff_to_approved_intent, create_v1_compatibility_run,
    get_durable_workflow_run, get_stored_task, list_role_attempts_for_instance,
    list_role_instances_for_run, persist_role_attempt_if_absent, persist_role_instance_if_absent,
    record_role_attempt_runtime_handle, run_full_daemon, run_full_daemon_with_workflow_driver,
};
use relay_protocol::{AmbiguityPolicy, HandoffTask, TaskSpec};
use relay_runtime::{
    EffectKey, LaunchSpecification, MockProviderAdapter, ProviderAdapter, ProviderCapabilities,
    RuntimeBackplane, RuntimeProvisionRequest, SanitizedEnvironment,
};
use relay_runtime_fake::{FakeBackplane, FakeProcessBehavior};
use relay_store::{DaemonFence, NewTaskRecord, NewWorkflowRunRecord, WorkflowRunStart};

const IMPLEMENTER: &str = "relay-implementer";
const VERIFIER: &str = "relay-verifier";

fn unique_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relay-waveb-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn init_git_repo(path: &Path) {
    let run = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(path);
        cmd.args(args);
        cmd.env_remove("GIT_DIR");
        cmd.env_remove("GIT_WORK_TREE");
        cmd.env_remove("GIT_COMMON_DIR");
        let out = cmd.output().expect("git command failed");
        assert!(out.status.success(), "git command {args:?} failed");
    };

    run(&["init"]);
    run(&["config", "user.name", "Relay Wave B"]);
    run(&["config", "user.email", "waveb@relay.local"]);
    fs::write(path.join("README.md"), "# Initial\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial commit"]);
}

fn budget() -> ExecutionBudget {
    ExecutionBudget {
        max_role_activations: 5,
        max_attempts_per_role: 3,
        max_delegations: 2,
        max_delegation_depth: 2,
        max_concurrent_attempts: 1,
        max_wall_clock_secs: Some(300),
    }
}

fn fake_target() -> ExecutionTarget {
    ExecutionTarget {
        provider_id: "fake".to_owned(),
        runtime_profile_id: "fake/waveb".to_owned(),
        provider_adapter_version: "1.0.0".to_owned(),
    }
}

/// The trusted execution context for the one project these tests register.
///
/// Production resolves this from the frozen `ConfigSnapshot` plus the project
/// registry; this test double is handed the registered workspace directly so
/// the workspace can never come from task input.
struct FixedContextResolver {
    project_id: ProjectId,
    working_directory: PathBuf,
}

impl WorkflowRunContextResolver for FixedContextResolver {
    fn resolve(&self, _run: &WorkflowRun) -> Result<WorkflowDriverContext, String> {
        Ok(WorkflowDriverContext {
            project_id: self.project_id.clone(),
            checkout_id: CheckoutId::generate(),
            working_directory: self.working_directory.clone(),
            execution_target: fake_target(),
            budget: budget(),
        })
    }
}

const TEST_HANDOFF: &str = r#"{
    "protocol_version": 1,
    "project": "wave-b-proj",
    "title": "Wave B run-path cutover",
    "task": { "goal": "Reach COMPLETE through the v2 orchestrator" },
    "ambiguity_policy": "needs_clarification"
}"#;

fn test_relay(project_path: &Path) -> Relay {
    let mut registry = ProjectRegistry::empty();
    registry
        .register(RegisterProject {
            key: "wave-b-proj".to_owned(),
            name: None,
            local_path: project_path.to_path_buf(),
            expected_remote: None,
            default_stack: None,
            github: None,
        })
        .unwrap();
    Relay::with_project_registry(registry)
}

fn sample_task() -> StoredTask {
    StoredTask {
        id: TaskId::generate(),
        handoff: HandoffTask {
            protocol_version: 1,
            project: "wave-b-proj".to_owned(),
            title: "Wave B run-path cutover".to_owned(),
            stack: None,
            source: None,
            task: TaskSpec {
                goal: "Reach COMPLETE through the v2 orchestrator".to_owned(),
                requirements: vec!["Publish a typed result".to_owned()],
                constraints: vec!["Deterministic".to_owned()],
                verification: vec!["cargo test".to_owned()],
            },
            ambiguity_policy: AmbiguityPolicy::NeedsClarification,
            extensions: std::collections::BTreeMap::new(),
        },
        state: TaskState::Ready,
        project_key: ProjectKey::parse("wave-b-proj").unwrap(),
        submission_transport: SubmissionTransport::LocalPaste,
        created_at: "now".to_owned(),
        updated_at: "now".to_owned(),
    }
}

fn published_implementer_result(detail: &str) -> FakeProcessBehavior {
    FakeProcessBehavior::succeeding_with(
        format!("{{\"role_result\":\"implemented\",\"detail\":\"{detail}\"}}\n").into_bytes(),
    )
}

fn published_verifier_result(passed: bool, reason: &str) -> FakeProcessBehavior {
    if passed {
        FakeProcessBehavior::succeeding_with(
            b"{\"role_result\":\"verification_passed\"}\n".to_vec(),
        )
    } else {
        FakeProcessBehavior::succeeding_with(
            format!("{{\"role_result\":\"verification_failed\",\"reason\":\"{reason}\"}}\n")
                .into_bytes(),
        )
    }
}

/// Acquires the daemon lease and returns the exact fence a daemon holding it
/// would write with.
async fn lease_fence(store: &SqliteStore, lease_id: &str, pid: i64) -> DaemonFence {
    let lease = store
        .acquire_or_renew_daemon_lease(
            lease_id,
            pid,
            "waveb-host",
            &format!("waveb-inst-{pid}"),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    DaemonFence {
        lease_id: lease_id.to_owned(),
        holder_pid: lease.holder_pid,
        holder_instance: lease.holder_instance,
        fencing_token: lease.fencing_token,
    }
}

/// Seeds the durable task row a Run path needs.
///
/// Production persists the task when the handoff is imported; the direct
/// orchestrator fixtures below build the task in memory, so they must write the
/// parent row themselves. `workflow_runs.task_id` references `tasks(id)`, so the
/// row must use the task's own id.
async fn persist_task_row(store: &SqliteStore, task: &StoredTask, fence: &DaemonFence) {
    let record = NewTaskRecord {
        id: task.id.to_string(),
        project_key: task.project_key.as_str().to_owned(),
        title: task.handoff.title.clone(),
        requested_stack: task.handoff.stack.clone(),
        submission_transport: task.submission_transport.as_str().to_owned(),
        source_kind: task
            .handoff
            .source
            .as_ref()
            .map(|source| source.kind.clone()),
        source_provider: task
            .handoff
            .source
            .as_ref()
            .map(|source| source.provider.clone()),
        state: task.state.as_str().to_owned(),
        protocol_version: i64::from(task.handoff.protocol_version),
        handoff_json: serde_json::to_string(&task.handoff).unwrap(),
    };
    store
        .insert_task_if_absent_fenced(&record, fence)
        .await
        .unwrap();
}

async fn persist_approved_workflow_run(
    store: &SqliteStore,
    task: &StoredTask,
    intent: &ApprovedIntent,
    grant: &ExecutionGrant,
    workflow_run: &WorkflowRun,
    snapshot: &ConfigSnapshot,
    fence: &DaemonFence,
) {
    let run_record = NewWorkflowRunRecord {
        id: workflow_run.id.0.clone(),
        task_id: workflow_run.task_id.to_string(),
        execution_grant_id: workflow_run.execution_grant_id.0.clone(),
        workflow_definition_id: workflow_run.workflow_definition_id.0.clone(),
        state: "running".to_owned(),
        config_snapshot_id: workflow_run.config_snapshot_id.0.clone(),
    };
    let intent_json = serde_json::to_string(intent).unwrap();
    let grant_json = serde_json::to_string(grant).unwrap();
    let snapshot_json = serde_json::to_string(snapshot).unwrap();
    let event_json = serde_json::json!({
        "task_id": task.id.to_string(),
        "workflow_run_id": workflow_run.id.0,
        "execution_grant_id": grant.id.0,
        "config_snapshot_id": snapshot.id.0,
        "state": "running",
    })
    .to_string();

    store
        .start_workflow_run_fenced(
            &WorkflowRunStart {
                run: &run_record,
                approved_intent_json: &intent_json,
                execution_grant_json: &grant_json,
                config_snapshot_json: &snapshot_json,
                task_expected_state: "ready",
                task_new_state: "running",
                event_kind: "workflow_run_started",
                event_json: Some(&event_json),
            },
            fence,
        )
        .await
        .unwrap();
}

fn test_provider_adapters() -> Vec<Arc<dyn ProviderAdapter>> {
    let adapter: Arc<dyn ProviderAdapter> = MockProviderAdapter::new(
        "fake",
        ProviderCapabilities {
            supported_roles: ["implementer", "verifier"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            filesystem_write: true,
            network: false,
            streaming_output: true,
            cancellation: true,
            supported_models: vec!["default".to_owned()],
        },
    )
    .with_role_executable("implementer", IMPLEMENTER)
    .with_role_executable("verifier", VERIFIER)
    .arc();
    vec![adapter]
}

/// Submits a daemon command and returns the raw response.
///
/// `DaemonClient::submit_command` only reports the status, so these tests go
/// through the request path where the result payload - and fail-closed
/// refusals - are visible.
async fn submit_response(
    server: &DaemonServer,
    command_id: &str,
    command: DaemonCommand,
) -> DaemonResponse {
    server
        .handle_request(DaemonRequest::SubmitCommand {
            command_id: command_id.to_owned(),
            command,
            state_identity: server.state_identity().to_owned(),
        })
        .await
}

/// Submits a daemon command that must reach a terminal command result.
async fn submit(
    server: &DaemonServer,
    command_id: &str,
    command: DaemonCommand,
) -> (CommandStatus, Option<String>) {
    match submit_response(server, command_id, command).await {
        DaemonResponse::CommandResult {
            status,
            result_json,
            ..
        } => (status, result_json),
        other => panic!("expected a command result for '{command_id}', got {other:?}"),
    }
}

/// Submits a command that must be refused, and returns the refusal message.
async fn submit_refused(server: &DaemonServer, command_id: &str, command: DaemonCommand) -> String {
    match submit_response(server, command_id, command).await {
        DaemonResponse::Error(message) => message,
        other => panic!("expected a fail-closed refusal for '{command_id}', got {other:?}"),
    }
}

/// Submits a handoff, approves a Run through the daemon, and returns the durable
/// task and workflow run ids.
async fn approve_run(server: &DaemonServer, command_prefix: &str) -> (String, String) {
    let (status, result) = submit(
        server,
        &format!("{command_prefix}-submit"),
        DaemonCommand::SubmitHandoff {
            project_key: "wave-b-proj".to_owned(),
            handoff_json: TEST_HANDOFF.to_owned(),
            submission_transport: "local-paste".to_owned(),
        },
    )
    .await;
    assert_eq!(status, CommandStatus::Applied);
    let submitted_json: serde_json::Value =
        serde_json::from_str(result.as_deref().unwrap()).unwrap();
    assert_eq!(
        submitted_json["state"], "ready",
        "an imported handoff must be durable and Ready before a Run can be approved"
    );
    let task_id = submitted_json["id"].as_str().unwrap().to_owned();

    let (status, result) = submit(
        server,
        &format!("{command_prefix}-run"),
        DaemonCommand::RunTask {
            task_id: task_id.clone(),
            granted_by: "operator-alice".to_owned(),
        },
    )
    .await;
    assert_eq!(
        status,
        CommandStatus::Applied,
        "an explicit Run approval must be applied: {result:?}"
    );
    let run_json: serde_json::Value = serde_json::from_str(result.as_deref().unwrap()).unwrap();
    assert_eq!(
        run_json["state"], "running",
        "approving a Run must move the durable run (and task) into action"
    );
    let workflow_run_id = run_json["workflow_run_id"].as_str().unwrap().to_owned();

    (task_id, workflow_run_id)
}

/// Polls until `predicate` holds, so these tests never depend on driver timing.
async fn wait_for<F, Fut>(mut predicate: F, what: &str)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..400 {
        if predicate().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Every artifact blob payload stored under `root`, so assertions can prove the
/// artifact holds what the role process published.
fn blob_payloads(root: &Path) -> Vec<String> {
    let mut payloads = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return payloads;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            payloads.extend(blob_payloads(&path));
        } else if path.extension().is_some_and(|ext| ext == "blob") {
            payloads.push(fs::read_to_string(&path).unwrap_or_default());
        }
    }
    payloads
}

/// Durable name of a run state, matching the store's persisted spelling.
fn run_state_name(state: WorkflowRunState) -> &'static str {
    match state {
        WorkflowRunState::Pending => "pending",
        WorkflowRunState::Running => "running",
        WorkflowRunState::Suspended => "blocked",
        WorkflowRunState::WaitingForAttention => "waiting_decision",
        WorkflowRunState::Completed => "completed",
        WorkflowRunState::Failed => "failed",
        WorkflowRunState::Cancelled => "cancelled",
    }
}

async fn run_state(store: &SqliteStore, run_id: &str) -> String {
    run_state_name(
        get_durable_workflow_run(store, run_id)
            .await
            .unwrap()
            .expect("workflow run must be durable")
            .state,
    )
    .to_owned()
}

async fn stored_task_state(store: &SqliteStore, task_id: &str) -> TaskState {
    get_stored_task(store, TaskId::parse(task_id).unwrap())
        .await
        .unwrap()
        .expect("task must be durable")
        .state
}

async fn start_event_count(store: &SqliteStore) -> usize {
    store
        .get_events_after(0, 500)
        .await
        .unwrap()
        .iter()
        .filter(|event| event.event_kind == "workflow_run_started")
        .count()
}

#[tokio::test]
async fn a_ready_handoff_reaches_complete_through_daemon_and_backplane() {
    let base = unique_temp_dir("happy");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let artifact_root = base.join("artifacts");

    let store = SqliteStore::in_memory().await.unwrap();
    let backplane = Arc::new(FakeBackplane::new());
    backplane.script_process(
        IMPLEMENTER,
        published_implementer_result("wrote feature.txt").writing_file("feature.txt", "v1\n"),
    );
    backplane.script_process(VERIFIER, published_verifier_result(true, ""));

    let (server, daemon) = run_full_daemon_with_workflow_driver(
        store.clone(),
        test_relay(&repo),
        "lease-waveb-happy",
        Duration::from_secs(60),
        &artifact_root,
        Arc::clone(&backplane),
        Arc::new(FixedContextResolver {
            project_id: ProjectId::parse("wave-b-proj").unwrap(),
            working_directory: repo.clone(),
        }),
        test_provider_adapters(),
    )
    .await
    .unwrap();

    let (task_id, run_id) = approve_run(&server, "cmd-happy").await;

    let poll_store = store.clone();
    let poll_task = task_id.clone();
    let poll_run = run_id.clone();
    wait_for(
        || {
            let store = poll_store.clone();
            let task_id = poll_task.clone();
            let run_id = poll_run.clone();
            async move {
                stored_task_state(&store, &task_id).await == TaskState::Done
                    && run_state(&store, &run_id).await == "completed"
            }
        },
        "the completed run to publish the task's Done state",
    )
    .await;

    assert_eq!(
        stored_task_state(&store, &task_id).await,
        TaskState::Done,
        "a completed run must publish the task's user-facing state"
    );

    let instances = list_role_instances_for_run(&store, &WorkflowRunId(run_id.clone()))
        .await
        .unwrap();
    assert_eq!(
        instances.len(),
        2,
        "the happy path activates exactly one implementer and one verifier, got {instances:?}"
    );

    assert!(
        blob_payloads(&artifact_root)
            .iter()
            .any(|payload| payload.contains("\"detail\":\"wrote feature.txt\"")),
        "the implementation artifact must store the result the role published, not a summary"
    );

    daemon.shutdown().await;
}

#[tokio::test]
async fn verifier_failure_enters_bounded_remediation_and_completes() {
    let base = unique_temp_dir("remediation");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let artifact_root = base.join("artifacts");

    let store = SqliteStore::in_memory().await.unwrap();
    let backplane = Arc::new(FakeBackplane::new());
    backplane.script_process(
        IMPLEMENTER,
        published_implementer_result("first attempt").writing_file("feature.txt", "v1\n"),
    );
    backplane.script_process(VERIFIER, published_verifier_result(false, "missing test"));
    backplane.script_process(
        IMPLEMENTER,
        published_implementer_result("remediated").writing_file("feature.txt", "v2\n"),
    );
    backplane.script_process(VERIFIER, published_verifier_result(true, ""));

    let (server, daemon) = run_full_daemon_with_workflow_driver(
        store.clone(),
        test_relay(&repo),
        "lease-waveb-remediation",
        Duration::from_secs(60),
        &artifact_root,
        Arc::clone(&backplane),
        Arc::new(FixedContextResolver {
            project_id: ProjectId::parse("wave-b-proj").unwrap(),
            working_directory: repo.clone(),
        }),
        test_provider_adapters(),
    )
    .await
    .unwrap();

    let (_task_id, run_id) = approve_run(&server, "cmd-remediation").await;

    let poll_store = store.clone();
    let poll_run = run_id.clone();
    wait_for(
        || {
            let store = poll_store.clone();
            let run_id = poll_run.clone();
            async move { run_state(&store, &run_id).await == "completed" }
        },
        "the remediated workflow run to reach COMPLETE",
    )
    .await;

    let instances = list_role_instances_for_run(&store, &WorkflowRunId(run_id.clone()))
        .await
        .unwrap();
    assert_eq!(
        instances.len(),
        4,
        "one implementer and one verifier per generation, got {instances:?}"
    );

    // Generation 1's verifier must have genuinely reported FAIL, and generation
    // 2 must be a fresh pair rather than a retry of the same instance.
    let mut generations: Vec<u32> = instances.iter().map(|i| i.generation).collect();
    generations.sort_unstable();
    assert_eq!(generations, vec![1, 1, 2, 2]);

    let verifier_fail_instance = instances
        .iter()
        .find(|i| i.role_definition_id.0 == "verifier" && i.generation == 1)
        .expect("generation 1 must have a verifier instance");
    let attempts = list_role_attempts_for_instance(&store, &verifier_fail_instance.id)
        .await
        .unwrap();
    assert_eq!(
        attempts.first().and_then(|a| a.runtime_outcome.clone()),
        Some("FAIL".to_owned()),
        "a verifier that reports FAIL completes its own role with that outcome"
    );

    assert!(
        blob_payloads(&artifact_root)
            .iter()
            .any(|payload| payload.contains("verification_failed")),
        "the verification artifact must preserve the verifier's own FAIL result"
    );

    daemon.shutdown().await;
}

/// Shared setup for the direct-orchestrator tests: a real repo, a durable run,
/// a daemon fence, and an orchestrator wired to a scriptable backplane.
async fn orchestrator_fixture(
    name: &str,
) -> (
    SqliteStore,
    OrchestratorV2<FakeBackplane>,
    PathBuf,
    WorkflowRun,
    DaemonFence,
) {
    orchestrator_fixture_with_adapters(name, test_provider_adapters()).await
}

async fn orchestrator_fixture_with_adapters(
    name: &str,
    provider_adapters: Vec<Arc<dyn ProviderAdapter>>,
) -> (
    SqliteStore,
    OrchestratorV2<FakeBackplane>,
    PathBuf,
    WorkflowRun,
    DaemonFence,
) {
    let base = unique_temp_dir(name);
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);

    let store = SqliteStore::in_memory().await.unwrap();
    let artifact_root = base.join("artifacts");
    let backplane = Arc::new(FakeBackplane::new());

    let fence = lease_fence(&store, &format!("lease-{name}"), 7001).await;

    let task = sample_task();
    persist_task_row(&store, &task, &fence).await;
    let intent = adapt_v1_handoff_to_approved_intent(&task, "operator-alice").unwrap();
    let (grant, workflow_run, snapshot) =
        create_v1_compatibility_run(&task, &intent, budget(), "operator-alice").unwrap();
    persist_approved_workflow_run(
        &store,
        &task,
        &intent,
        &grant,
        &workflow_run,
        &snapshot,
        &fence,
    )
    .await;

    let orchestrator = OrchestratorV2::new(
        store.clone(),
        Arc::new(SchedulerV2::new(SchedulerV2Config::default())),
        Arc::clone(&backplane),
        ArtifactStore::new(&artifact_root).unwrap(),
        provider_adapters,
    );

    (store, orchestrator, repo, workflow_run, fence)
}

#[tokio::test]
async fn missing_provider_adapter_fails_attempt_without_provisioning() {
    let (store, orchestrator, repo, workflow_run, fence) =
        orchestrator_fixture_with_adapters("missing-provider-adapter", vec![]).await;
    let project_id = ProjectId::parse("wave-b-proj").unwrap();
    let checkout_id = CheckoutId::generate();
    let target = fake_target();
    let budget = budget();
    let ctx = PhaseContext {
        project_id: &project_id,
        checkout_id: &checkout_id,
        working_directory: &repo,
        execution_target: &target,
        budget: &budget,
    };

    let error = orchestrator
        .run_implementer_phase(&workflow_run, &ctx, 1, &fence)
        .await
        .expect_err("unregistered frozen provider must fail closed");
    assert!(
        matches!(error, OrchestratorError::ProviderAdapterMissing(ref provider) if provider == "fake"),
        "expected a clear missing-adapter error, got {error:?}"
    );

    let instances = list_role_instances_for_run(&store, &workflow_run.id)
        .await
        .unwrap();
    let attempts = list_role_attempts_for_instance(&store, &instances[0].id)
        .await
        .unwrap();
    assert_eq!(attempts[0].state, RoleAttemptState::Failed);
    assert!(attempts[0].runtime_handle_id.is_none());
    assert_eq!(
        orchestrator
            .backplane()
            .snapshot()
            .await
            .unwrap()
            .active_processes,
        0,
        "an unregistered provider must not provision a process"
    );
}

#[tokio::test]
async fn process_exit_success_without_a_published_result_cannot_advance_a_role() {
    let (store, orchestrator, repo, workflow_run, fence) =
        orchestrator_fixture("missing-result").await;

    // Exits 0 while publishing nothing a role result can be read from.
    orchestrator.backplane().script_process(
        IMPLEMENTER,
        FakeProcessBehavior::succeeding_with("no result here\n"),
    );

    let project_id = ProjectId::parse("wave-b-proj").unwrap();
    let checkout_id = CheckoutId::generate();
    let target = fake_target();
    let budget = budget();
    let ctx = PhaseContext {
        project_id: &project_id,
        checkout_id: &checkout_id,
        working_directory: &repo,
        execution_target: &target,
        budget: &budget,
    };

    let error = orchestrator
        .run_implementer_phase(&workflow_run, &ctx, 1, &fence)
        .await
        .expect_err("exit success without a published result must not complete a role");

    assert!(
        matches!(error, OrchestratorError::ResultMissing(_)),
        "expected a missing-result rejection, got {error:?}"
    );

    let instances = list_role_instances_for_run(&store, &workflow_run.id)
        .await
        .unwrap();
    let attempt = list_role_attempts_for_instance(&store, &instances[0].id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        attempt.state,
        RoleAttemptState::ResultPending,
        "a missing result leaves the attempt recoverable, never completed"
    );

    assert_eq!(
        run_state(&store, &workflow_run.id.0).await,
        "running",
        "a run whose role published nothing must not advance"
    );
}

#[tokio::test]
async fn malformed_result_fails_the_attempt_without_completing_the_run() {
    let (store, orchestrator, repo, workflow_run, fence) = orchestrator_fixture("malformed").await;

    // A result object that violates the contract: `implemented` requires detail.
    orchestrator.backplane().script_process(
        IMPLEMENTER,
        FakeProcessBehavior::succeeding_with("{\"role_result\":\"implemented\"}\n"),
    );

    let project_id = ProjectId::parse("wave-b-proj").unwrap();
    let checkout_id = CheckoutId::generate();
    let target = fake_target();
    let budget = budget();
    let ctx = PhaseContext {
        project_id: &project_id,
        checkout_id: &checkout_id,
        working_directory: &repo,
        execution_target: &target,
        budget: &budget,
    };

    let error = orchestrator
        .run_implementer_phase(&workflow_run, &ctx, 1, &fence)
        .await
        .expect_err("a malformed result must never be accepted as a role result");

    assert!(
        matches!(error, OrchestratorError::ResultMalformed(_)),
        "expected a malformed-result rejection, got {error:?}"
    );

    let instances = list_role_instances_for_run(&store, &workflow_run.id)
        .await
        .unwrap();
    let attempt = list_role_attempts_for_instance(&store, &instances[0].id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        attempt.state,
        RoleAttemptState::Failed,
        "a process that answers wrongly fails its attempt"
    );

    assert_eq!(
        run_state(&store, &workflow_run.id.0).await,
        "running",
        "a failed attempt must not advance the run on its own"
    );
}

#[tokio::test]
async fn restart_reconciliation_uses_the_recorded_runtime_handle() {
    let base = unique_temp_dir("restart");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);

    let store = SqliteStore::in_memory().await.unwrap();
    let backplane = Arc::new(FakeBackplane::new());
    let artifact_root = base.join("artifacts");

    let fence = lease_fence(&store, "lease-waveb-restart", 7002).await;

    let task = sample_task();
    persist_task_row(&store, &task, &fence).await;
    let intent = adapt_v1_handoff_to_approved_intent(&task, "operator-alice").unwrap();
    let (grant, workflow_run, snapshot) =
        create_v1_compatibility_run(&task, &intent, budget(), "operator-alice").unwrap();
    persist_approved_workflow_run(
        &store,
        &task,
        &intent,
        &grant,
        &workflow_run,
        &snapshot,
        &fence,
    )
    .await;

    let mut instance = RoleInstance::new(
        workflow_run.id.clone(),
        RoleDefinitionId::new("implementer"),
        1,
        None,
    );
    persist_role_instance_if_absent(&store, &instance, &fence)
        .await
        .unwrap();

    // Attempt 1 was launched as a real handle this backplane owns, under a
    // launch id that is deliberately *not* derived from the attempt id: only a
    // daemon that recorded the real handle can still see it.
    let launch_id = format!("external-launch-{}-1", workflow_run.id.0);
    let handle = backplane
        .provision(RuntimeProvisionRequest::new(
            EffectKey::new("effect-restart-1"),
            launch_id,
            &repo,
            LaunchSpecification::new(IMPLEMENTER, vec![]),
            SanitizedEnvironment::new(),
            1024 * 1024,
        ))
        .await
        .unwrap();

    let mut live_attempt = instance.spawn_attempt(fake_target(), &budget()).unwrap();
    live_attempt.state = RoleAttemptState::Running;
    persist_role_attempt_if_absent(&store, &live_attempt, &fence)
        .await
        .unwrap();
    live_attempt.runtime_handle_id = Some(handle.handle_id.clone());
    live_attempt.runtime_backplane_id = Some(handle.backplane_id.to_string());
    record_role_attempt_runtime_handle(&store, &live_attempt, &fence)
        .await
        .unwrap();

    // Attempt 2 crashed before its process identity was ever recorded.
    let mut unrecorded_attempt = instance.spawn_attempt(fake_target(), &budget()).unwrap();
    unrecorded_attempt.state = RoleAttemptState::Running;
    persist_role_attempt_if_absent(&store, &unrecorded_attempt, &fence)
        .await
        .unwrap();

    let orchestrator = OrchestratorV2::new(
        store.clone(),
        Arc::new(SchedulerV2::new(SchedulerV2Config::default())),
        Arc::clone(&backplane),
        ArtifactStore::new(&artifact_root).unwrap(),
        test_provider_adapters(),
    );

    orchestrator.reconcile_on_startup(&fence).await.unwrap();

    let attempts = list_role_attempts_for_instance(&store, &instance.id)
        .await
        .unwrap();
    let live = attempts
        .iter()
        .find(|attempt| attempt.id == live_attempt.id)
        .expect("live attempt must still be durable");
    assert_eq!(
        live.state,
        RoleAttemptState::Running,
        "an attempt whose process is still alive must stay running across a restart"
    );

    let unrecorded = attempts
        .iter()
        .find(|attempt| attempt.id == unrecorded_attempt.id)
        .expect("unrecorded attempt must still be durable");
    assert_eq!(
        unrecorded.state,
        RoleAttemptState::Lost,
        "an attempt with no recorded handle cannot be reconciled and must be marked lost"
    );
    assert!(
        unrecorded
            .runtime_outcome
            .as_deref()
            .is_some_and(|reason| reason.contains("no runtime handle")),
        "the loss must state why it could not be reconciled, got {:?}",
        unrecorded.runtime_outcome
    );
}

#[tokio::test]
async fn a_replayed_run_approval_converges_on_one_run_and_one_start_event() {
    let base = unique_temp_dir("replay");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);

    let store = SqliteStore::in_memory().await.unwrap();
    // No workflow driver: the assertion is about the durable approval, not the
    // execution that follows it.
    let (server, daemon) = run_full_daemon(
        store.clone(),
        test_relay(&repo),
        "lease-waveb-replay",
        Duration::from_secs(60),
    )
    .await
    .unwrap();

    let (task_id, run_id) = approve_run(&server, "cmd-replay").await;

    // Replaying the exact same command id reports the durable approval and
    // leaves the durable state untouched.
    let (status, replay_result) = submit(
        &server,
        "cmd-replay-run",
        DaemonCommand::RunTask {
            task_id: task_id.clone(),
            granted_by: "operator-alice".to_owned(),
        },
    )
    .await;
    assert_eq!(
        status,
        CommandStatus::Duplicate,
        "a replayed approval must be recognised as a duplicate, not re-applied"
    );
    let replay_json: serde_json::Value =
        serde_json::from_str(replay_result.as_deref().unwrap()).unwrap();
    assert_eq!(
        replay_json["workflow_run_id"].as_str().unwrap(),
        run_id,
        "a replay must report the same durable run"
    );

    assert_eq!(
        store.list_active_workflow_runs().await.unwrap().len(),
        1,
        "a replayed approval must not create a second run"
    );
    assert_eq!(
        start_event_count(&store).await,
        1,
        "a replayed approval must not append a second start event"
    );
    assert_eq!(
        stored_task_state(&store, &task_id).await,
        TaskState::Running
    );

    // Reusing the id for a different command fails closed instead of silently
    // reporting the earlier command's outcome.
    let refused_message = submit_refused(
        &server,
        "cmd-replay-run",
        DaemonCommand::RunTask {
            task_id: task_id.clone(),
            granted_by: "someone-else".to_owned(),
        },
    )
    .await;
    assert!(
        refused_message.contains("different payload"),
        "the refusal must name the id-reuse conflict, got {refused_message:?}"
    );

    assert_eq!(store.list_active_workflow_runs().await.unwrap().len(), 1);
    assert_eq!(start_event_count(&store).await, 1);

    daemon.shutdown().await;
}

#[tokio::test]
async fn a_second_run_approval_for_a_running_task_is_refused() {
    let base = unique_temp_dir("second-approval");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);

    let store = SqliteStore::in_memory().await.unwrap();
    // No workflow driver: this proves the durable approval boundary, not the
    // execution that follows it.
    let (server, daemon) = run_full_daemon(
        store.clone(),
        test_relay(&repo),
        "lease-waveb-second-approval",
        Duration::from_secs(60),
    )
    .await
    .unwrap();

    let (task_id, run_id) = approve_run(&server, "cmd-first").await;

    // A second Run approval for the same task, under a *different* command id,
    // so it is not a replay. Different command ids are not serialized, so both
    // may pass the `Ready` pre-check and race; the store's compare-and-set is
    // what must refuse the loser. Either way the second approval is refused
    // fail-closed: an apply-time refusal is reported as a failed command result
    // (a `DaemonResponse::Error` is reserved for a refusal before the command is
    // recorded, such as lost authority or command-id reuse).
    let (status, result) = submit(
        &server,
        "cmd-second-run",
        DaemonCommand::RunTask {
            task_id: task_id.clone(),
            granted_by: "operator-alice".to_owned(),
        },
    )
    .await;
    assert_eq!(
        status,
        CommandStatus::Failed,
        "a second Run approval for a running task must be refused, got {result:?}"
    );
    let refusal = result.unwrap_or_default();
    assert!(
        refusal.contains("running"),
        "the refusal must name the state that blocked the second approval, got {refusal:?}"
    );

    // The refused approval left nothing durable behind: one task, one run, one
    // start event.
    let active = store.list_active_workflow_runs().await.unwrap();
    assert_eq!(
        active.len(),
        1,
        "a second approval must not create a second run, got {active:?}"
    );
    assert_eq!(active[0].id, run_id);
    assert_eq!(
        start_event_count(&store).await,
        1,
        "a second approval must not append a second start event"
    );
    assert_eq!(
        stored_task_state(&store, &task_id).await,
        TaskState::Running
    );

    // Replaying the original approval still converges on the same durable run.
    let (status, replay_result) = submit(
        &server,
        "cmd-first-run",
        DaemonCommand::RunTask {
            task_id: task_id.clone(),
            granted_by: "operator-alice".to_owned(),
        },
    )
    .await;
    assert_eq!(status, CommandStatus::Duplicate);
    let replay_json: serde_json::Value =
        serde_json::from_str(replay_result.as_deref().unwrap()).unwrap();
    assert_eq!(replay_json["workflow_run_id"].as_str().unwrap(), run_id);

    daemon.shutdown().await;
}

#[tokio::test]
async fn a_daemon_that_lost_its_lease_cannot_start_or_drive_a_run() {
    let base = unique_temp_dir("stale");
    let repo = base.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);

    let store = SqliteStore::in_memory().await.unwrap();
    let (server, daemon) = run_full_daemon(
        store.clone(),
        test_relay(&repo),
        "lease-waveb-stale",
        Duration::from_secs(60),
    )
    .await
    .unwrap();

    // One run is approved while this process is still authoritative.
    let (_task1, run1) = approve_run(&server, "cmd-stale-1").await;

    // A second handoff is imported but not yet approved.
    let (status, result) = submit(
        &server,
        "cmd-stale-2-submit",
        DaemonCommand::SubmitHandoff {
            project_key: "wave-b-proj".to_owned(),
            handoff_json: TEST_HANDOFF.to_owned(),
            submission_transport: "local-paste".to_owned(),
        },
    )
    .await;
    assert_eq!(status, CommandStatus::Applied);
    let task2 =
        serde_json::from_str::<serde_json::Value>(result.as_deref().unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

    // A successor takes the lease, bumping the fencing token and changing the
    // holder identity, exactly as a real takeover would.
    let successor = store
        .acquire_or_renew_daemon_lease(
            "lease-waveb-stale",
            999_999,
            "host-b",
            "inst-successor",
            Duration::ZERO,
        )
        .await
        .unwrap();
    assert_eq!(
        successor.fencing_token, 2,
        "the successor must hold a strictly newer generation"
    );

    // The now-stale daemon cannot start the second run: nothing is written.
    let refused_message = submit_refused(
        &server,
        "cmd-stale-2-run",
        DaemonCommand::RunTask {
            task_id: task2.clone(),
            granted_by: "operator-alice".to_owned(),
        },
    )
    .await;
    assert!(
        !refused_message.is_empty(),
        "a daemon that lost its lease must report the refusal, got {refused_message:?}"
    );

    assert_eq!(
        store.list_active_workflow_runs().await.unwrap().len(),
        1,
        "the stale approval must not create a second run"
    );
    assert_eq!(
        stored_task_state(&store, &task2).await,
        TaskState::Ready,
        "the stale approval must not advance the second task"
    );
    assert_eq!(
        start_event_count(&store).await,
        1,
        "the stale approval must not append a start event"
    );

    // A driver iteration holding the stale fence is refused at its first write,
    // and reports authority loss rather than a run failure.
    let stale_fence = server.fence();
    let orchestrator = OrchestratorV2::new(
        store.clone(),
        Arc::new(SchedulerV2::new(SchedulerV2Config::default())),
        Arc::new(FakeBackplane::new()),
        ArtifactStore::new(base.join("artifacts")).unwrap(),
        test_provider_adapters(),
    );
    let project_id = ProjectId::parse("wave-b-proj").unwrap();
    let checkout_id = CheckoutId::generate();
    let target = fake_target();
    let budget = budget();
    let error = orchestrator
        .run_compatibility_workflow(
            &WorkflowRunId(run1.clone()),
            &project_id,
            &checkout_id,
            &repo,
            &target,
            &budget,
            &stale_fence,
        )
        .await
        .expect_err("a stale driver must not be able to drive a run");
    assert!(
        error.is_authority_lost(),
        "the stale driver must report authority loss, not a run failure, got {error:?}"
    );
    assert!(
        list_role_instances_for_run(&store, &WorkflowRunId(run1))
            .await
            .unwrap()
            .is_empty(),
        "a stale driver must not create role instances"
    );

    daemon.shutdown().await;
}
