//! Trusted execution context resolution for approved daemon workflow runs.

use std::{
    path::{Component, Path, PathBuf},
    process::Command,
};

use relay_store::WorkflowRunRecord;

use crate::{
    Checkout, CheckoutId, CheckoutTrustState, ExecutionBudget, ExecutionTarget,
    PortableRepositoryConfig, Relay, SqliteStore, TrustClassification, WorkflowDriverContext,
    WorkflowRun, WorkflowRunContextFuture, WorkflowRunContextResolver, WorktreeHandle,
    WorktreeManager, durable_state::get_stored_task,
};

/// Resolves durable run authority against the current trusted project registry.
#[derive(Clone)]
pub struct TrustedRunContextResolver {
    relay_config_path: Option<PathBuf>,
    store: SqliteStore,
}

impl TrustedRunContextResolver {
    pub fn new(relay_config_path: Option<PathBuf>, store: SqliteStore) -> Self {
        Self {
            relay_config_path,
            store,
        }
    }
}

impl WorkflowRunContextResolver for TrustedRunContextResolver {
    fn resolve<'a>(&'a self, run: &'a WorkflowRun) -> WorkflowRunContextFuture<'a> {
        Box::pin(self.resolve_inner(run))
    }
}

impl TrustedRunContextResolver {
    async fn resolve_inner(&self, run: &WorkflowRun) -> Result<WorkflowDriverContext, String> {
        let task = get_stored_task(&self.store, run.task_id)
            .await
            .map_err(|error| format!("could not load task '{}': {error}", run.task_id))?
            .ok_or_else(|| format!("task '{}' was not found", run.task_id))?;

        let config_path = self
            .relay_config_path
            .as_deref()
            .ok_or_else(|| "no trusted project registry is configured".to_owned())?;
        let trusted_relay = Relay::load_from_config_path(config_path)
            .map_err(|error| format!("could not reload the trusted project registry: {error}"))?;
        let project = trusted_relay
            .project_registry()
            .get(task.project_key.as_str())
            .map_err(|_| format!("project '{}' is not registered", task.project_key))?;
        let registration = project.to_trusted_registration();
        let checkout_id = CheckoutId::for_project_key(&task.project_key);
        let registered_checkout = registration.get_checkout(&checkout_id).ok_or_else(|| {
            format!(
                "project '{}' has no registered checkout '{}'",
                task.project_key, checkout_id
            )
        })?;

        let canonical_root =
            registered_checkout
                .canonical_root
                .canonicalize()
                .map_err(|error| {
                    format!(
                        "registered project '{}' checkout could not be opened: {error}",
                        task.project_key
                    )
                })?;
        let checkout = Checkout::new(
            checkout_id.clone(),
            registration.project_id.clone(),
            canonical_root.clone(),
            registered_checkout.expected_remote.clone(),
            Some(registered_checkout.allowed_worktree_root.clone()),
            CheckoutTrustState::Trusted,
        )
        .map_err(|error| format!("registered checkout is invalid: {error}"))?;

        let portable = PortableRepositoryConfig::load_from_dir(&checkout.canonical_root)
            .map_err(|error| format!("project trust configuration could not be loaded: {error}"))?
            .unwrap_or_else(|| PortableRepositoryConfig {
                relay_dir: checkout
                    .canonical_root
                    .join(crate::portable_config::RELAY_DIR),
                declared_identity: None,
                restrictions: Default::default(),
            });
        let trust = portable
            .validate_and_classify(&registration, &checkout)
            .map_err(|error| error.to_string())?;
        match &trust {
            TrustClassification::TrustedAgainstRegistration { .. } => {}
            TrustClassification::UntrustedDeclaration { reason } => return Err(reason.clone()),
        }

        let record = self
            .store
            .get_workflow_run(&run.id.0)
            .await
            .map_err(|error| format!("could not load workflow run '{}': {error}", run.id))?
            .ok_or_else(|| format!("workflow run '{}' was not found", run.id))?;
        if record.task_id != run.task_id.to_string() {
            return Err(format!(
                "workflow run '{}' is bound to a different task than its durable record",
                run.id
            ));
        }
        let (execution_target, budget) = frozen_run_authority(&record)?;

        let base_commit = git_output(&checkout.canonical_root, &["rev-parse", "HEAD"])
            .map_err(|error| format!("could not resolve the trusted checkout HEAD: {error}"))?;
        let worktree_name = format!("relay-run-{}", run.id.0);
        if !valid_worktree_name(&worktree_name) {
            return Err(format!(
                "workflow run '{}' has an invalid worktree identity",
                run.id
            ));
        }

        std::fs::create_dir_all(&checkout.allowed_worktree_root)
            .map_err(|error| format!("could not prepare the trusted worktree root: {error}"))?;
        let canonical_worktree_root = checkout
            .allowed_worktree_root
            .canonicalize()
            .map_err(|error| format!("could not resolve the trusted worktree root: {error}"))?;
        if !canonical_worktree_root.starts_with(&checkout.canonical_root) {
            return Err(
                "trusted worktree root resolves outside the registered checkout".to_owned(),
            );
        }

        let working_tree = match existing_registered_worktree(
            &checkout,
            &canonical_worktree_root,
            &worktree_name,
            &run.started_at,
        )? {
            Some(worktree) => worktree,
            None => WorktreeManager::create_isolated_worktree(
                &checkout,
                &trust,
                &worktree_name,
                &base_commit,
            )
            .map_err(|error| format!("could not create the isolated run worktree: {error}"))?,
        };

        Ok(WorkflowDriverContext {
            project_id: registration.project_id,
            checkout_id,
            working_directory: working_tree.path,
            execution_target,
            budget,
        })
    }
}

fn frozen_run_authority(
    record: &WorkflowRunRecord,
) -> Result<(ExecutionTarget, ExecutionBudget), String> {
    let snapshot = if record.config_snapshot_json.trim().is_empty() {
        None
    } else {
        Some(
            serde_json::from_str::<serde_json::Value>(&record.config_snapshot_json).map_err(
                |error| {
                    format!(
                        "workflow run '{}' has invalid frozen config snapshot JSON: {error}",
                        record.id
                    )
                },
            )?,
        )
    };

    let target_value = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get("execution_target"))
        .filter(|target| !target.is_null())
        .ok_or_else(|| {
            format!(
                "workflow run '{}' has no frozen execution target",
                record.id
            )
        })?;
    let execution_target: ExecutionTarget =
        serde_json::from_value(target_value.clone()).map_err(|error| {
            format!(
                "workflow run '{}' has an invalid frozen execution target: {error}",
                record.id
            )
        })?;
    if execution_target.provider_id.trim().is_empty()
        || execution_target.runtime_profile_id.trim().is_empty()
        || execution_target.provider_adapter_version.trim().is_empty()
        || execution_target
            .runtime_profile_id
            .split('/')
            .next()
            .is_none_or(|provider| provider != execution_target.provider_id.as_str())
    {
        return Err(format!(
            "workflow run '{}' has a misconfigured frozen execution target",
            record.id
        ));
    }

    // Older runs may have no snapshot or a snapshot without a budget. Only that
    // genuinely absent value uses the compatibility default; malformed budgets
    // fail closed rather than silently changing the approved limit.
    let budget = match snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get("budget"))
        .filter(|budget| !budget.is_null())
    {
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
            format!(
                "workflow run '{}' has an invalid frozen execution budget: {error}",
                record.id
            )
        })?,
        None => ExecutionBudget::default(),
    };

    Ok((execution_target, budget))
}

fn existing_registered_worktree(
    checkout: &Checkout,
    canonical_worktree_root: &Path,
    worktree_name: &str,
    created_at: &str,
) -> Result<Option<WorktreeHandle>, String> {
    let worktree_path = checkout.allowed_worktree_root.join(worktree_name);
    match std::fs::symlink_metadata(&worktree_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect the run worktree path: {error}")),
        Ok(_) => {}
    }

    let canonical_worktree = worktree_path
        .canonicalize()
        .map_err(|error| format!("existing run worktree cannot be resolved: {error}"))?;
    if !canonical_worktree.starts_with(canonical_worktree_root) {
        return Err("existing run worktree resolves outside the trusted worktree root".to_owned());
    }

    let listing = git_output(
        &checkout.canonical_root,
        &["worktree", "list", "--porcelain"],
    )
    .map_err(|error| format!("could not verify the existing Git worktree: {error}"))?;
    let registered = listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|listed| {
            PathBuf::from(listed)
                .canonicalize()
                .is_ok_and(|path| path == canonical_worktree)
        });
    if !registered {
        return Err(
            "existing run worktree path is not registered to this checkout; refusing reuse"
                .to_owned(),
        );
    }
    let base_commit = git_output(&canonical_worktree, &["rev-parse", "HEAD"])
        .map_err(|error| format!("could not inspect the existing run worktree HEAD: {error}"))?;

    Ok(Some(WorktreeHandle {
        id: worktree_name.to_owned(),
        path: canonical_worktree,
        base_commit,
        created_at: created_at.to_owned(),
    }))
}

fn valid_worktree_name(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name).components().all(|component| {
            matches!(component, Component::Normal(value) if value.to_string_lossy().chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        })
}

fn git_output(repository: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.current_dir(repository).args(args);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(name);
    }

    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "git {} exited with {:?}: {}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|error| format!("git {} returned non-UTF-8 output: {error}", args.join(" ")))?;
    Ok(output.trim().to_owned())
}
