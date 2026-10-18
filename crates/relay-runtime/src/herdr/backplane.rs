//! Hosts one trusted Relay role attempt in each Herdr pane.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    RuntimeMetadata,
    backplane::{
        BackplaneError, BackplaneFuture, BackplaneId, BackplaneSnapshot, DisposeReceipt,
        InterruptReceipt, ProcessStatus, ReconcileOutcome, RuntimeBackplane, RuntimeBinding,
        RuntimeBindingId, RuntimeBindingState, RuntimeHandle, RuntimeObservation,
        RuntimeProvisionRequest, SendReceipt,
    },
    herdr::{
        HerdrClientError, HerdrOps, HerdrProcessInfo, HerdrTopologyPlanner, TrustedToken,
        binding::RuntimeBindingStore,
    },
    role_host::{RoleHostSpec, role_host_result_path, role_host_spec_path, write_role_host_spec},
};

const SHELL_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

struct HerdrAttempt {
    tab_id: String,
    spec_path: PathBuf,
}

pub struct HerdrBackplane<O: HerdrOps> {
    id: BackplaneId,
    ops: Arc<O>,
    topology: HerdrTopologyPlanner,
    bindings: RuntimeBindingStore,
    workspaces: Mutex<HashMap<PathBuf, String>>,
    attempts: Mutex<HashMap<String, HerdrAttempt>>,
    relay_executable: PathBuf,
    role_host_dir: PathBuf,
    capacity_slots: usize,
}

impl<O: HerdrOps> HerdrBackplane<O> {
    pub fn new(
        id: impl Into<String>,
        ops: Arc<O>,
        capacity_slots: usize,
        relay_executable: PathBuf,
        role_host_dir: PathBuf,
    ) -> Self {
        Self {
            id: BackplaneId::new(id),
            ops,
            topology: HerdrTopologyPlanner::new(),
            bindings: RuntimeBindingStore::new(),
            workspaces: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            relay_executable,
            role_host_dir,
            capacity_slots,
        }
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

    async fn ensure_workspace(&self, cwd: &Path, label: &str) -> Result<String, HerdrClientError> {
        if let Some(workspace) = self.workspaces.lock().unwrap().get(cwd).cloned() {
            return Ok(workspace);
        }
        let workspace = match self
            .ops
            .workspace_list()
            .await?
            .into_iter()
            .find(|workspace| workspace.label == label)
        {
            Some(workspace) => workspace.workspace_id,
            None => {
                self.ops
                    .workspace_create(cwd, label)
                    .await?
                    .workspace
                    .workspace_id
            }
        };
        self.workspaces
            .lock()
            .unwrap()
            .insert(cwd.to_path_buf(), workspace.clone());
        Ok(workspace)
    }

    async fn wait_for_shell(&self, pane: &str) -> Result<HerdrProcessInfo, HerdrClientError> {
        tokio::time::timeout(SHELL_READY_TIMEOUT, async {
            loop {
                let info = self.ops.pane_process_info(pane).await?;
                if info.foreground_processes.iter().any(is_shell) {
                    return Ok(info);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| HerdrClientError::Timeout("pane shell did not become ready".into()))?
    }

    async fn cleanup_failed_launch(&self, tab: &str, spec: &Path) {
        let _ = self.ops.tab_close(tab).await;
        let _ = fs::remove_file(spec);
    }

    fn attempt_id(&self, handle: &RuntimeHandle) -> Result<String, BackplaneError> {
        self.bindings
            .get_by_handle(&handle.handle_id)
            .map(|binding| binding.role_attempt_id)
            .ok_or_else(|| BackplaneError::HandleNotFound(handle.handle_id.clone()))
    }

    async fn observe_attempt(
        &self,
        handle: &RuntimeHandle,
        attempt_id: &str,
    ) -> Result<RuntimeObservation, BackplaneError> {
        let result_path = role_host_result_path(&self.role_host_dir, attempt_id)
            .map_err(|error| BackplaneError::Io(error.to_string()))?;
        match fs::read(&result_path) {
            Ok(bytes) => {
                let result: crate::RoleHostResult = serde_json::from_slice(&bytes)
                    .map_err(|error| BackplaneError::Io(error.to_string()))?;
                let status = match result.exit_code {
                    Some(0) => ProcessStatus::ExitedSuccess { exit_code: 0 },
                    exit_code => ProcessStatus::ExitedFailure { exit_code },
                };
                if let Some(binding) = self.bindings.get_by_handle(&handle.handle_id) {
                    let state = if result.interrupted {
                        RuntimeBindingState::Interrupted
                    } else if result.exit_code == Some(0) {
                        RuntimeBindingState::ExitedCleanly {
                            exit_code: result.exit_code,
                        }
                    } else {
                        RuntimeBindingState::ExitedFailed {
                            exit_code: result.exit_code,
                        }
                    };
                    self.bindings.update_state(&binding.id, state);
                }
                let stdout_chunk = result.stdout.into_bytes();
                let stderr_chunk = result.stderr.into_bytes();
                let total_bytes_captured = stdout_chunk.len() + stderr_chunk.len();
                Ok(RuntimeObservation {
                    handle_id: handle.handle_id.clone(),
                    status,
                    stdout_chunk,
                    stderr_chunk,
                    total_bytes_captured,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match self.ops.pane_get(&handle.handle_id).await {
                    Ok(Some(_)) => Ok(RuntimeObservation {
                        handle_id: handle.handle_id.clone(),
                        status: ProcessStatus::Running,
                        stdout_chunk: Vec::new(),
                        stderr_chunk: Vec::new(),
                        total_bytes_captured: 0,
                    }),
                    Ok(None) | Err(HerdrClientError::ResourceNotFound(_)) => {
                        if let Some(binding) = self.bindings.get_by_handle(&handle.handle_id) {
                            self.bindings
                                .update_state(&binding.id, RuntimeBindingState::Interrupted);
                        }
                        Ok(RuntimeObservation {
                            handle_id: handle.handle_id.clone(),
                            status: ProcessStatus::Interrupted,
                            stdout_chunk: Vec::new(),
                            stderr_chunk: Vec::new(),
                            total_bytes_captured: 0,
                        })
                    }
                    Err(_) => Ok(lost_observation(handle)),
                }
            }
            Err(error) => Err(BackplaneError::Io(error.to_string())),
        }
    }

    async fn reconcile_attempt(
        &self,
        handle: &RuntimeHandle,
        attempt_id: &str,
    ) -> Result<ReconcileOutcome, BackplaneError> {
        let observation = self.observe_attempt(handle, attempt_id).await?;
        match observation.status {
            ProcessStatus::ExitedSuccess { exit_code } => Ok(ReconcileOutcome::CleanlyExited {
                exit_code: Some(exit_code),
            }),
            ProcessStatus::ExitedFailure { exit_code } => {
                Ok(ReconcileOutcome::FailedExit { exit_code })
            }
            ProcessStatus::Interrupted => Ok(ReconcileOutcome::FailedExit { exit_code: None }),
            ProcessStatus::Lost => Ok(ReconcileOutcome::Mismatched {
                reason: "Herdr is unreachable; pane state could not be confirmed".into(),
            }),
            ProcessStatus::Starting | ProcessStatus::Running => {
                match self.ops.pane_process_info(&handle.handle_id).await {
                    Ok(info) => info
                        .shell_pid
                        .or_else(|| info.foreground_processes.first().map(|process| process.pid))
                        .map(|pid| ReconcileOutcome::Alive { pid })
                        .ok_or_else(|| {
                            BackplaneError::ReconciliationFailed(
                                "Herdr reported a live pane without a process ID".into(),
                            )
                        }),
                    Err(error) => Ok(ReconcileOutcome::Mismatched {
                        reason: format!("Herdr could not report pane process info: {error}"),
                    }),
                }
            }
        }
    }
}

impl<O: HerdrOps> RuntimeBackplane for HerdrBackplane<O> {
    fn id(&self) -> BackplaneId {
        self.id.clone()
    }

    fn provision<'a>(&'a self, req: RuntimeProvisionRequest) -> BackplaneFuture<'a, RuntimeHandle> {
        Box::pin(async move {
            if let Some(binding) = self.bindings.get_by_effect(&req.effect_key) {
                return Ok(binding.runtime_handle);
            }
            if !req.trusted_cwd.is_dir() {
                return Err(BackplaneError::UnauthorizedCwd(req.trusted_cwd));
            }

            let relay_path = self.relay_executable.to_str().ok_or_else(|| {
                BackplaneError::LaunchFailed(format!(
                    "Relay's executable path contains characters that can't be typed safely into a Herdr pane: {}",
                    self.relay_executable.display()
                ))
            })?;
            let relay_token = TrustedToken::new(relay_path).map_err(|_| {
                BackplaneError::LaunchFailed(format!(
                    "Relay's executable path contains characters that can't be typed safely into a Herdr pane: {}",
                    self.relay_executable.display()
                ))
            })?;
            let attempt_token = TrustedToken::new(req.role_attempt_id.clone())
                .map_err(|error| BackplaneError::LaunchFailed(error.to_string()))?;
            let command = [
                relay_token,
                TrustedToken::new("role-host").unwrap(),
                TrustedToken::new("--attempt").unwrap(),
                attempt_token,
            ];

            let spec = RoleHostSpec::from_request(&self.role_host_dir, &req)
                .map_err(|error| BackplaneError::LaunchFailed(error.to_string()))?;
            let spec_path = write_role_host_spec(&self.role_host_dir, &spec)
                .map_err(|error| BackplaneError::LaunchFailed(error.to_string()))?;
            let plan = self.topology.plan(&req);

            let workspace = match self
                .ensure_workspace(&req.trusted_cwd, &plan.workspace_label)
                .await
            {
                Ok(workspace) => workspace,
                Err(error) => {
                    let _ = fs::remove_file(&spec_path);
                    return Err(BackplaneError::LaunchFailed(format!(
                        "failed to ensure workspace: {error}"
                    )));
                }
            };
            let tab = match self
                .ops
                .tab_create(&workspace, &req.trusted_cwd, &plan.tab_label)
                .await
            {
                Ok(tab) => tab,
                Err(error) => {
                    let _ = fs::remove_file(&spec_path);
                    return Err(BackplaneError::LaunchFailed(format!(
                        "failed to create attempt tab: {error}"
                    )));
                }
            };

            let shell = match self.wait_for_shell(&tab.root_pane.pane_id).await {
                Ok(shell) => shell,
                Err(error) => {
                    self.cleanup_failed_launch(&tab.tab.tab_id, &spec_path)
                        .await;
                    return Err(BackplaneError::LaunchFailed(format!(
                        "pane shell did not become ready: {error}"
                    )));
                }
            };
            if let Err(error) = self
                .ops
                .pane_run_trusted(&tab.root_pane.pane_id, &command)
                .await
            {
                self.cleanup_failed_launch(&tab.tab.tab_id, &spec_path)
                    .await;
                return Err(BackplaneError::LaunchFailed(format!(
                    "failed to start Relay role-host: {error}"
                )));
            }
            let metadata = plan.metadata_tags.into_iter().collect::<Vec<_>>();
            let _ = self
                .ops
                .pane_report_metadata(
                    &tab.root_pane.pane_id,
                    "relay",
                    &req.role_attempt_id,
                    &metadata,
                )
                .await;

            let handle = RuntimeHandle {
                handle_id: tab.root_pane.pane_id.clone(),
                backplane_id: self.id.clone(),
                pid: shell.shell_pid,
                created_at: now_timestamp(),
            };
            let mut metadata = req.metadata.clone().unwrap_or_default();
            metadata.role_attempt_id = req.role_attempt_id.clone();
            metadata.effect_key = req.effect_key.clone();
            self.bindings.insert(RuntimeBinding {
                id: RuntimeBindingId::new(uuid::Uuid::new_v4().to_string()),
                role_attempt_id: req.role_attempt_id.clone(),
                backplane_id: self.id.clone(),
                runtime_handle: handle.clone(),
                created_at: now_timestamp(),
                last_reconciled_at: now_timestamp(),
                state: RuntimeBindingState::Running,
                metadata: Some(metadata),
            });
            self.attempts.lock().unwrap().insert(
                handle.handle_id.clone(),
                HerdrAttempt {
                    tab_id: tab.tab.tab_id,
                    spec_path,
                },
            );
            Ok(handle)
        })
    }

    fn observe<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, RuntimeObservation> {
        Box::pin(async move {
            let attempt_id = self.attempt_id(handle)?;
            self.observe_attempt(handle, &attempt_id).await
        })
    }

    fn send<'a>(
        &'a self,
        _handle: &'a RuntimeHandle,
        _input: &'a [u8],
    ) -> BackplaneFuture<'a, SendReceipt> {
        Box::pin(async {
            Err(BackplaneError::Unsupported(
                "Herdr role panes do not support runtime input".into(),
            ))
        })
    }

    fn interrupt<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, InterruptReceipt> {
        Box::pin(async move {
            if self.bindings.get_by_handle(&handle.handle_id).is_none() {
                return Err(BackplaneError::HandleNotFound(handle.handle_id.clone()));
            }
            self.ops
                .pane_send_keys(&handle.handle_id, &["ctrl+c".into()])
                .await
                .map_err(|error| match error {
                    HerdrClientError::ResourceNotFound(_) => {
                        BackplaneError::HandleNotFound(handle.handle_id.clone())
                    }
                    other => BackplaneError::Io(other.to_string()),
                })?;
            Ok(InterruptReceipt { initiated: true })
        })
    }

    fn dispose<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, DisposeReceipt> {
        Box::pin(async move {
            let attempt = self
                .attempts
                .lock()
                .unwrap()
                .get(&handle.handle_id)
                .map(|attempt| (attempt.tab_id.clone(), attempt.spec_path.clone()));
            let (tab_id, spec_path) = match attempt {
                Some(attempt) => attempt,
                None => {
                    let pane = match self.ops.pane_get(&handle.handle_id).await {
                        Ok(Some(pane)) => pane,
                        Ok(None) | Err(HerdrClientError::ResourceNotFound(_)) => {
                            return Ok(DisposeReceipt { reaped: false });
                        }
                        Err(error) => return Err(BackplaneError::Io(error.to_string())),
                    };
                    let binding = self
                        .bindings
                        .get_by_handle(&handle.handle_id)
                        .ok_or_else(|| BackplaneError::HandleNotFound(handle.handle_id.clone()))?;
                    let spec_path =
                        role_host_spec_path(&self.role_host_dir, &binding.role_attempt_id)
                            .map_err(|error| BackplaneError::Io(error.to_string()))?;
                    (pane.tab_id, spec_path)
                }
            };
            let reaped = match self.ops.tab_close(&tab_id).await {
                Ok(()) => true,
                Err(HerdrClientError::ResourceNotFound(_)) => false,
                Err(error) => return Err(BackplaneError::Io(error.to_string())),
            };
            let _ = fs::remove_file(spec_path);
            self.attempts.lock().unwrap().remove(&handle.handle_id);
            if let Some(binding) = self.bindings.get_by_handle(&handle.handle_id) {
                self.bindings
                    .update_state(&binding.id, RuntimeBindingState::Disposed);
            }
            Ok(DisposeReceipt { reaped })
        })
    }

    fn snapshot<'a>(&'a self) -> BackplaneFuture<'a, BackplaneSnapshot> {
        Box::pin(async move {
            Ok(BackplaneSnapshot {
                backplane_id: self.id.clone(),
                active_processes: self.bindings.list_active().len(),
                capacity_slots: self.capacity_slots,
            })
        })
    }

    fn reconcile<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, ReconcileOutcome> {
        Box::pin(async move {
            let attempt_id = self.attempt_id(handle)?;
            self.reconcile_attempt(handle, &attempt_id).await
        })
    }

    fn reconcile_binding<'a>(
        &'a self,
        binding: &'a RuntimeBinding,
    ) -> BackplaneFuture<'a, ReconcileOutcome> {
        Box::pin(async move {
            self.reconcile_attempt(&binding.runtime_handle, &binding.role_attempt_id)
                .await
        })
    }
}

fn is_shell(process: &crate::HerdrPaneProcess) -> bool {
    let name = process
        .name
        .rsplit(|character| character == '/' || character == '\\')
        .next()
        .unwrap_or(&process.name)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "powershell.exe" | "pwsh.exe" | "cmd.exe" | "sh" | "bash" | "zsh" | "fish"
    )
}

fn lost_observation(handle: &RuntimeHandle) -> RuntimeObservation {
    RuntimeObservation {
        handle_id: handle.handle_id.clone(),
        status: ProcessStatus::Lost,
        stdout_chunk: Vec::new(),
        stderr_chunk: Vec::new(),
        total_bytes_captured: 0,
    }
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}
