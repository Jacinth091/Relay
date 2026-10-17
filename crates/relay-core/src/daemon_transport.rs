//! Relay Architecture v2 - Wave A: local out-of-process daemon transport.
//!
//! The only supported host today is Windows/MSVC (see repository quality
//! gates), so this transport uses a Windows named pipe rather than a
//! cross-platform abstraction that nothing currently exercises. Framing is a
//! u32-LE length prefix followed by a JSON-encoded [`DaemonRequest`] or
//! [`DaemonResponse`].

#![cfg(windows)]

use std::{sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
    sync::watch,
};

use crate::daemon::{
    CommandStatus, DaemonClient, DaemonCommand, DaemonError, DaemonEvent, DaemonRequest,
    DaemonResponse, DaemonServer, DaemonSnapshot,
};

/// Maximum single-frame size accepted over the pipe. Requests/responses are
/// small typed commands and snapshots, never bulk data transfer.
const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

const ERROR_PIPE_BUSY: i32 = 231;
const ERROR_FILE_NOT_FOUND: i32 = 2;

/// The local pipe name for the daemon serving a given durable state identity.
///
/// The suffix is derived from the database's own identity (migration 0007) rather
/// than being a fixed string, so two differently configured processes cannot alias
/// onto each other's daemon by default: different databases produce different pipe
/// names. The variable part stays a bare suffix rather than a full pipe path
/// deliberately - some shells/process-launchers mangle a leading `\\` when it
/// round-trips through an environment variable value, so the `\\.\pipe\` prefix is
/// always constructed in-process instead of being transmitted.
///
/// `RELAY_DAEMON_PIPE_SUFFIX` overrides the derived suffix for test isolation and
/// manual experiments. It only changes *where* a client looks: the handshake
/// identity check in [`NamedPipeDaemonClient`] still refuses a daemon that is
/// serving different durable state, so a shared suffix cannot silently cross-write.
pub fn default_pipe_name(state_identity: &str) -> String {
    match std::env::var("RELAY_DAEMON_PIPE_SUFFIX") {
        Ok(suffix) if !suffix.is_empty() => format!(r"\\.\pipe\{suffix}"),
        _ => {
            let digest = crate::digest::sha256_digest(state_identity.as_bytes());
            format!(r"\\.\pipe\relay-daemon-{}", &digest[..16])
        }
    }
}

async fn write_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    bytes: &[u8],
) -> std::io::Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "frame too large to send")
    })?;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(bytes).await?;
    writer.flush().await
}

async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME_BYTES} byte limit"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Runs the named pipe accept loop, dispatching each connection's requests to
/// `server.handle_request`. Returns once `shutdown` is signalled.
///
/// Binds the first pipe instance before returning to the caller's control: the
/// `ready` signal fires exactly once with `Ok(())` when the listener owns its
/// transport, so the daemon host does not report a running daemon that has no
/// listener. If a stale predecessor still holds the pipe (for example during a
/// lease takeover), binding is retried within a bounded window instead of
/// failing immediately; if it still cannot bind, the listener returns an error
/// and `ready` is dropped unsent, which the host treats as a failed startup.
/// If startup is cancelled (shared shutdown) while the pipe is still held, the
/// listener reports that cancellation through `ready` rather than a successful
/// bind, so a cancelled startup can never be mistaken for a bound listener.
pub async fn run_named_pipe_listener(
    server: Arc<DaemonServer>,
    pipe_name: &str,
    mut shutdown: watch::Receiver<bool>,
    ready: tokio::sync::oneshot::Sender<Result<(), String>>,
) -> Result<(), DaemonError> {
    const BIND_RETRY_ATTEMPTS: u32 = 200;
    const BIND_RETRY_INTERVAL: Duration = Duration::from_millis(50);

    let mut ready = Some(ready);
    let mut first_instance = true;
    loop {
        let pipe_server = {
            let mut attempt = 0u32;
            loop {
                let mut options = ServerOptions::new();
                options.first_pipe_instance(first_instance);
                match options.create(pipe_name) {
                    Ok(pipe_server) => break pipe_server,
                    Err(err) => {
                        attempt += 1;
                        if attempt >= BIND_RETRY_ATTEMPTS {
                            return Err(DaemonError::Transport(format!(
                                "failed to create pipe instance '{pipe_name}' after {attempt} attempts: {err}"
                            )));
                        }
                        tracing::warn!(
                            attempt,
                            error = %err,
                            "daemon pipe is held by another instance; retrying to take over"
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(BIND_RETRY_INTERVAL) => {}
                            _ = shutdown.changed() => {
                                if *shutdown.borrow() {
                                    // Startup was cancelled while a predecessor
                                    // still held the pipe. Report the cancellation
                                    // instead of a successful bind: signalling
                                    // readiness here would let the host return a
                                    // "running" daemon that has no listener at all.
                                    if let Some(ready) = ready.take() {
                                        let _ = ready.send(Err(
                                            "daemon startup was cancelled before the pipe bound"
                                                .to_owned(),
                                        ));
                                    }
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        };
        first_instance = false;

        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }

        tokio::select! {
            connect_result = pipe_server.connect() => {
                connect_result.map_err(|err| {
                    DaemonError::Transport(format!("failed to accept pipe connection: {err}"))
                })?;
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    if let Err(err) = serve_connection(&server, pipe_server).await {
                        tracing::warn!(error = %err, "daemon pipe connection ended with an error");
                    }
                });
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

/// Serves one accepted pipe connection.
///
/// A connection is *not* trusted until it completes a matching-identity
/// handshake. Until then only a `Handshake` is accepted; every other request is
/// refused without being dispatched, so a raw peer that skips the handshake (or
/// fails it against the wrong database) cannot mutate state, and a handshake
/// against one identity cannot be replayed into commands for another.
async fn serve_connection(server: &DaemonServer, mut pipe: NamedPipeServer) -> std::io::Result<()> {
    let mut admitted = false;
    loop {
        let frame = match read_frame(&mut pipe).await {
            Ok(frame) => frame,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err),
        };

        let request: DaemonRequest = match serde_json::from_slice(&frame) {
            Ok(request) => request,
            Err(err) => {
                let response = DaemonResponse::Error(format!("malformed request: {err}"));
                let bytes = serde_json::to_vec(&response).unwrap_or_default();
                write_frame(&mut pipe, &bytes).await?;
                continue;
            }
        };

        let response = match &request {
            DaemonRequest::Handshake {
                protocol_version,
                state_identity,
                ..
            } => {
                let response = server.handshake_response(*protocol_version, state_identity);
                // Only a successful ack admits this connection; a refusal
                // (wrong version or wrong database) leaves it unadmitted, and a
                // later handshake can still be attempted on the same connection.
                admitted = matches!(response, DaemonResponse::HandshakeAck { .. });
                response
            }
            _ if !admitted => DaemonResponse::Error(
                "request rejected: the connection has not completed a matching identity handshake"
                    .to_owned(),
            ),
            _ => server.handle_request(request).await,
        };

        let bytes = serde_json::to_vec(&response).unwrap_or_else(|err| {
            serde_json::to_vec(&DaemonResponse::Error(err.to_string())).unwrap()
        });
        write_frame(&mut pipe, &bytes).await?;
    }
}

/// A [`DaemonClient`] backed by a real out-of-process named pipe connection.
/// Each call opens a fresh connection: at this command volume a persistent
/// connection would add concurrency complexity for no measurable benefit.
pub struct NamedPipeDaemonClient {
    pipe_name: String,
    expected_state_identity: String,
}

/// Client name used for the implicit per-call identity handshake.
const IMPLICIT_HANDSHAKE_CLIENT: &str = "relay-client";

impl NamedPipeDaemonClient {
    /// Creates a client for `pipe_name` that will only ever mutate durable state
    /// whose identity matches `expected_state_identity` (the database identity of
    /// the store this client was opened against).
    pub fn new(pipe_name: impl Into<String>, expected_state_identity: impl Into<String>) -> Self {
        Self {
            pipe_name: pipe_name.into(),
            expected_state_identity: expected_state_identity.into(),
        }
    }

    async fn connect(&self) -> Result<NamedPipeClient, DaemonError> {
        const MAX_ATTEMPTS: u32 = 10;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match ClientOptions::new().open(&self.pipe_name) {
                Ok(client) => return Ok(client),
                Err(err) if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
                    return Err(DaemonError::NotRunning);
                }
                Err(err)
                    if err.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempt < MAX_ATTEMPTS =>
                {
                    tokio::time::sleep(Duration::from_millis(20 * u64::from(attempt))).await;
                }
                Err(err) => {
                    return Err(DaemonError::Transport(format!(
                        "failed to connect to daemon pipe: {err}"
                    )));
                }
            }
        }
    }

    async fn call(&self, request: DaemonRequest) -> Result<DaemonResponse, DaemonError> {
        let (mut client, _ack) = self
            .connect_and_handshake(IMPLICIT_HANDSHAKE_CLIENT)
            .await?;
        let bytes = serde_json::to_vec(&request)?;
        write_frame(&mut client, &bytes)
            .await
            .map_err(|err| DaemonError::Transport(err.to_string()))?;
        let response_bytes = read_frame(&mut client)
            .await
            .map_err(|err| DaemonError::Transport(err.to_string()))?;
        let response: DaemonResponse = serde_json::from_slice(&response_bytes)?;
        Ok(response)
    }

    /// Opens a connection and verifies the daemon's durable state identity on it
    /// before the caller's request is sent.
    ///
    /// Every [`NamedPipeDaemonClient`] request goes through this, and every request
    /// opens its own connection, so a client cannot reach a daemon serving
    /// different durable state even once - a reachable pipe is never accepted as
    /// proof of matching identity.
    async fn connect_and_handshake(
        &self,
        client_name: &str,
    ) -> Result<(NamedPipeClient, DaemonResponse), DaemonError> {
        let mut client = self.connect().await?;

        let request = DaemonRequest::Handshake {
            client_name: client_name.to_owned(),
            protocol_version: crate::daemon::DAEMON_PROTOCOL_VERSION,
            state_identity: self.expected_state_identity.clone(),
        };
        let bytes = serde_json::to_vec(&request)?;
        write_frame(&mut client, &bytes)
            .await
            .map_err(|err| DaemonError::Transport(err.to_string()))?;
        let response_bytes = read_frame(&mut client)
            .await
            .map_err(|err| DaemonError::Transport(err.to_string()))?;
        let ack: DaemonResponse = serde_json::from_slice(&response_bytes)?;

        match &ack {
            DaemonResponse::HandshakeAck { state_identity, .. } => {
                if state_identity != &self.expected_state_identity {
                    return Err(DaemonError::StateIdentityMismatch {
                        expected: self.expected_state_identity.clone(),
                        actual: state_identity.clone(),
                    });
                }
                Ok((client, ack))
            }
            DaemonResponse::StateIdentityMismatch {
                daemon_identity, ..
            } => Err(DaemonError::StateIdentityMismatch {
                expected: self.expected_state_identity.clone(),
                actual: daemon_identity.clone(),
            }),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err.clone())),
            other => Err(DaemonError::Command(format!(
                "unexpected handshake response: {other:?}"
            ))),
        }
    }
}

impl DaemonClient for NamedPipeDaemonClient {
    async fn handshake(&self, client_name: &str) -> Result<(String, i64), DaemonError> {
        let (_client, ack) = self.connect_and_handshake(client_name).await?;
        match ack {
            DaemonResponse::HandshakeAck {
                daemon_version,
                fencing_token,
                ..
            } => Ok((daemon_version, fencing_token)),
            other => Err(DaemonError::Command(format!(
                "unexpected handshake response: {other:?}"
            ))),
        }
    }

    async fn get_snapshot(&self) -> Result<DaemonSnapshot, DaemonError> {
        match self.call(DaemonRequest::GetSnapshot).await? {
            DaemonResponse::Snapshot(snapshot) => Ok(snapshot),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected snapshot response".to_owned(),
            )),
        }
    }

    async fn get_events_after(
        &self,
        after_sequence: i64,
        limit: i64,
    ) -> Result<Vec<DaemonEvent>, DaemonError> {
        match self
            .call(DaemonRequest::GetEventsAfter {
                after_sequence,
                limit,
            })
            .await?
        {
            DaemonResponse::Events(events) => Ok(events),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected events response".to_owned(),
            )),
        }
    }

    async fn submit_command(
        &self,
        command_id: &str,
        command: DaemonCommand,
    ) -> Result<CommandStatus, DaemonError> {
        match self
            .call(DaemonRequest::SubmitCommand {
                command_id: command_id.to_owned(),
                command,
                state_identity: self.expected_state_identity.clone(),
            })
            .await?
        {
            DaemonResponse::CommandResult { status, .. } => Ok(status),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected command response".to_owned(),
            )),
        }
    }

    async fn ping(&self) -> Result<(), DaemonError> {
        match self.call(DaemonRequest::Ping).await? {
            DaemonResponse::Pong => Ok(()),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command("unexpected ping response".to_owned())),
        }
    }
}

/// Real named-pipe coverage for the R17-01 server-side admission gate.
///
/// These deliberately speak the raw framing protocol instead of going through
/// [`NamedPipeDaemonClient`]: the point is to prove what a peer that *skips* the
/// handshake (or fails it) can and cannot do, which the typed client - by
/// construction - never does.
#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use relay_store::SqliteStore;
    use tokio::sync::watch;

    use super::*;
    use crate::{
        Relay,
        daemon::{DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonServer},
    };

    const TEST_HANDOFF: &str = r#"{
        "protocol_version": 1,
        "project": "test-proj",
        "title": "Transport Admission Test",
        "task": { "goal": "Prove a raw peer cannot mutate without a handshake" },
        "ambiguity_policy": "needs_clarification"
    }"#;

    fn test_relay() -> Relay {
        let mut registry = crate::ProjectRegistry::empty();
        let path = std::env::temp_dir().join("relay-daemon-transport-test-project");
        std::fs::create_dir_all(&path).unwrap();
        registry
            .register(crate::RegisterProject {
                key: "test-proj".to_owned(),
                name: None,
                local_path: path,
                expected_remote: None,
                default_stack: None,
                github: None,
            })
            .unwrap();
        Relay::with_project_registry(registry)
    }

    struct Harness {
        pipe_name: String,
        state_identity: String,
        server: Arc<DaemonServer>,
        shutdown: watch::Sender<bool>,
        listener: tokio::task::JoinHandle<()>,
    }

    impl Harness {
        async fn start() -> Self {
            let store = SqliteStore::in_memory().await.unwrap();
            let state_identity = store.store_identity().await.unwrap();
            let server = Arc::new(
                DaemonServer::start(
                    store,
                    test_relay(),
                    "test-lease-admission",
                    Duration::from_secs(60),
                )
                .await
                .unwrap(),
            );

            let pipe_name = default_pipe_name(&state_identity);
            let (shutdown, shutdown_rx) = watch::channel(false);
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let listener = {
                let server = Arc::clone(&server);
                let pipe_name = pipe_name.clone();
                tokio::spawn(async move {
                    let _ =
                        run_named_pipe_listener(server, &pipe_name, shutdown_rx, ready_tx).await;
                })
            };
            ready_rx
                .await
                .expect("the test listener must report its bind outcome")
                .expect("the test listener must bind its pipe");

            Self {
                pipe_name,
                state_identity,
                server,
                shutdown,
                listener,
            }
        }

        async fn connect(&self) -> NamedPipeClient {
            for _ in 0..100 {
                match ClientOptions::new().open(&self.pipe_name) {
                    Ok(client) => return client,
                    Err(err)
                        if matches!(
                            err.raw_os_error(),
                            Some(ERROR_PIPE_BUSY) | Some(ERROR_FILE_NOT_FOUND)
                        ) =>
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(err) => panic!("failed to connect to the test pipe: {err}"),
                }
            }
            panic!("test pipe never became reachable");
        }

        async fn request(
            &self,
            client: &mut NamedPipeClient,
            request: &DaemonRequest,
        ) -> DaemonResponse {
            let bytes = serde_json::to_vec(request).unwrap();
            write_frame(client, &bytes).await.unwrap();
            let response_bytes = read_frame(client).await.unwrap();
            serde_json::from_slice(&response_bytes).unwrap()
        }

        async fn submit_handoff(&self, command_id: &str) -> DaemonRequest {
            DaemonRequest::SubmitCommand {
                command_id: command_id.to_owned(),
                command: DaemonCommand::SubmitHandoff {
                    project_key: "test-proj".to_owned(),
                    handoff_json: TEST_HANDOFF.to_owned(),
                    submission_transport: "local-paste".to_owned(),
                },
                state_identity: self.state_identity.clone(),
            }
        }

        async fn stop(self) {
            let _ = self.shutdown.send(true);
            let _ = self.listener.await;
        }
    }

    /// Counts durable command rows, so a refused request can be proven to have
    /// had no effect rather than merely having produced an error response.
    async fn recorded_command_count(server: &DaemonServer, command_id: &str) -> bool {
        server
            .store()
            .get_command(command_id)
            .await
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn raw_peer_without_a_handshake_cannot_submit_a_command() {
        let harness = Harness::start().await;
        let mut client = harness.connect().await;

        // No handshake: the very first frame is a mutating command.
        let request = harness.submit_handoff("cmd-no-handshake").await;
        match harness.request(&mut client, &request).await {
            DaemonResponse::Error(err) => {
                assert!(
                    err.contains("has not completed a matching identity handshake"),
                    "unexpected refusal message: {err}"
                );
            }
            other => panic!("expected an unadmitted connection to be refused, got {other:?}"),
        }

        assert!(
            !recorded_command_count(&harness.server, "cmd-no-handshake").await,
            "a refused command must not be recorded"
        );
        match harness
            .server
            .handle_request(DaemonRequest::GetSnapshot)
            .await
        {
            DaemonResponse::Snapshot(snapshot) => assert!(
                snapshot.tasks.is_empty(),
                "a refused command must not import a task"
            ),
            other => panic!("expected a snapshot, got {other:?}"),
        }

        harness.stop().await;
    }

    #[tokio::test]
    async fn mismatched_handshake_does_not_admit_the_connection() {
        let harness = Harness::start().await;
        let mut client = harness.connect().await;

        // Handshake against a *different* database's identity.
        let mismatch = DaemonRequest::Handshake {
            client_name: "raw-test".to_owned(),
            protocol_version: DAEMON_PROTOCOL_VERSION,
            state_identity: "some-other-database".to_owned(),
        };
        match harness.request(&mut client, &mismatch).await {
            DaemonResponse::StateIdentityMismatch { .. } => {}
            other => panic!("expected an identity mismatch, got {other:?}"),
        }

        // The connection must still be unadmitted.
        let request = harness.submit_handoff("cmd-wrong-handshake").await;
        match harness.request(&mut client, &request).await {
            DaemonResponse::Error(err) => {
                assert!(
                    err.contains("has not completed a matching identity handshake"),
                    "unexpected refusal message: {err}"
                );
            }
            other => panic!("expected an unadmitted connection to be refused, got {other:?}"),
        }

        assert!(
            !recorded_command_count(&harness.server, "cmd-wrong-handshake").await,
            "a refused command must not be recorded"
        );

        harness.stop().await;
    }

    #[tokio::test]
    async fn matching_handshake_admits_subsequent_commands() {
        let harness = Harness::start().await;
        let mut client = harness.connect().await;

        let handshake = DaemonRequest::Handshake {
            client_name: "raw-test".to_owned(),
            protocol_version: DAEMON_PROTOCOL_VERSION,
            state_identity: harness.state_identity.clone(),
        };
        match harness.request(&mut client, &handshake).await {
            DaemonResponse::HandshakeAck { .. } => {}
            other => panic!("expected a handshake ack, got {other:?}"),
        }

        let request = harness.submit_handoff("cmd-admitted").await;
        match harness.request(&mut client, &request).await {
            DaemonResponse::CommandResult { status, .. } => {
                assert_eq!(status, CommandStatus::Applied)
            }
            other => panic!("expected the admitted command to apply, got {other:?}"),
        }

        harness.stop().await;
    }

    /// R18-02: a startup that is cancelled while the pipe is still held by a
    /// predecessor must not report a bound listener. Previously this branch sent
    /// the *success* signal, which the host read as "the transport is bound" even
    /// though nothing had bound and authority had already been lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_startup_does_not_report_a_bound_listener() {
        let store = SqliteStore::in_memory().await.unwrap();
        let state_identity = store.store_identity().await.unwrap();
        let server = Arc::new(
            DaemonServer::start(
                store,
                test_relay(),
                "test-lease-cancelled",
                Duration::from_secs(60),
            )
            .await
            .unwrap(),
        );

        let pipe_name = default_pipe_name(&state_identity);
        // Hold the pipe for the whole test, so the listener cannot bind and has
        // to enter its bounded takeover retry loop.
        let blocker = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .expect("the test must be able to hold the pipe first");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let listener = tokio::spawn({
            let server = Arc::clone(&server);
            let pipe_name = pipe_name.clone();
            async move {
                let _ = run_named_pipe_listener(server, &pipe_name, shutdown_rx, ready_tx).await;
            }
        });

        // Let the listener reach its retry loop, then cancel startup, as a
        // heartbeat failure or host shutdown would.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = shutdown_tx.send(true);

        let outcome = ready_rx
            .await
            .expect("the listener must report a startup outcome");
        assert!(
            outcome.is_err(),
            "a cancelled startup must not report a bound listener"
        );

        let _ = listener.await;
        drop(blocker);
    }
}
