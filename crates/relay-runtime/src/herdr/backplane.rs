//! HerdrBackplane implementation conforming to the RuntimeBackplane trait.
//!
//! Provides the reference interactive execution hosting seam.
//! Encapsulates physical workspaces, tabs, and panes behind provider-neutral
//! execution abstractions.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use crate::{
    backplane::{
        BackplaneError, BackplaneFuture, BackplaneId, BackplaneSnapshot, DisposeReceipt,
        InterruptReceipt, ProcessStatus, ReconcileOutcome, RuntimeBackplane, RuntimeBinding,
        RuntimeBindingId, RuntimeBindingState, RuntimeHandle, RuntimeObservation,
        RuntimeProvisionRequest, SendReceipt,
    },
    herdr::{
        binding::RuntimeBindingStore,
        client::{HerdrClient, HerdrPaneStatus, HerdrTransport},
        topology::HerdrTopologyPlanner,
    },
};

pub struct HerdrBackplane {
    id: BackplaneId,
    client: HerdrClient,
    topology: HerdrTopologyPlanner,
    bindings: RuntimeBindingStore,
    owned_resources: Arc<Mutex<HashSet<String>>>,
    byte_limits: Arc<Mutex<HashMap<String, usize>>>,
    total_bytes_captured: Arc<Mutex<HashMap<String, usize>>>,
    capacity_slots: usize,
}

impl HerdrBackplane {
    pub fn new(
        id: impl Into<String>,
        transport: Arc<dyn HerdrTransport>,
        capacity_slots: usize,
    ) -> Self {
        Self {
            id: BackplaneId::new(id),
            client: HerdrClient::new(transport),
            topology: HerdrTopologyPlanner::new(),
            bindings: RuntimeBindingStore::new(),
            owned_resources: Arc::new(Mutex::new(HashSet::new())),
            byte_limits: Arc::new(Mutex::new(HashMap::new())),
            total_bytes_captured: Arc::new(Mutex::new(HashMap::new())),
            capacity_slots,
        }
    }

    pub fn client(&self) -> &HerdrClient {
        &self.client
    }

    pub fn topology(&self) -> &HerdrTopologyPlanner {
        &self.topology
    }

    pub fn bindings(&self) -> &RuntimeBindingStore {
        &self.bindings
    }

    pub fn with_bindings(mut self, bindings: RuntimeBindingStore) -> Self {
        self.bindings = bindings;
        self
    }
}

impl RuntimeBackplane for HerdrBackplane {
    fn id(&self) -> BackplaneId {
        self.id.clone()
    }

    fn provision<'a>(&'a self, req: RuntimeProvisionRequest) -> BackplaneFuture<'a, RuntimeHandle> {
        Box::pin(async move {
            // 1. Idempotency check: if effect_key has already been provisioned, return existing handle
            if let Some(existing_binding) = self.bindings.get_by_effect(&req.effect_key) {
                return Ok(existing_binding.runtime_handle);
            }

            // 2. Validate trusted CWD
            if !req.trusted_cwd.exists() && !req.trusted_cwd.is_dir() {
                return Err(BackplaneError::UnauthorizedCwd(req.trusted_cwd));
            }

            // 3. Plan topology (internal adapter concern)
            let plan = self.topology.plan(&req);

            // 4. Ensure physical Herdr hierarchy
            self.client
                .ensure_workspace(
                    &plan.workspace_id,
                    &plan.workspace_title,
                    Some(&req.trusted_cwd),
                )
                .await
                .map_err(|e| {
                    BackplaneError::LaunchFailed(format!("failed to ensure workspace: {}", e))
                })?;

            self.client
                .ensure_tab(&plan.tab_id, &plan.workspace_id, &plan.tab_title)
                .await
                .map_err(|e| {
                    BackplaneError::LaunchFailed(format!("failed to ensure tab: {}", e))
                })?;

            // 5. Spawn pane
            let pane_info = self
                .client
                .spawn_pane(
                    &plan.pane_id,
                    &plan.tab_id,
                    &plan.workspace_id,
                    &req.launch.executable,
                    &req.launch.args,
                    &req.trusted_cwd,
                    &req.environment.vars,
                    &plan.metadata_tags,
                )
                .await
                .map_err(|e| {
                    BackplaneError::LaunchFailed(format!("failed to spawn pane: {}", e))
                })?;

            // 6. Record resource ownership and limits
            self.owned_resources
                .lock()
                .unwrap()
                .insert(plan.pane_id.clone());
            self.byte_limits
                .lock()
                .unwrap()
                .insert(plan.pane_id.clone(), req.output_byte_limit);
            self.total_bytes_captured
                .lock()
                .unwrap()
                .insert(plan.pane_id.clone(), 0);

            // 7. Create RuntimeHandle
            let handle = RuntimeHandle {
                handle_id: plan.pane_id.clone(),
                backplane_id: self.id(),
                pid: Some(pane_info.pid),
                created_at: "now".to_string(),
            };

            // 8. Persist durable binding
            let binding = RuntimeBinding {
                id: RuntimeBindingId::new(uuid::Uuid::new_v4().to_string()),
                role_attempt_id: req.role_attempt_id.clone(),
                backplane_id: self.id(),
                runtime_handle: handle.clone(),
                created_at: "now".to_string(),
                last_reconciled_at: "now".to_string(),
                state: RuntimeBindingState::Running,
                metadata: req.metadata.clone(),
            };
            self.bindings.insert(binding);

            Ok(handle)
        })
    }

    fn observe<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, RuntimeObservation> {
        Box::pin(async move {
            let pane = match self.client.get_pane(&handle.handle_id).await {
                Ok(p) => p,
                Err(crate::herdr::client::HerdrClientError::ResourceNotFound(_)) => {
                    return Ok(RuntimeObservation {
                        handle_id: handle.handle_id.clone(),
                        status: ProcessStatus::Lost,
                        stdout_chunk: Vec::new(),
                        stderr_chunk: Vec::new(),
                        total_bytes_captured: 0,
                    });
                }
                Err(e) => return Err(BackplaneError::Io(e.to_string())),
            };

            // Read output chunks
            let output = self
                .client
                .read_output(&handle.handle_id)
                .await
                .map_err(|e| BackplaneError::Io(e.to_string()))?;

            let byte_limit = self
                .byte_limits
                .lock()
                .unwrap()
                .get(&handle.handle_id)
                .copied()
                .unwrap_or(usize::MAX);

            let mut total_captured = self.total_bytes_captured.lock().unwrap();
            let current_total = total_captured.entry(handle.handle_id.clone()).or_insert(0);

            let remaining_budget = byte_limit.saturating_sub(*current_total);

            let mut stdout_chunk = output.stdout;
            let mut stderr_chunk = output.stderr;

            if stdout_chunk.len() > remaining_budget {
                stdout_chunk.truncate(remaining_budget);
            }
            let remaining_budget = remaining_budget.saturating_sub(stdout_chunk.len());
            if stderr_chunk.len() > remaining_budget {
                stderr_chunk.truncate(remaining_budget);
            }

            *current_total += stdout_chunk.len() + stderr_chunk.len();
            let final_captured = *current_total;
            drop(total_captured);

            // Normalize status and detect manual user intervention
            let status = match pane.status {
                HerdrPaneStatus::Starting => ProcessStatus::Starting,
                HerdrPaneStatus::Running => ProcessStatus::Running,
                HerdrPaneStatus::Exited { code: 0 } => {
                    ProcessStatus::ExitedSuccess { exit_code: 0 }
                }
                HerdrPaneStatus::Exited { code } => ProcessStatus::ExitedFailure {
                    exit_code: Some(code),
                },
                // Manual user termination reflects as failure/interrupted, NEVER fabricates completion!
                HerdrPaneStatus::TerminatedByUser => ProcessStatus::Interrupted,
                HerdrPaneStatus::Lost => ProcessStatus::Lost,
            };

            Ok(RuntimeObservation {
                handle_id: handle.handle_id.clone(),
                status,
                stdout_chunk,
                stderr_chunk,
                total_bytes_captured: final_captured,
            })
        })
    }

    fn send<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        input: &'a [u8],
    ) -> BackplaneFuture<'a, SendReceipt> {
        Box::pin(async move {
            self.client
                .send_input(&handle.handle_id, input)
                .await
                .map_err(|e| match e {
                    crate::herdr::client::HerdrClientError::ResourceNotFound(_) => {
                        BackplaneError::HandleNotFound(handle.handle_id.clone())
                    }
                    other => BackplaneError::Io(other.to_string()),
                })?;

            Ok(SendReceipt {
                bytes_written: input.len(),
            })
        })
    }

    fn interrupt<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, InterruptReceipt> {
        Box::pin(async move {
            self.client
                .interrupt_pane(&handle.handle_id)
                .await
                .map_err(|e| match e {
                    crate::herdr::client::HerdrClientError::ResourceNotFound(_) => {
                        BackplaneError::HandleNotFound(handle.handle_id.clone())
                    }
                    other => BackplaneError::Io(other.to_string()),
                })?;

            Ok(InterruptReceipt { initiated: true })
        })
    }

    fn dispose<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, DisposeReceipt> {
        Box::pin(async move {
            // Check ownership: never dispose user-created unrelated Herdr resources!
            let is_owned = self
                .owned_resources
                .lock()
                .unwrap()
                .remove(&handle.handle_id);

            if !is_owned {
                return Ok(DisposeReceipt { reaped: false });
            }

            let reaped = self
                .client
                .dispose_pane(&handle.handle_id)
                .await
                .unwrap_or(false);

            if let Some(binding) = self.bindings.get_by_handle(&handle.handle_id) {
                self.bindings
                    .update_state(&binding.id, RuntimeBindingState::Disposed);
            }

            Ok(DisposeReceipt { reaped })
        })
    }

    fn snapshot<'a>(&'a self) -> BackplaneFuture<'a, BackplaneSnapshot> {
        Box::pin(async move {
            let panes = self
                .client
                .list_panes()
                .await
                .map_err(|e| BackplaneError::Io(e.to_string()))?;

            let active_processes = panes
                .iter()
                .filter(|p| {
                    matches!(
                        p.status,
                        HerdrPaneStatus::Starting | HerdrPaneStatus::Running
                    )
                })
                .count();

            Ok(BackplaneSnapshot {
                backplane_id: self.id(),
                active_processes,
                capacity_slots: self.capacity_slots,
            })
        })
    }

    fn reconcile<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, ReconcileOutcome> {
        Box::pin(async move {
            let binding = self.bindings.get_by_handle(&handle.handle_id);

            let pane = match self.client.get_pane(&handle.handle_id).await {
                Ok(p) => p,
                Err(crate::herdr::client::HerdrClientError::ResourceNotFound(_)) => {
                    if let Some(b) = &binding {
                        self.bindings
                            .update_state(&b.id, RuntimeBindingState::Missing);
                    }
                    return Ok(ReconcileOutcome::ResourceNotFound);
                }
                Err(e) => return Err(BackplaneError::Io(e.to_string())),
            };

            // Reconcile metadata: ensure pane was not hijacked or repurposed
            if let Some(b) = &binding {
                if let Some(meta) = &b.metadata {
                    if let Some(tag_attempt) = pane.metadata.get("relay.role_attempt_id") {
                        if tag_attempt != &meta.role_attempt_id {
                            self.bindings.update_state(
                                &b.id,
                                RuntimeBindingState::Mismatched {
                                    reason: format!(
                                        "attempt mismatch: expected {}, found {}",
                                        meta.role_attempt_id, tag_attempt
                                    ),
                                },
                            );
                            return Ok(ReconcileOutcome::Mismatched {
                                reason: "Pane role attempt metadata mismatch".to_string(),
                            });
                        }
                    }
                }
            }

            match pane.status {
                HerdrPaneStatus::Starting | HerdrPaneStatus::Running => {
                    if let Some(b) = &binding {
                        self.bindings
                            .update_state(&b.id, RuntimeBindingState::Running);
                    }
                    Ok(ReconcileOutcome::Alive { pid: pane.pid })
                }
                HerdrPaneStatus::Exited { code: 0 } => {
                    if let Some(b) = &binding {
                        self.bindings.update_state(
                            &b.id,
                            RuntimeBindingState::ExitedCleanly { exit_code: Some(0) },
                        );
                    }
                    Ok(ReconcileOutcome::CleanlyExited { exit_code: Some(0) })
                }
                HerdrPaneStatus::Exited { code } => {
                    if let Some(b) = &binding {
                        self.bindings.update_state(
                            &b.id,
                            RuntimeBindingState::ExitedFailed {
                                exit_code: Some(code),
                            },
                        );
                    }
                    Ok(ReconcileOutcome::FailedExit {
                        exit_code: Some(code),
                    })
                }
                HerdrPaneStatus::TerminatedByUser => {
                    if let Some(b) = &binding {
                        self.bindings
                            .update_state(&b.id, RuntimeBindingState::Interrupted);
                    }
                    Ok(ReconcileOutcome::FailedExit { exit_code: None })
                }
                HerdrPaneStatus::Lost => {
                    if let Some(b) = &binding {
                        self.bindings
                            .update_state(&b.id, RuntimeBindingState::Missing);
                    }
                    Ok(ReconcileOutcome::ResourceNotFound)
                }
            }
        })
    }

    fn reconcile_binding<'a>(
        &'a self,
        binding: &'a RuntimeBinding,
    ) -> BackplaneFuture<'a, ReconcileOutcome> {
        self.reconcile(&binding.runtime_handle)
    }
}
