//! Relay Architecture v2 - Wave A: daemon process lifecycle.
//!
//! Owns starting a full daemon (lease heartbeat + request listener + effect
//! worker + startup reconciliation) and the client-side logic for either
//! connecting to an already-running daemon or embedding one in-process when
//! none is running yet.

use std::{sync::Arc, time::Duration};

use tokio::sync::watch;

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
/// [`EmbeddedDaemon::shutdown`] stops the heartbeat, pipe listener, and
/// effect worker cleanly.
pub struct EmbeddedDaemon {
    server: Arc<DaemonServer>,
    shutdown_tx: watch::Sender<bool>,
    /// Receives `true` when this daemon stops: an explicit
    /// [`EmbeddedDaemon::shutdown`], or loss of the daemon lease.
    shutdown: watch::Receiver<bool>,
    heartbeat: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    effect_worker: tokio::task::JoinHandle<()>,
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
        // Release the lease only after the heartbeat loop has fully stopped,
        // so nothing can renew the row right after this deletes it - a
        // subsequent process does not have to wait out the full TTL before
        // it can become the daemon.
        if let Err(err) = self.server.release_lease().await {
            tracing::warn!(error = %err, "failed to release daemon lease on shutdown");
        }
    }
}

/// Starts a full daemon: acquires the exclusive lease, runs startup command
/// reconciliation once, then spawns the heartbeat loop, the named pipe
/// request listener, and the effect worker under one shutdown signal.
///
/// The transport address is derived from the durable identity of the database
/// this daemon serves, so a client configured for a different database looks
/// somewhere else entirely.
pub async fn run_full_daemon(
    store: SqliteStore,
    relay: Relay,
    lease_id: &str,
    lease_ttl: Duration,
) -> Result<(Arc<DaemonServer>, EmbeddedDaemon), DaemonError> {
    let daemon = DaemonServer::start(store, relay, lease_id, lease_ttl).await?;

    daemon.reconcile_on_startup().await?;

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
    let startup_failure = match tokio::time::timeout(LISTENER_READY_TIMEOUT, ready_rx).await {
        Ok(Ok(Ok(()))) => None,
        Ok(Ok(Err(reason))) => Some(reason),
        Ok(Err(_dropped)) => {
            Some("daemon pipe listener stopped before it bound its transport".to_owned())
        }
        Err(_elapsed) => Some("daemon pipe listener did not bind its transport in time".to_owned()),
    };

    if let Some(reason) = startup_failure {
        // Startup failed after the lease was acquired. Tear the whole startup
        // down - stop and join the heartbeat as well as the listener - and
        // release the lease this process still owns, so a successor does not
        // have to wait out the TTL to take over. `release_daemon_lease` is
        // scoped to our own holder pid, so it can never delete a successor's row.
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
        EffectWorkerRegistry::new(),
        Duration::from_secs(5),
        5,
        shutdown_rx.clone(),
    ));

    Ok((
        Arc::clone(&server),
        EmbeddedDaemon {
            server,
            shutdown_tx,
            shutdown: shutdown_rx,
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
