//! Relay Architecture v2 - Phase 05: v2 Orchestration Engine (R12-02).
//!
//! Drives durable `WorkflowRun`, `RoleInstance`, and `RoleAttempt` execution
//! through `SchedulerV2` for slot allocation / write leases and `RuntimeBackplane`
//! for execution isolation, process supervision, and event observation.
//!
//! Authority: every durable write this engine performs is fenced on the daemon
//! lease (`relay_store::DaemonFence`). The driver that hands work to this engine
//! passes the fence of the daemon instance that currently owns the database, so
//! a run being driven by a daemon that has lost authority is refused at its next
//! write instead of racing the successor. Reads are unfenced: a stale daemon
//! reading durable state cannot change it.
//!
//! Core Invariants:
//! 1. Process exit success alone NEVER completes a role instance; completion
//!    requires an accepted, typed result artifact (`ArtifactKind::ImplementationResult`
//!    or `ArtifactKind::VerificationResult`).
//! 2. Role attempts are monotonically numbered and retries do not create a new
//!    semantic `RoleInstance`.
//! 3. Write actions acquire monotonically fenced durable leases via `SchedulerV2`.
//! 4. Unbounded remediation is strictly halted by execution budget limits.
//! 5. Execution state transitions are persisted durably in SQLite for restart rehydration.

use std::{collections::HashSet, path::Path, sync::Arc};

use relay_runtime::{
    EffectKey, LaunchContext, LaunchSpecification, ProcessStatus, ProviderAdapter, ProviderError,
    ReconcileOutcome, RuntimeBackplane, RuntimeHandle, RuntimeModelSelection,
    RuntimeProvisionRequest, RuntimeReasoningSelection, SanitizedEnvironment,
};
use relay_store::{DaemonFence, StoreError};
use thiserror::Error;

use crate::{
    ApprovedIntent, ArtifactKind, ArtifactStore, Candidate, CandidateFingerprint, CheckoutId,
    CreateArtifactRequest, ExecutionBudget, ExecutionTarget, ProjectId, RoleAttempt,
    RoleAttemptState, RoleDefinitionId, RoleInstance, SchedulePriority, SchedulerV2, SqliteStore,
    WorkflowRun, WorkflowRunId, WorkflowRunState,
    durable_state::{
        TaskPersistenceError, get_durable_workflow_run, list_active_workflow_runs,
        list_role_attempts_for_instance, list_role_instances_for_run,
        persist_role_attempt_if_absent, persist_role_instance_if_absent, record_role_attempt_lease,
        record_role_attempt_runtime_handle, update_durable_role_attempt,
        update_durable_role_instance, update_durable_workflow_run_state,
    },
    scheduler_v2::QueuedWorkItem,
};

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("durable state error: {0}")]
    DurableState(#[from] TaskPersistenceError),
    #[error("workflow run '{0}' not found")]
    RunNotFound(String),
    #[error("scheduling failed: {0}")]
    SchedulingFailed(String),
    #[error("no provider adapter is registered for frozen provider '{0}'")]
    ProviderAdapterMissing(String),
    #[error("provider launch could not be built: {0}")]
    ProviderLaunchFailed(String),
    #[error("backplane error: {0}")]
    Backplane(String),
    #[error("workflow error: {0}")]
    Workflow(#[from] crate::WorkflowError),
    #[error("artifact store error: {0}")]
    Artifact(#[from] crate::ArtifactError),
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("role invariant violation: {0}")]
    InvariantViolation(String),
    #[error("remediation loop failed: {0}")]
    RemediationFailed(String),
    #[error("role attempt published no result: {0}")]
    ResultMissing(String),
    #[error("role attempt published an unusable result: {0}")]
    ResultMalformed(String),
}

impl OrchestratorError {
    /// Whether this error means the daemon driving the run no longer owns the
    /// database. Such an error is not a run failure: the successor owns the run
    /// now, so the driver must stop instead of recording a false failure.
    pub fn is_authority_lost(&self) -> bool {
        matches!(
            self,
            Self::Store(StoreError::DaemonAuthorityLost { .. })
                | Self::DurableState(TaskPersistenceError::Store(
                    StoreError::DaemonAuthorityLost { .. }
                ))
        )
    }
}

// The typed role-result contract lives in the runtime layer (see
// `relay_runtime::role_result`), so provider adapters and this orchestrator
// share exactly one parser. It is re-exported here to preserve the historical
// `relay_core::orchestrator_v2::*` public paths.
pub use relay_runtime::{
    PublishedRoleResult, ResultDefect, RoleResultPayload, observed_role_result,
};

/// Inputs shared by every role phase of one workflow run.
#[derive(Clone, Copy)]
pub struct PhaseContext<'a> {
    pub project_id: &'a ProjectId,
    pub checkout_id: &'a CheckoutId,
    pub working_directory: &'a Path,
    pub execution_target: &'a ExecutionTarget,
    pub budget: &'a ExecutionBudget,
}

/// The v2 Orchestrator engine driving workflow runs across scheduler and backplane seams.
pub struct OrchestratorV2<B: RuntimeBackplane> {
    store: SqliteStore,
    scheduler: Arc<SchedulerV2>,
    backplane: Arc<B>,
    artifact_store: ArtifactStore,
    provider_adapters: Vec<Arc<dyn ProviderAdapter>>,
}

impl<B: RuntimeBackplane> OrchestratorV2<B> {
    pub fn new(
        store: SqliteStore,
        scheduler: Arc<SchedulerV2>,
        backplane: Arc<B>,
        artifact_store: ArtifactStore,
        provider_adapters: Vec<Arc<dyn ProviderAdapter>>,
    ) -> Self {
        Self {
            store,
            scheduler,
            backplane,
            artifact_store,
            provider_adapters,
        }
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn scheduler(&self) -> &SchedulerV2 {
        &self.scheduler
    }

    pub fn backplane(&self) -> &B {
        &self.backplane
    }

    pub fn artifact_store(&self) -> &ArtifactStore {
        &self.artifact_store
    }

    /// Executes a full Phase 05 compatibility workflow run:
    /// Implementer -> Candidate 1 -> Verifier.
    ///
    /// If verification fails, bounded remediation triggers:
    /// Implementer (Remediation) -> Candidate 2 -> Verifier (PASS) -> Complete.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_compatibility_workflow(
        &self,
        workflow_run_id: &WorkflowRunId,
        project_id: &ProjectId,
        checkout_id: &CheckoutId,
        working_directory: &Path,
        execution_target: &ExecutionTarget,
        budget: &ExecutionBudget,
        fence: &DaemonFence,
    ) -> Result<WorkflowRunState, OrchestratorError> {
        let mut workflow_run = get_durable_workflow_run(&self.store, &workflow_run_id.0)
            .await?
            .ok_or_else(|| OrchestratorError::RunNotFound(workflow_run_id.0.clone()))?;

        if workflow_run.state == WorkflowRunState::Completed {
            return Ok(WorkflowRunState::Completed);
        }

        let ctx = PhaseContext {
            project_id,
            checkout_id,
            working_directory,
            execution_target,
            budget,
        };
        let mut current_generation = 1u32;
        let mut role_activations = 0u32;
        let mut remediation_reason = None;

        loop {
            role_activations += 1;
            if role_activations > budget.max_role_activations {
                workflow_run.fail();
                update_durable_workflow_run_state(&self.store, &workflow_run, fence).await?;
                return Err(OrchestratorError::BudgetExhausted(format!(
                    "max role activations ({}) exceeded",
                    budget.max_role_activations
                )));
            }

            // 1. Run Implementer
            let (candidate, _impl_artifact_id) = self
                .run_implementer_phase_with_reason(
                    &workflow_run,
                    &ctx,
                    current_generation,
                    remediation_reason.as_deref(),
                    fence,
                )
                .await?;

            role_activations += 1;
            if role_activations > budget.max_role_activations {
                workflow_run.fail();
                update_durable_workflow_run_state(&self.store, &workflow_run, fence).await?;
                return Err(OrchestratorError::BudgetExhausted(format!(
                    "max role activations ({}) exceeded",
                    budget.max_role_activations
                )));
            }

            // 2. Run Verifier
            let verifier_failure_reason = self
                .run_verifier_phase_with_failure_reason(
                    &workflow_run,
                    &ctx,
                    current_generation,
                    &candidate,
                    fence,
                )
                .await?;

            if verifier_failure_reason.is_none() {
                workflow_run.complete();
                update_durable_workflow_run_state(&self.store, &workflow_run, fence).await?;
                return Ok(WorkflowRunState::Completed);
            }

            // Verifier failed: trigger bounded remediation loop
            current_generation += 1;
            remediation_reason = verifier_failure_reason;
        }
    }

    /// Runs the Implementer role instance and its attempt through scheduler and backplane.
    pub async fn run_implementer_phase(
        &self,
        workflow_run: &WorkflowRun,
        ctx: &PhaseContext<'_>,
        generation: u32,
        fence: &DaemonFence,
    ) -> Result<(Candidate, String), OrchestratorError> {
        self.run_implementer_phase_with_reason(workflow_run, ctx, generation, None, fence)
            .await
    }

    async fn run_implementer_phase_with_reason(
        &self,
        workflow_run: &WorkflowRun,
        ctx: &PhaseContext<'_>,
        generation: u32,
        remediation_reason: Option<&str>,
        fence: &DaemonFence,
    ) -> Result<(Candidate, String), OrchestratorError> {
        let PhaseContext {
            project_id,
            checkout_id,
            working_directory,
            execution_target,
            budget,
        } = *ctx;
        let approved_intent = self.load_approved_intent(workflow_run).await?;
        let implementer_role_id = RoleDefinitionId::new("implementer");
        let mut implementer_instance = RoleInstance::new(
            workflow_run.id.clone(),
            implementer_role_id,
            generation,
            None,
        );
        persist_role_instance_if_absent(&self.store, &implementer_instance, fence).await?;

        let mut attempt = implementer_instance
            .spawn_attempt(execution_target.clone(), budget)
            .map_err(OrchestratorError::Workflow)?;
        persist_role_attempt_if_absent(&self.store, &attempt, fence).await?;
        update_durable_role_instance(&self.store, &implementer_instance, fence).await?;

        let effect_key = EffectKey::new(format!("effect-{}", attempt.id));
        let launch = match self.build_role_launch(
            execution_target,
            &attempt,
            working_directory,
            "implementer",
            &approved_intent,
            remediation_reason,
            effect_key.clone(),
        ) {
            Ok(launch) => launch,
            Err(error) => {
                self.fail_attempt_before_launch(
                    &mut attempt,
                    &mut implementer_instance,
                    error.to_string(),
                    fence,
                )
                .await?;
                return Err(error);
            }
        };

        // Enqueue to SchedulerV2 with write lease requirement
        let item = QueuedWorkItem {
            task_id: workflow_run.task_id,
            project_id: project_id.clone(),
            checkout_id: checkout_id.clone(),
            workflow_run_id: workflow_run.id.clone(),
            role_instance_id: implementer_instance.id.clone(),
            role_attempt_id: attempt.id.clone(),
            target_provider: execution_target.provider_id.clone(),
            priority: SchedulePriority::Normal,
            required_capabilities: HashSet::new(),
            requires_exclusive_checkout: true,
            queued_at: 0,
            age_weight: 0,
        };
        self.scheduler.enqueue(item);

        let slots = self.scheduler.schedule_next(&self.store).await;
        let slot = match slots
            .into_iter()
            .find(|s| s.work_item.role_attempt_id == attempt.id)
        {
            Some(slot) => slot,
            None => {
                // The work item is still queued. This attempt is about to be
                // abandoned, so it must not be left for a later phase to admit
                // as a slot nobody will release.
                self.scheduler.withdraw_queued(&attempt.id);
                return Err(OrchestratorError::SchedulingFailed(
                    "exclusive checkout write lease could not be scheduled".to_owned(),
                ));
            }
        };

        attempt.lease_id = slot.lease_id.clone().map(|l| l.0);
        attempt.state = RoleAttemptState::Running;
        update_durable_role_attempt(&self.store, &attempt, fence).await?;
        record_role_attempt_lease(&self.store, &attempt, fence).await?;

        // Provision on RuntimeBackplane
        let prov_req = RuntimeProvisionRequest::new(
            effect_key,
            attempt.id.to_string(),
            working_directory,
            launch,
            SanitizedEnvironment::new(),
            1024 * 1024,
        );

        let handle = self
            .backplane
            .provision(prov_req)
            .await
            .map_err(|e| OrchestratorError::Backplane(e.to_string()))?;

        // Record the process identity durably so a restarted daemon can
        // reconcile the process this attempt actually launched.
        attempt.runtime_handle_id = Some(handle.handle_id.clone());
        attempt.runtime_backplane_id = Some(handle.backplane_id.to_string());
        record_role_attempt_runtime_handle(&self.store, &attempt, fence).await?;

        // Observe process execution
        let observation = self
            .backplane
            .observe(&handle)
            .await
            .map_err(|e| OrchestratorError::Backplane(e.to_string()))?;

        // INVARIANT: Process success alone NEVER completes a role instance!
        if !matches!(observation.status, ProcessStatus::ExitedSuccess { .. }) {
            attempt.fail("process exited with failure".to_owned());
            update_durable_role_attempt(&self.store, &attempt, fence).await?;
            implementer_instance.fail();
            update_durable_role_instance(&self.store, &implementer_instance, fence).await?;
            let _ = self.backplane.dispose(&handle).await;
            self.scheduler
                .release_slot(&slot.slot_id, &self.store)
                .await;
            return Err(OrchestratorError::Backplane(
                "implementer process failed".to_owned(),
            ));
        }

        // Process exited 0; the role is still NOT completed. Completion requires
        // a typed result the process itself published, never exit status alone.
        attempt.state = RoleAttemptState::ResultPending;
        attempt.runtime_outcome = Some("EXITED_SUCCESS".to_owned());
        update_durable_role_attempt(&self.store, &attempt, fence).await?;

        // Clean up handle and slot
        let _ = self.backplane.dispose(&handle).await;
        self.scheduler
            .release_slot(&slot.slot_id, &self.store)
            .await;

        // Accept only a typed result the implementer published on stdout.
        let published = match observed_role_result(&observation) {
            Ok(published) => published,
            Err(defect) => {
                return Err(self
                    .reject_unusable_result(
                        &mut attempt,
                        &mut implementer_instance,
                        "implementer",
                        defect,
                        fence,
                    )
                    .await);
            }
        };
        if !matches!(published.payload, RoleResultPayload::Implemented { .. }) {
            let defect = ResultDefect::Malformed(format!(
                "implementer attempt published a '{}' result",
                published.payload.kind()
            ));
            return Err(self
                .reject_unusable_result(
                    &mut attempt,
                    &mut implementer_instance,
                    "implementer",
                    defect,
                    fence,
                )
                .await);
        }

        // Capture candidate fingerprint from repository
        let fingerprint = CandidateFingerprint::capture(working_directory, None)
            .map_err(|e| OrchestratorError::InvariantViolation(e.to_string()))?;

        let candidate = Candidate::create(
            project_id.clone(),
            checkout_id.clone(),
            workflow_run.id.clone(),
            generation,
            implementer_instance.id.clone(),
            attempt.id.clone(),
            fingerprint,
        );

        // Store ImplementationResult artifact
        let impl_artifact = self.artifact_store.put(CreateArtifactRequest {
            kind: ArtifactKind::ImplementationResult,
            project_id: project_id.clone(),
            task_id: workflow_run.task_id,
            workflow_run_id: workflow_run.id.clone(),
            producer_role_instance_id: Some(implementer_instance.id.clone()),
            producer_attempt_id: Some(attempt.id.clone()),
            candidate_id: Some(candidate.id.clone()),
            trust_class: crate::ArtifactTrustClass::RelayObserved,
            sensitivity: crate::ArtifactSensitivity::Internal,
            retention: crate::RetentionClass::WorkflowScoped,
            payload: published.raw.as_bytes(),
        })?;

        // Now accept typed result artifact -> role instance Completed
        implementer_instance.accept_result(impl_artifact.id.0.clone());
        update_durable_role_instance(&self.store, &implementer_instance, fence).await?;

        attempt.finish(
            "EXITED_SUCCESS".to_owned(),
            Some(impl_artifact.id.0.clone()),
        );
        update_durable_role_attempt(&self.store, &attempt, fence).await?;

        Ok((candidate, impl_artifact.id.0))
    }

    /// Runs the Verifier role instance and its attempt through scheduler and backplane.
    pub async fn run_verifier_phase(
        &self,
        workflow_run: &WorkflowRun,
        ctx: &PhaseContext<'_>,
        generation: u32,
        candidate: &Candidate,
        fence: &DaemonFence,
    ) -> Result<bool, OrchestratorError> {
        Ok(self
            .run_verifier_phase_with_failure_reason(workflow_run, ctx, generation, candidate, fence)
            .await?
            .is_none())
    }

    async fn run_verifier_phase_with_failure_reason(
        &self,
        workflow_run: &WorkflowRun,
        ctx: &PhaseContext<'_>,
        generation: u32,
        candidate: &Candidate,
        fence: &DaemonFence,
    ) -> Result<Option<String>, OrchestratorError> {
        let PhaseContext {
            project_id,
            checkout_id,
            working_directory,
            execution_target,
            budget,
        } = *ctx;
        let approved_intent = self.load_approved_intent(workflow_run).await?;
        let verifier_role_id = RoleDefinitionId::new("verifier");
        let mut verifier_instance =
            RoleInstance::new(workflow_run.id.clone(), verifier_role_id, generation, None);
        persist_role_instance_if_absent(&self.store, &verifier_instance, fence).await?;

        let mut attempt = verifier_instance
            .spawn_attempt(execution_target.clone(), budget)
            .map_err(OrchestratorError::Workflow)?;
        persist_role_attempt_if_absent(&self.store, &attempt, fence).await?;
        update_durable_role_instance(&self.store, &verifier_instance, fence).await?;

        let effect_key = EffectKey::new(format!("effect-{}", attempt.id));
        let launch = match self.build_role_launch(
            execution_target,
            &attempt,
            working_directory,
            "verifier",
            &approved_intent,
            None,
            effect_key.clone(),
        ) {
            Ok(launch) => launch,
            Err(error) => {
                self.fail_attempt_before_launch(
                    &mut attempt,
                    &mut verifier_instance,
                    error.to_string(),
                    fence,
                )
                .await?;
                return Err(error);
            }
        };

        // Verifier is ReadOnly (no exclusive checkout write lease required)
        let item = QueuedWorkItem {
            task_id: workflow_run.task_id,
            project_id: project_id.clone(),
            checkout_id: checkout_id.clone(),
            workflow_run_id: workflow_run.id.clone(),
            role_instance_id: verifier_instance.id.clone(),
            role_attempt_id: attempt.id.clone(),
            target_provider: execution_target.provider_id.clone(),
            priority: SchedulePriority::Normal,
            required_capabilities: HashSet::new(),
            requires_exclusive_checkout: false,
            queued_at: 0,
            age_weight: 0,
        };
        self.scheduler.enqueue(item);

        let slots = self.scheduler.schedule_next(&self.store).await;
        let slot = match slots
            .into_iter()
            .find(|s| s.work_item.role_attempt_id == attempt.id)
        {
            Some(slot) => slot,
            None => {
                // Same as the implementer: never leave an abandoned attempt's
                // work item queued for someone else to admit.
                self.scheduler.withdraw_queued(&attempt.id);
                return Err(OrchestratorError::SchedulingFailed(
                    "verifier slot could not be scheduled".to_owned(),
                ));
            }
        };

        attempt.state = RoleAttemptState::Running;
        update_durable_role_attempt(&self.store, &attempt, fence).await?;

        // Backplane provision & observe
        let prov_req = RuntimeProvisionRequest::new(
            effect_key,
            attempt.id.to_string(),
            working_directory,
            launch,
            SanitizedEnvironment::new(),
            1024 * 1024,
        );

        let handle = self
            .backplane
            .provision(prov_req)
            .await
            .map_err(|e| OrchestratorError::Backplane(e.to_string()))?;

        // Record the process identity durably so a restarted daemon can
        // reconcile the process this attempt actually launched.
        attempt.runtime_handle_id = Some(handle.handle_id.clone());
        attempt.runtime_backplane_id = Some(handle.backplane_id.to_string());
        record_role_attempt_runtime_handle(&self.store, &attempt, fence).await?;

        let observation = self
            .backplane
            .observe(&handle)
            .await
            .map_err(|e| OrchestratorError::Backplane(e.to_string()))?;

        let _ = self.backplane.dispose(&handle).await;
        self.scheduler
            .release_slot(&slot.slot_id, &self.store)
            .await;

        // A crashed verifier process is an execution failure, not a verification
        // verdict. It must not be mistaken for "the implementation failed" and
        // must not silently launch a remediation generation.
        if !matches!(observation.status, ProcessStatus::ExitedSuccess { .. }) {
            attempt.fail("verifier process exited with failure".to_owned());
            update_durable_role_attempt(&self.store, &attempt, fence).await?;
            verifier_instance.fail();
            update_durable_role_instance(&self.store, &verifier_instance, fence).await?;
            return Err(OrchestratorError::Backplane(
                "verifier process failed".to_owned(),
            ));
        }

        // Accept only a typed result the verifier published on stdout.
        let published = match observed_role_result(&observation) {
            Ok(published) => published,
            Err(defect) => {
                return Err(self
                    .reject_unusable_result(
                        &mut attempt,
                        &mut verifier_instance,
                        "verifier",
                        defect,
                        fence,
                    )
                    .await);
            }
        };
        let failure_reason = match &published.payload {
            RoleResultPayload::VerificationPassed => None,
            RoleResultPayload::VerificationFailed { reason } => Some(reason.clone()),
            other => {
                let defect = ResultDefect::Malformed(format!(
                    "verifier attempt published a '{}' result",
                    other.kind()
                ));
                return Err(self
                    .reject_unusable_result(
                        &mut attempt,
                        &mut verifier_instance,
                        "verifier",
                        defect,
                        fence,
                    )
                    .await);
            }
        };

        // Verify fingerprint matches repository state
        let matches_fingerprint = candidate
            .fingerprint
            .verify_against_repo(working_directory)
            .map_err(|e| OrchestratorError::InvariantViolation(e.to_string()))?;

        // A tampered checkout is an integrity failure, not a verification verdict.
        if !matches_fingerprint {
            attempt.fail("candidate fingerprint mismatch during verification".to_owned());
            update_durable_role_attempt(&self.store, &attempt, fence).await?;
            verifier_instance.fail();
            update_durable_role_instance(&self.store, &verifier_instance, fence).await?;
            return Err(OrchestratorError::InvariantViolation(
                "candidate fingerprint mismatch during verification".to_owned(),
            ));
        }

        // Record verification artifact
        let verif_artifact = self.artifact_store.put(CreateArtifactRequest {
            kind: ArtifactKind::VerificationResult,
            project_id: project_id.clone(),
            task_id: workflow_run.task_id,
            workflow_run_id: workflow_run.id.clone(),
            producer_role_instance_id: Some(verifier_instance.id.clone()),
            producer_attempt_id: Some(attempt.id.clone()),
            candidate_id: Some(candidate.id.clone()),
            trust_class: crate::ArtifactTrustClass::RelayObserved,
            sensitivity: crate::ArtifactSensitivity::Internal,
            retention: crate::RetentionClass::WorkflowScoped,
            payload: published.raw.as_bytes(),
        })?;

        // A verifier that legitimately reports FAIL has still completed its own
        // role: the verification result is its accepted typed result. What
        // failed is the implementation, so the run enters remediation.
        verifier_instance.accept_result(verif_artifact.id.0.clone());
        update_durable_role_instance(&self.store, &verifier_instance, fence).await?;

        let outcome = if failure_reason.is_none() {
            "PASS"
        } else {
            "FAIL"
        };
        attempt.finish(outcome.to_owned(), Some(verif_artifact.id.0));
        update_durable_role_attempt(&self.store, &attempt, fence).await?;

        Ok(failure_reason)
    }

    async fn load_approved_intent(
        &self,
        workflow_run: &WorkflowRun,
    ) -> Result<ApprovedIntent, OrchestratorError> {
        let record = self
            .store
            .get_workflow_run(&workflow_run.id.0)
            .await?
            .ok_or_else(|| OrchestratorError::RunNotFound(workflow_run.id.0.clone()))?;
        if record.approved_intent_json.trim().is_empty() {
            return Err(OrchestratorError::InvariantViolation(format!(
                "workflow run '{}' has no frozen approved intent",
                workflow_run.id
            )));
        }
        serde_json::from_str(&record.approved_intent_json).map_err(|error| {
            OrchestratorError::InvariantViolation(format!(
                "workflow run '{}' has invalid frozen approved intent JSON: {error}",
                workflow_run.id
            ))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_role_launch(
        &self,
        execution_target: &ExecutionTarget,
        attempt: &RoleAttempt,
        working_directory: &Path,
        role: &str,
        approved_intent: &ApprovedIntent,
        remediation_reason: Option<&str>,
        effect_key: EffectKey,
    ) -> Result<LaunchSpecification, OrchestratorError> {
        let adapter = self
            .provider_adapters
            .iter()
            .find(|adapter| adapter.provider_id().0 == execution_target.provider_id)
            .ok_or_else(|| {
                OrchestratorError::ProviderAdapterMissing(execution_target.provider_id.clone())
            })?;
        let runtime_target = relay_runtime::ExecutionTarget {
            provider_id: adapter.provider_id(),
            runtime_profile_id: execution_target.runtime_profile_id.clone(),
            model: RuntimeModelSelection::ProviderDefault,
            reasoning_effort: RuntimeReasoningSelection::ProfileDefault,
            capability_snapshot: adapter.capabilities(),
            provider_adapter_version: execution_target.provider_adapter_version.clone(),
        };
        let mut runtime_attempt = relay_runtime::RoleAttempt::new(
            attempt.role_instance_id.0.clone(),
            attempt.attempt_no,
            relay_runtime::ProviderCapabilityRequirements::new(role),
        );
        runtime_attempt.id = relay_runtime::RoleAttemptId(attempt.id.0.clone());
        runtime_attempt.provider_pin = Some(runtime_target.provider_id.clone());
        runtime_attempt.runtime_profile_id = runtime_target.runtime_profile_id.clone();
        let context = LaunchContext::new(
            working_directory,
            role,
            effect_key,
            crate::role_prompt(role, approved_intent, remediation_reason),
        );

        adapter
            .build_launch(&runtime_target, &runtime_attempt, &context)
            .map_err(|error: ProviderError| {
                OrchestratorError::ProviderLaunchFailed(error.to_string())
            })
    }

    async fn fail_attempt_before_launch(
        &self,
        attempt: &mut RoleAttempt,
        role_instance: &mut RoleInstance,
        reason: String,
        fence: &DaemonFence,
    ) -> Result<(), OrchestratorError> {
        attempt.fail(reason);
        update_durable_role_attempt(&self.store, attempt, fence).await?;
        role_instance.fail();
        update_durable_role_instance(&self.store, role_instance, fence).await?;
        Ok(())
    }

    /// Refuses an attempt whose published result cannot be accepted.
    ///
    /// A *missing* result leaves the attempt in `ResultPending`: the process
    /// said nothing usable, which is a recoverable observation a restarted
    /// daemon can still reconcile. A *malformed* result fails the attempt and
    /// its role instance outright, because the process answered and answered
    /// wrongly. In both cases the role never completes and the workflow run
    /// never advances.
    async fn reject_unusable_result(
        &self,
        attempt: &mut RoleAttempt,
        role_instance: &mut RoleInstance,
        role: &str,
        defect: ResultDefect,
        fence: &DaemonFence,
    ) -> OrchestratorError {
        match defect {
            ResultDefect::Missing => OrchestratorError::ResultMissing(format!(
                "{role} attempt {} exited successfully but published no result object",
                attempt.id
            )),
            ResultDefect::Malformed(reason) => {
                attempt.fail(format!("unusable result: {reason}"));
                if let Err(error) = update_durable_role_attempt(&self.store, attempt, fence).await {
                    return OrchestratorError::from(error);
                }
                role_instance.fail();
                if let Err(error) =
                    update_durable_role_instance(&self.store, role_instance, fence).await
                {
                    return OrchestratorError::from(error);
                }
                OrchestratorError::ResultMalformed(format!(
                    "{role} attempt {} published an unusable result: {reason}",
                    attempt.id
                ))
            }
        }
    }

    /// Reconciles all in-progress workflow runs upon daemon restart.
    ///
    /// Rehydrates active role instances and attempts from SQLite, checks
    /// backplane state for interrupted/running attempts, and marks stale attempts
    /// lost/interrupted so execution can safely resume.
    pub async fn reconcile_on_startup(&self, fence: &DaemonFence) -> Result<(), OrchestratorError> {
        let active_runs = list_active_workflow_runs(&self.store).await?;

        for run in active_runs {
            let role_instances = list_role_instances_for_run(&self.store, &run.id).await?;

            for instance in role_instances {
                let attempts = list_role_attempts_for_instance(&self.store, &instance.id).await?;

                for mut attempt in attempts {
                    if matches!(
                        attempt.state,
                        RoleAttemptState::Starting
                            | RoleAttemptState::Provisioning
                            | RoleAttemptState::Running
                    ) {
                        // Reconcile against the process this attempt was actually
                        // launched as. Without a durable handle id there is
                        // nothing to ask the backplane about.
                        let Some(handle_id) = attempt.runtime_handle_id.clone() else {
                            attempt.state = RoleAttemptState::Lost;
                            attempt.runtime_outcome = Some(
                                "no runtime handle was recorded before the daemon restarted"
                                    .to_owned(),
                            );
                            update_durable_role_attempt(&self.store, &attempt, fence).await?;
                            continue;
                        };

                        let handle = RuntimeHandle {
                            handle_id,
                            backplane_id: attempt
                                .runtime_backplane_id
                                .clone()
                                .map(relay_runtime::BackplaneId::new)
                                .unwrap_or_else(|| self.backplane.id()),
                            pid: None,
                            // The handle timestamp is not authoritative for
                            // reconciliation; only the identity is.
                            created_at: "unknown".to_owned(),
                        };

                        let outcome = self.backplane.reconcile(&handle).await;
                        match outcome {
                            Ok(ReconcileOutcome::Alive { .. }) => {
                                // Still alive
                            }
                            Ok(ReconcileOutcome::Orphaned { pid }) => {
                                // The backplane no longer owns this process, but
                                // a provider process from a previous daemon is
                                // still alive. Terminate its whole tree before
                                // marking the attempt lost so no orphan is left
                                // running behind the new daemon.
                                relay_runtime::terminate_pid_tree(pid).await;
                                attempt.state = RoleAttemptState::Lost;
                                attempt.runtime_outcome = Some(
                                    "orphaned provider process from a previous daemon was terminated"
                                        .to_owned(),
                                );
                                update_durable_role_attempt(&self.store, &attempt, fence).await?;
                            }
                            _ => {
                                // Process died or backplane was restarted: reconcile attempt state
                                attempt.state = RoleAttemptState::Lost;
                                attempt.runtime_outcome =
                                    Some("process lost across daemon restart".to_owned());
                                update_durable_role_attempt(&self.store, &attempt, fence).await?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
