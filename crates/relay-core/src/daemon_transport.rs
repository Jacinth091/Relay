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

/// The default local pipe name. The variable part is a bare suffix rather
/// than a full pipe path deliberately: some shells/process-launchers mangle
/// a leading `\\` when it round-trips through an environment variable value,
/// so the `\\.\pipe\` prefix is always constructed in-process instead of
/// being transmitted. Mirrors the `RELAY_DATABASE_PATH` override convention
/// used elsewhere for test isolation.
pub fn default_pipe_name() -> String {
    let suffix =
        std::env::var("RELAY_DAEMON_PIPE_SUFFIX").unwrap_or_else(|_| "relay-daemon".to_owned());
    format!(r"\\.\pipe\{suffix}")
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
pub async fn run_named_pipe_listener(
    server: Arc<DaemonServer>,
    pipe_name: &str,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), DaemonError> {
    let mut first_instance = true;
    loop {
        let pipe_server = {
            let mut options = ServerOptions::new();
            options.first_pipe_instance(first_instance);
            options.create(pipe_name).map_err(|err| {
                DaemonError::Transport(format!("failed to create pipe instance: {err}"))
            })?
        };
        first_instance = false;

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

async fn serve_connection(server: &DaemonServer, mut pipe: NamedPipeServer) -> std::io::Result<()> {
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

        let response = server.handle_request(request).await;
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
}

impl NamedPipeDaemonClient {
    pub fn new(pipe_name: impl Into<String>) -> Self {
        Self {
            pipe_name: pipe_name.into(),
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
        let mut client = self.connect().await?;
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
}

impl DaemonClient for NamedPipeDaemonClient {
    async fn handshake(&self, client_name: &str) -> Result<(String, i64), DaemonError> {
        match self
            .call(DaemonRequest::Handshake {
                client_name: client_name.to_owned(),
                protocol_version: crate::daemon::DAEMON_PROTOCOL_VERSION,
            })
            .await?
        {
            DaemonResponse::HandshakeAck {
                daemon_version,
                fencing_token,
                ..
            } => Ok((daemon_version, fencing_token)),
            DaemonResponse::Error(err) => Err(DaemonError::Command(err)),
            _ => Err(DaemonError::Command(
                "unexpected handshake response".to_owned(),
            )),
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
