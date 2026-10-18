//! Relay Architecture v2 - Wave A: daemon process lifecycle.
//!
//! Owns starting a full daemon (lease heartbeat + request listener + effect
//! worker + startup reconciliation) and the client-side logic for either
//! connecting to an already-running daemon or embedding one in-process when
//! none is running yet.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use tokio::sync::watch;

use relay_store::StoreError;

use crate::{
    Relay, SqliteStore,
    daemon::{
        CommandStatus, DaemonClient, DaemonCommand, DaemonError, DaemonEvent, DaemonServer,
        DaemonSnapshot, InProcessDaemonClient,
    },
    daemon_transport::{self, NamedPipeDaemonClient},
    effect_worker::{EffectWorkerRegistry, run_effect_worker},
};

/// Upper bound on how long `run_full_daemon` waits for its listener to bind the
/// transport before reporting startup failure. It exceeds the listener's own
/// bounded takeover retry window, so a genuine bind failure is reported by the
/// listener rather than masked by this timeout.
const LISTENER_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// The listener's readiness report: `Ok(Ok(()))` when the transport bound,
/// `Ok(Err(reason))` when startup was cancelled instead, a dropped sender when
/// the listener could not bind at all, and the outer `Err` when binding took
/// longer than [`LISTENER_READY_TIMEOUT`].
type ListenerReadiness = Result<
    Result<Result<(), String>, tokio::sync::oneshot::error::RecvError>,
    tokio::time::error::Elapsed,
>;

/// Interprets the listener's readiness outcome as a startup failure reason, or
/// `None` when the daemon is genuinely bound and serving.
///
/// A published readiness signal is not readiness if the shared shutdown was
/// already signalled. The listener checks that before it publishes, and this
/// re-checks it before a successful handle is returned; neither check awaits
/// between observing the shutdown and acting, so a cancellation can only land
/// before the listener's check (reported by the listener) or after this one (a
/// normal post-startup shutdown), never inside the report itself.
fn readiness_failure(readiness: ListenerReadiness, shutdown_signalled: bool) -> Option<String> {
    match readiness {
        Ok(Ok(Ok(()))) if !shutdown_signalled => None,
        Ok(Ok(Ok(()))) => {
            Some("daemon startup was cancelled before it reported readiness".to_owned())
        }
        Ok(Ok(Err(reason))) => Some(reason),
        Ok(Err(_dropped)) => {
            Some("daemon pipe listener stopped before it bound its transport".to_owned())
        }
        Err(_elapsed) => Some("daemon pipe listener did not bind its transport in time".to_owned()),
    }
}

/// A client handle to the canonical Relay Daemon, whether reached in-process
/// (test harnesses, or this process embedding the daemon itself) or over the
/// real local transport. `DaemonClient`'s methods use native `async fn`
/// (RPITIT), which is not `dyn`-safe, so this enum is the minimal way to give
/// callers one concrete type regardless of which transport won.
pub enum DaemonClientHandle {
    InProcess(InProcessDaemonClient),
    Pipe(NamedPipeDaemonClient),
}

impl DaemonClient for DaemonClientHandle {
    async fn handshake(&self, client_name: &str) -> Result<(String, i64), DaemonError> {
        match self {
            Self::InProcess(c) => c.handshake(client_name).await,
            Self::Pipe(c) => c.handshake(client_name).await,
        }
    }

    async fn get_snapshot(&self) -> Result<DaemonSnapshot, DaemonError> {
        match self {
            Self::InProcess(c) => c.get_snapshot().await,
            Self::Pipe(c) => c.get_snapshot().await,
        }
    }

    async fn get_events_after(
        &self,
        after_sequence: i64,
        limit: i64,
    ) -> Result<Vec<DaemonEvent>, DaemonError> {
        match self {
            Self::InProcess(c) => c.get_events_after(after_sequence, limit).await,
            Self::Pipe(c) => c.get_events_after(after_sequence, limit).await,
        }
    }

    async fn submit_command(
        &self,
        command_id: &str,
        command: DaemonCommand,
    ) -> Result<CommandStatus, DaemonError> {
        match self {
            Self::InProcess(c) => c.submit_command(command_id, command).await,
            Self::Pipe(c) => c.submit_command(command_id, command).await,
        }
    }

    async fn ping(&self) -> Result<(), DaemonError> {
        match self {
            Self::InProcess(c) => c.ping().await,
            Self::Pipe(c) => c.ping().await,
        }
    }
}

/// A running embedded daemon (this process is acting as the daemon). Holds
/// the background task handles and a shutdown signal; dropping or calling
/// [`EmbeddedDaemon::shutdown`] stops the heartbeat, pipe listener, effect
/// worker, and (when one is attached) the v2 workflow driver cleanly.
pub struct EmbeddedDaemon {
    server: Arc<DaemonServer>,
    shutdown_tx: watch::Sender<bool>,
    /// Receives `true` when this daemon stops: an explicit
    /// [`EmbeddedDaemon::shutdown`], or loss of the daemon lease.
    shutdown: watch::Receiver<bool>,
    heartbeat: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    effect_worker: tokio::task::JoinHandle<()>,
    workflow_driver: Option<tokio::task::JoinHandle<()>>,
}

impl EmbeddedDaemon {
    /// Resolves once this daemon stops, whether because the host shut it down or
    /// because it lost authoritative ownership of the daemon lease.
    ///
    /// Long-running hosts should await this alongside their own shutdown signal so
    /// a daemon that has lost its lease exits instead of idling with no authority
    /// while a successor owns the database.
    pub async fn wait_for_shutdown(&mut self) {
        while !*self.shutdown.borrow_and_update() {
            if self.shutdown.changed().await.is_err() {
                return;
            }
        }
    }

    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = tokio::join!(self.heartbeat, self.listener, self.effect_worker);
        // The driver must stop before the lease is released: after release its
        // writes would be refused as authority-lost, which is a refusal it
        // should never have to observe during an orderly shutdown.
        if let Some(driver) = self.workflow_driver {
            let _ = driver.await;
        }
        // Release the lease only after the heartbeat loop has fully stopped,
        // so nothing can renew the row right after this deletes it - a
        // subsequent process does not have to wait out the full TTL before
        // it can become the daemon.
        if let Err(err) = self.server.release_lease().await {
            tracing::warn!(error = %err, "failed to release daemon lease on shutdown");
        }
    }
}

/// The trusted execution context one approved workflow run executes in.
///
/// `working_directory` comes from the registered project, never from task
/// input, matching the ingress rule that remote tasks cannot supply paths.
#[derive(Debug, Clone)]
pub struct WorkflowDriverContext {
    pub project_id: crate::ProjectId,
    pub checkout_id: crate::CheckoutId,
    pub working_directory: std::path::PathBuf,
    pub execution_target: crate::ExecutionTarget,
    pub budget: crate::ExecutionBudget,
}

pub type WorkflowRunContextFuture<'a> =
    Pin<Box<dyn Future<Output = Result<WorkflowDriverContext, String>> + Send + 'a>>;

/// Resolves the trusted execution context for an approved workflow run.
///
/// The host supplies this because only the host knows how a frozen
/// `ConfigSnapshot` maps onto a registered project and a concrete provider
/// target. The driver never invents a workspace.
pub trait WorkflowRunContextResolver: Send + Sync {
    fn resolve<'a>(&'a self, run: &'a crate::WorkflowRun) -> WorkflowRunContextFuture<'a>;
}

/// Drives approved v2 workflow runs until shutdown.
///
/// Owns the orchestration loop for every active `WorkflowRun`: on each wakeup it
/// re-scans durable state and hands each active run to the orchestrator. The
/// daemon never executes roles inline, so a slow or blocked run cannot stall
/// command handling, and a crash mid-run leaves the run durable for the next
/// driver to reconcile.
///
/// Every write is fenced on this daemon instance's lease. Losing authority is
/// not a run failure: the successor owns the run now, so the driver stops
/// instead of recording a false failure against it.
pub async fn run_workflow_driver<B, R>(
    server: Arc<DaemonServer>,
    orchestrator: Arc<crate::OrchestratorV2<B>>,
    resolver: Arc<R>,
    mut shutdown: watch::Receiver<bool>,
) where
    B: relay_runtime::RuntimeBackplane + 'static,
    R: WorkflowRunContextResolver + 'static,
{
    // Anything already active from a previous process is reconciled before the
    // first drive, so a restarted daemon resumes rather than duplicates.
    if let Err(err) = orchestrator.reconcile_on_startup(&server.fence()).await {
        if err.is_authority_lost() {
            tracing::warn!(
                error = %err,
                "workflow driver lost daemon authority before its first drive; stopping"
            );
            return;
        }
        tracing::error!(error = %err, "workflow reconcile on startup failed");
    }

    let wakeup = server.workflow_wakeup();
    let store = server.store().clone();

    loop {
        // Re-read authority each iteration: the fence is only proof for the
        // daemon instance that acquired it, and a driver that outlived its
        // authority must be refused at its next write, not race the successor.
        let fence = server.fence();

        match crate::durable_state::list_active_workflow_runs(&store).await {
            Ok(runs) => {
                for run in runs {
                    if *shutdown.borrow() {
                        return;
                    }

                    let context = match resolver.resolve(&run).await {
                        Ok(context) => context,
                        Err(reason) => {
                            let failure = format!("execution context resolution failed: {reason}");
                            tracing::warn!(
                                run = %run.id.0,
                                error = %failure,
                                "workflow run cannot resolve a trusted execution context"
                            );
                            if let Err(err) = fail_workflow_run(
                                &store,
                                &run,
                                &fence,
                                "execution_context",
                                &failure,
                            )
                            .await
                            {
                                if matches!(err, StoreError::DaemonAuthorityLost { .. }) {
                                    tracing::warn!(
                                        error = %err,
                                        "workflow driver lost daemon authority while recording context failure; stopping"
                                    );
                                    return;
                                }
                                tracing::error!(
                                    run = %run.id.0,
                                    error = %err,
                                    "failed to persist workflow context failure"
                                );
                            }
                            continue;
                        }
                    };

                    let driven = orchestrator
                        .run_compatibility_workflow(
                            &run.id,
                            &context.project_id,
                            &context.checkout_id,
                            &context.working_directory,
                            &context.execution_target,
                            &context.budget,
                            &fence,
                        )
                        .await;

                    match driven {
                        Ok(crate::WorkflowRunState::Completed) => {
                            // A completed run publishes the task's user-facing
                            // state, so the handoff the human approved does not
                            // stay 'running' after its run has finished. The
                            // transition is fenced like every other driver write.
                            let task_id = run.task_id.to_string();
                            match store
                                .update_task_state_fenced(
                                    task_id.as_str(),
                                    "running",
                                    "done",
                                    &fence,
                                )
                                .await
                            {
                                Ok(_) => {}
                                // A replay of a completion that already
                                // published `done` converges instead of failing.
                                Err(StoreError::StateConflict { actual_state, .. })
                                    if actual_state == "done" => {}
                                Err(err) => {
                                    tracing::error!(
                                        run = %run.id.0,
                                        error = %err,
                                        "failed to publish completed task state"
                                    );
                                }
                            }
                        }
                        Ok(crate::WorkflowRunState::Failed) => {
                            if let Err(err) = fail_workflow_run(
                                &store,
                                &run,
                                &fence,
                                "workflow_execution",
                                "workflow orchestrator returned a failed state",
                            )
                            .await
                            {
                                if matches!(err, StoreError::DaemonAuthorityLost { .. }) {
                                    tracing::warn!(
                                        error = %err,
                                        "workflow driver lost daemon authority while publishing failure; stopping"
                                    );
                                    return;
                                }
                                tracing::error!(
                                    run = %run.id.0,
                                    error = %err,
                                    "failed to publish workflow failure"
                                );
                            }
                        }
                        Ok(_) => {}
                        Err(err) if err.is_authority_lost() => {
                            tracing::warn!(
                                run = %run.id.0,
                                error = %err,
                                "workflow driver lost daemon authority; stopping instead of recording a false failure"
                            );
                            return;
                        }
                        Err(err) => {
                            // A provider/orchestration failure is terminal for this
                            // compatibility run; leave a durable typed state and
                            // user-facing task failure instead of retrying forever.
                            tracing::warn!(
                                run = %run.id.0,
                                error = %err,
                                "workflow drive attempt failed"
                            );
                            if let Err(persist_err) = fail_workflow_run(
                                &store,
                                &run,
                                &fence,
                                "workflow_execution",
                                &err.to_string(),
                            )
                            .await
                            {
                                if matches!(persist_err, StoreError::DaemonAuthorityLost { .. }) {
                                    tracing::warn!(
                                        error = %persist_err,
                                        "workflow driver lost daemon authority while recording execution failure; stopping"
                                    );
                                    return;
                                }
                                tracing::error!(
                                    run = %run.id.0,
                                    error = %persist_err,
                                    "failed to record workflow drive failure"
                                );
                            }
                        }
                    }
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to list active workflow runs");
            }
        }

        tokio::select! {
            _ = wakeup.notified() => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

async fn fail_workflow_run(
    store: &SqliteStore,
    run: &crate::WorkflowRun,
    fence: &relay_store::DaemonFence,
    failure_kind: &str,
    reason: &str,
) -> Result<(), StoreError> {
    store
        .update_workflow_run_state_fenced(&run.id.0, "failed", fence)
        .await?;

    let task_id = run.task_id.to_string();
    match store
        .update_task_state_fenced(&task_id, "running", "failed", fence)
        .await
    {
        Ok(_) => {}
        Err(StoreError::StateConflict { actual_state, .. }) if actual_state == "failed" => {}
        Err(error) => return Err(error),
    }

    let payload = serde_json::json!({
        "workflow_run_id": run.id.0,
        "failure_kind": failure_kind,
        "error": reason,
        "state": "failed",
    })
    .to_string();
    store
        .append_event_fenced(
            Some(task_id.as_str()),
            None,
            "workflow_drive_failed",
            Some(&payload),
            fence,
        )
        .await?;
    Ok(())
}

/// Starts a full daemon: acquires the exclusive lease, runs startup command
/// reconciliation once, then spawns the heartbeat loop, the named pipe
/// request listener, and the effect worker under one shutdown signal.
///
/// The transport address is derived from the durable identity of the database
/// this daemon serves, so a client configured for a different database looks
/// somewhere else entirely.
///
/// No v2 workflow driver is attached: approving a Run through this daemon
/// records a durable workflow run and leaves it for a driver to pick up.
pub async fn run_full_daemon(
    store: SqliteStore,
    relay: Relay,
    lease_id: &str,
    lease_ttl: Duration,
) -> Result<(Arc<DaemonServer>, EmbeddedDaemon), DaemonError> {
    let (server, handles) = start_daemon_tasks(store, relay, lease_id, lease_ttl).await?;
    Ok((server, handles.into_embedded(None)))
}

/// Starts a full daemon and attaches the v2 workflow driver.
///
/// Separate from [`run_full_daemon`] because the driver is generic over the
/// runtime backplane while the daemon itself is not: this is the seam where a
/// host composes a concrete backplane with the orchestration loop.
#[allow(clippy::too_many_arguments)]
pub async fn run_full_daemon_with_workflow_driver<B, R>(
    store: SqliteStore,
    relay: Relay,
    lease_id: &str,
    lease_ttl: Duration,
    artifact_root: impl Into<std::path::PathBuf>,
    backplane: Arc<B>,
    resolver: Arc<R>,
    provider_adapters: Vec<Arc<dyn relay_runtime::ProviderAdapter>>,
) -> Result<(Arc<DaemonServer>, EmbeddedDaemon), DaemonError>
where
    B: relay_runtime::RuntimeBackplane + 'static,
    R: WorkflowRunContextResolver + 'static,
{
    let orchestrator = Arc::new(crate::OrchestratorV2::new(
        store.clone(),
        Arc::new(crate::SchedulerV2::new(crate::SchedulerV2Config::default())),
        backplane,
        crate::ArtifactStore::new(artifact_root)
            .map_err(|err| DaemonError::Command(err.to_string()))?,
        provider_adapters,
    ));

    let (server, handles) = start_daemon_tasks(store, relay, lease_id, lease_ttl).await?;

    let driver = {
        let driver_server = Arc::clone(&server);
        let driver_shutdown = handles.shutdown_rx();
        tokio::spawn(async move {
            run_workflow_driver(driver_server, orchestrator, resolver, driver_shutdown).await;
        })
    };

    Ok((server, handles.into_embedded(Some(driver))))
}

/// Background tasks that make an embedded daemon a daemon, before the optional
/// workflow driver is attached to them.
struct DaemonTaskHandles {
    server: Arc<DaemonServer>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    heartbeat: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    effect_worker: tokio::task::JoinHandle<()>,
}

impl DaemonTaskHandles {
    fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    fn into_embedded(self, workflow_driver: Option<tokio::task::JoinHandle<()>>) -> EmbeddedDaemon {
        EmbeddedDaemon {
            server: self.server,
            shutdown_tx: self.shutdown_tx,
            shutdown: self.shutdown_rx,
            heartbeat: self.heartbeat,
            listener: self.listener,
            effect_worker: self.effect_worker,
            workflow_driver,
        }
    }
}

async fn start_daemon_tasks(
    store: SqliteStore,
    relay: Relay,
    lease_id: &str,
    lease_ttl: Duration,
) -> Result<(Arc<DaemonServer>, DaemonTaskHandles), DaemonError> {
    let daemon = DaemonServer::start(store, relay, lease_id, lease_ttl).await?;

    if let Err(err) = daemon.reconcile_on_startup().await {
        // Startup failed after the lease was acquired but before anything was
        // spawned, so there is nothing to join; release the lease this process
        // still owns so a successor does not have to wait out the TTL.
        if let Err(release_err) = daemon.release_lease().await {
            tracing::warn!(
                error = %release_err,
                "failed to release the daemon lease after a failed startup"
            );
        }
        return Err(err);
    }

    let server = Arc::new(daemon);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let heartbeat = tokio::spawn(run_heartbeat_loop(
        Arc::clone(&server),
        lease_ttl,
        shutdown_tx.clone(),
        shutdown_rx.clone(),
    ));

    let listener_server = Arc::clone(&server);
    let listener_pipe_name = daemon_transport::default_pipe_name(server.state_identity());
    let listener_shutdown = shutdown_rx.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let listener = tokio::spawn(async move {
        if let Err(err) = daemon_transport::run_named_pipe_listener(
            listener_server,
            &listener_pipe_name,
            listener_shutdown,
            ready_tx,
        )
        .await
        {
            tracing::error!(error = %err, "daemon pipe listener stopped with an error");
        }
    });

    // Do not report a running daemon until its transport is actually bound: a
    // successor that claimed the lease while a stale daemon still held the pipe
    // must wait out the takeover (bounded) rather than return a daemon with no
    // listener. A dropped `ready` means the listener could not bind at all, and
    // an `Err` payload means startup was cancelled before it bound - neither is
    // reported as readiness.
    let readiness = tokio::time::timeout(LISTENER_READY_TIMEOUT, ready_rx).await;
    let startup_failure = readiness_failure(readiness, *shutdown_rx.borrow());

    if let Some(reason) = startup_failure {
        // Startup failed after the lease was acquired. Tear the whole startup
        // down - stop and join the heartbeat as well as the listener - and
        // release the lease this process still owns, so a successor does not
        // have to wait out the TTL to take over. `release_daemon_lease` is
        // scoped to this daemon instance, so it can never delete a successor's row.
        let _ = shutdown_tx.send(true);
        let _ = tokio::join!(heartbeat, listener);
        if let Err(err) = server.release_lease().await {
            tracing::warn!(
                error = %err,
                "failed to release the daemon lease after a failed startup"
            );
        }
        return Err(DaemonError::Transport(reason));
    }

    let worker_store = server.store().clone();
    let effect_worker = tokio::spawn(run_effect_worker(
        worker_store,
        server.fence(),
        EffectWorkerRegistry::new(),
        Duration::from_secs(5),
        5,
        shutdown_rx.clone(),
    ));

    Ok((
        Arc::clone(&server),
        DaemonTaskHandles {
            server,
            shutdown_tx,
            shutdown_rx,
            heartbeat,
            listener,
            effect_worker,
        },
    ))
}

async fn run_heartbeat_loop(
    server: Arc<DaemonServer>,
    ttl: Duration,
    shutdown_tx: watch::Sender<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let heartbeat_interval = (ttl / 3).max(Duration::from_secs(1));
    let mut interval = tokio::time::interval(heartbeat_interval);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(err) = server.heartbeat(ttl).await {
                    // Losing the lease means a successor may already own this
                    // database. Stop claiming authority right away: every request
                    // fails closed, and the listener and effect worker shut down so
                    // the successor can bind the transport.
                    tracing::error!(
                        error = %err,
                        "lost the daemon lease; stopping the daemon instead of serving stale state"
                    );
                    server.mark_authority_lost(format!("daemon lease renewal failed: {err}"));
                    let _ = shutdown_tx.send(true);
                    return;
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// Generates a fresh command id for a client submitting a `DaemonCommand`.
pub fn new_command_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Connects to an already-running daemon over the local pipe; if none is
/// running, embeds a daemon in this process instead. Returns the client
/// handle and, when this process became the daemon, the embedded daemon
/// guard (the caller keeps it alive for the process lifetime and shuts it
/// down on exit).
pub async fn connect_or_embed(
    store: SqliteStore,
    relay: Relay,
    lease_id: &str,
    lease_ttl: Duration,
) -> Result<(DaemonClientHandle, Option<EmbeddedDaemon>), DaemonError> {
    let state_identity = store.store_identity().await?;
    let pipe_client = NamedPipeDaemonClient::new(
        daemon_transport::default_pipe_name(&state_identity),
        state_identity,
    );
    match pipe_client.ping().await {
        Ok(()) => return Ok((DaemonClientHandle::Pipe(pipe_client), None)),
        Err(DaemonError::NotRunning) => {}
        Err(other) => return Err(other),
    }

    match run_full_daemon(store, relay, lease_id, lease_ttl).await {
        Ok((server, embedded)) => Ok((
            DaemonClientHandle::InProcess(InProcessDaemonClient::new(server)),
            Some(embedded),
        )),
        Err(DaemonError::LeaseHeld(_)) => {
            // Another process won the race to become the daemon between our
            // ping and our attempt to start one. Give it a moment to finish
            // opening its pipe, then connect as a client.
            const RETRY_ATTEMPTS: u32 = 10;
            for attempt in 1..=RETRY_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(50 * u64::from(attempt))).await;
                match pipe_client.ping().await {
                    Ok(()) => return Ok((DaemonClientHandle::Pipe(pipe_client), None)),
                    Err(DaemonError::NotRunning) => continue,
                    Err(other) => return Err(other),
                }
            }
            Err(DaemonError::Transport(
                "another Relay process is starting the daemon; try again".to_owned(),
            ))
        }
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FLOW-19-03: a readiness signal published after the shared shutdown was
    /// already signalled is not readiness. The host re-checks that state before it
    /// returns a successful handle, so a startup cancelled after the pipe was
    /// created cannot surface as a daemon whose listener is about to exit.
    #[test]
    fn a_readiness_signal_under_a_cancelled_startup_is_not_readiness() {
        assert_eq!(
            readiness_failure(Ok(Ok(Ok(()))), false),
            None,
            "a bound transport under a live startup is readiness"
        );

        let cancelled = readiness_failure(Ok(Ok(Ok(()))), true).expect(
            "readiness published after a cancellation must not be reported as a running daemon",
        );
        assert!(
            cancelled.contains("cancelled"),
            "unexpected startup failure reason: {cancelled}"
        );

        // The listener's own cancellation reason is preserved rather than replaced.
        let reason = readiness_failure(Ok(Ok(Err("listener cancelled".to_owned()))), false)
            .expect("a cancelled listener must not be reported as readiness");
        assert_eq!(reason, "listener cancelled");
    }
}
