//! Phase 06: LocalProcessBackplane.
//!
//! Direct local process host using explicit executable + argument arrays,
//! trusted CWD enforcement, streaming output capturing, and process tree termination.

use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::mpsc,
};

use crate::backplane::{
    BackplaneError, BackplaneFuture, BackplaneId, BackplaneSnapshot, DisposeReceipt,
    InterruptReceipt, ProcessStatus, ReconcileOutcome, RuntimeBackplane, RuntimeHandle,
    RuntimeObservation, RuntimeProvisionRequest, SendReceipt,
};
use crate::process_tree::{
    ProcessIdentity, process_identity, terminate_pid_tree, terminate_process_tree,
};

/// OS-provided environment variables that are copied from the parent process.
///
/// The child environment starts cleared; these names alone are inherited
/// because a process cannot start without them (Windows in particular needs
/// `SystemRoot`, `windir` and `PATH`). Every other parent variable is dropped.
/// A name present in the request's sanitized vars overrides the inherited value.
const PARENT_ENV_ALLOW_LIST: [&str; 14] = [
    "SystemRoot",
    "SYSTEMDRIVE",
    "windir",
    "TEMP",
    "TMP",
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "HOME",
    "USER",
    "LANG",
];

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

struct ProcessSlot {
    child: Child,
    stdin_tx: Option<mpsc::Sender<Vec<u8>>>,
    stdout_buf: Arc<Mutex<Vec<u8>>>,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    exited_code: Option<Option<i32>>,
}

#[derive(Clone)]
pub struct LocalProcessBackplane {
    id: BackplaneId,
    capacity: usize,
    active: Arc<Mutex<HashMap<String, ProcessSlot>>>,
    provisioned_keys: Arc<Mutex<HashMap<String, RuntimeHandle>>>,
}

impl Default for LocalProcessBackplane {
    fn default() -> Self {
        Self::new("local-process-default", 8)
    }
}

impl LocalProcessBackplane {
    pub fn new(id: impl Into<String>, capacity: usize) -> Self {
        Self {
            id: BackplaneId::new(id),
            capacity,
            active: Arc::new(Mutex::new(HashMap::new())),
            provisioned_keys: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl RuntimeBackplane for LocalProcessBackplane {
    fn id(&self) -> BackplaneId {
        self.id.clone()
    }

    fn provision<'a>(&'a self, req: RuntimeProvisionRequest) -> BackplaneFuture<'a, RuntimeHandle> {
        Box::pin(async move {
            // Enforce duplicate effect key idempotency
            {
                let keys = self.provisioned_keys.lock().unwrap();
                if let Some(existing) = keys.get(&req.effect_key.0) {
                    return Ok(existing.clone());
                }
            }

            // Verify trusted CWD exists
            if !req.trusted_cwd.is_dir() {
                return Err(BackplaneError::UnauthorizedCwd(req.trusted_cwd.clone()));
            }

            // Prepare command with explicit args (never arbitrary shell strings)
            let mut cmd = Command::new(&req.launch.executable);
            cmd.args(&req.launch.args);
            cmd.current_dir(&req.trusted_cwd);

            // On Unix, start the child as its own process-group leader so a
            // later group kill reaches the whole process tree.
            #[cfg(unix)]
            cmd.process_group(0);

            // Start cleared: inherit only the OS allow-list from the parent,
            // then let the sanitized vars override anything.
            cmd.env_clear();
            for name in PARENT_ENV_ALLOW_LIST {
                if let Some(value) = std::env::var_os(name) {
                    cmd.env(name, value);
                }
            }
            for (k, v) in &req.environment.vars {
                cmd.env(k, v);
            }

            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
            cmd.stdin(Stdio::piped());

            let mut child = cmd
                .spawn()
                .map_err(|e| BackplaneError::LaunchFailed(e.to_string()))?;

            let pid = child.id();
            let handle_id = format!("proc-{}", uuid::Uuid::new_v4().simple());
            let handle = RuntimeHandle {
                handle_id: handle_id.clone(),
                backplane_id: self.id.clone(),
                pid,
                created_at: now_timestamp(),
            };

            let stdout_buf = Arc::new(Mutex::new(Vec::new()));
            let stderr_buf = Arc::new(Mutex::new(Vec::new()));

            // Stream stdout in background task
            if let Some(mut stdout) = child.stdout.take() {
                let sbuf = stdout_buf.clone();
                let limit = req.output_byte_limit;
                tokio::spawn(async move {
                    let mut chunk = [0u8; 1024];
                    while let Ok(n) = stdout.read(&mut chunk).await {
                        if n == 0 {
                            break;
                        }
                        let mut b = sbuf.lock().unwrap();
                        if b.len() < limit {
                            let take = (limit - b.len()).min(n);
                            b.extend_from_slice(&chunk[..take]);
                        }
                    }
                });
            }

            // Stream stderr in background task
            if let Some(mut stderr) = child.stderr.take() {
                let sbuf = stderr_buf.clone();
                let limit = req.output_byte_limit;
                tokio::spawn(async move {
                    let mut chunk = [0u8; 1024];
                    while let Ok(n) = stderr.read(&mut chunk).await {
                        if n == 0 {
                            break;
                        }
                        let mut b = sbuf.lock().unwrap();
                        if b.len() < limit {
                            let take = (limit - b.len()).min(n);
                            b.extend_from_slice(&chunk[..take]);
                        }
                    }
                });
            }

            // Setup stdin channel to avoid holding mutex locks across async writes
            let stdin_tx = if let Some(mut stdin) = child.stdin.take() {
                let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    while let Some(bytes) = rx.recv().await {
                        if stdin.write_all(&bytes).await.is_err() {
                            break;
                        }
                        if stdin.flush().await.is_err() {
                            break;
                        }
                    }
                });
                Some(tx)
            } else {
                None
            };

            let slot = ProcessSlot {
                child,
                stdin_tx,
                stdout_buf,
                stderr_buf,
                exited_code: None,
            };

            self.active.lock().unwrap().insert(handle_id.clone(), slot);
            self.provisioned_keys
                .lock()
                .unwrap()
                .insert(req.effect_key.0, handle.clone());

            Ok(handle)
        })
    }

    fn observe<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, RuntimeObservation> {
        Box::pin(async move {
            let mut active = self.active.lock().unwrap();
            let slot = active
                .get_mut(&handle.handle_id)
                .ok_or_else(|| BackplaneError::HandleNotFound(handle.handle_id.clone()))?;

            // Poll child exit status non-blockingly
            if slot.exited_code.is_none() {
                if let Ok(Some(status)) = slot.child.try_wait() {
                    slot.exited_code = Some(status.code());
                }
            }

            let status = match slot.exited_code {
                Some(Some(0)) => ProcessStatus::ExitedSuccess { exit_code: 0 },
                Some(Some(c)) => ProcessStatus::ExitedFailure { exit_code: Some(c) },
                Some(None) => ProcessStatus::ExitedFailure { exit_code: None },
                None => ProcessStatus::Running,
            };

            let stdout_chunk = slot.stdout_buf.lock().unwrap().clone();
            let stderr_chunk = slot.stderr_buf.lock().unwrap().clone();
            let total = stdout_chunk.len() + stderr_chunk.len();

            Ok(RuntimeObservation {
                handle_id: handle.handle_id.clone(),
                status,
                stdout_chunk,
                stderr_chunk,
                total_bytes_captured: total,
            })
        })
    }

    fn send<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        input: &'a [u8],
    ) -> BackplaneFuture<'a, SendReceipt> {
        Box::pin(async move {
            let tx = {
                let active = self.active.lock().unwrap();
                let slot = active
                    .get(&handle.handle_id)
                    .ok_or_else(|| BackplaneError::HandleNotFound(handle.handle_id.clone()))?;
                slot.stdin_tx
                    .clone()
                    .ok_or_else(|| BackplaneError::Io("stdin unavailable".to_owned()))?
            };

            tx.send(input.to_vec())
                .await
                .map_err(|e| BackplaneError::Io(e.to_string()))?;

            Ok(SendReceipt {
                bytes_written: input.len(),
            })
        })
    }

    fn interrupt<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, InterruptReceipt> {
        Box::pin(async move {
            // Copy the PID out instead of holding the std::sync::Mutex guard
            // across the termination await.
            let pid = {
                let mut active = self.active.lock().unwrap();
                let slot = active
                    .get_mut(&handle.handle_id)
                    .ok_or_else(|| BackplaneError::HandleNotFound(handle.handle_id.clone()))?;

                slot.exited_code = Some(None);
                slot.child.id()
            };

            if let Some(pid) = pid {
                terminate_pid_tree(pid).await;
            }

            Ok(InterruptReceipt { initiated: true })
        })
    }

    fn dispose<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, DisposeReceipt> {
        Box::pin(async move {
            let mut removed = {
                let mut active = self.active.lock().unwrap();
                active.remove(&handle.handle_id)
            };

            if let Some(ref mut slot) = removed {
                // The slot is already out of the map, so the guard is not held
                // across the await. Kill the whole tree, not just the child.
                terminate_process_tree(&mut slot.child).await;
                Ok(DisposeReceipt { reaped: true })
            } else {
                Ok(DisposeReceipt { reaped: false })
            }
        })
    }

    fn snapshot<'a>(&'a self) -> BackplaneFuture<'a, BackplaneSnapshot> {
        Box::pin(async move {
            let active = self.active.lock().unwrap().len();
            Ok(BackplaneSnapshot {
                backplane_id: self.id.clone(),
                active_processes: active,
                capacity_slots: self.capacity,
            })
        })
    }

    fn reconcile<'a>(&'a self, handle: &'a RuntimeHandle) -> BackplaneFuture<'a, ReconcileOutcome> {
        Box::pin(async move {
            // Classify the slot while the lock is held, then drop the guard
            // before any await (the orphan liveness probe).
            let state = {
                let mut active = self.active.lock().unwrap();
                match active.get_mut(&handle.handle_id) {
                    Some(slot) => match slot.child.try_wait() {
                        Ok(Some(status)) => SlotState::Exited(status.code()),
                        _ => SlotState::Running(handle.pid),
                    },
                    None => SlotState::Absent,
                }
            };

            match state {
                SlotState::Exited(exit_code) => Ok(ReconcileOutcome::CleanlyExited { exit_code }),
                SlotState::Running(Some(pid)) => Ok(ReconcileOutcome::Alive { pid }),
                SlotState::Running(None) => Ok(ReconcileOutcome::ResourceNotFound),
                // Not active in this backplane instance. A durable handle from
                // a previous daemon may still be running, but a live PID alone
                // is not proof: operating systems recycle PIDs, so the process
                // at that PID may now be an unrelated program. Only a verified
                // match is reported as killable.
                SlotState::Absent => match handle.pid {
                    Some(pid) => match process_identity(pid, &handle.created_at).await {
                        ProcessIdentity::Same => Ok(ReconcileOutcome::Orphaned { pid }),
                        ProcessIdentity::Dead => Ok(ReconcileOutcome::ResourceNotFound),
                        ProcessIdentity::Different => Ok(ReconcileOutcome::OrphanUnverified {
                            pid,
                            reason: format!(
                                "pid {pid} now belongs to a process started after Relay \
                                     launched it; the pid was reused"
                            ),
                        }),
                        ProcessIdentity::Unverifiable => Ok(ReconcileOutcome::OrphanUnverified {
                            pid,
                            reason: format!(
                                "pid {pid} could not be proven to identify the process Relay \
                                     launched"
                            ),
                        }),
                    },
                    None => Ok(ReconcileOutcome::ResourceNotFound),
                },
            }
        })
    }
}

/// Classification of a handle against this backplane instance's active slots.
enum SlotState {
    Exited(Option<i32>),
    Running(Option<u32>),
    Absent,
}
