//! Process-boundary tests for `relay serve-mcp`.
//!
//! Each test runs the real binary against an isolated config/database so it
//! never touches the developer's local Relay state, and exercises the
//! JSON-RPC framing behavior visible from outside the process.

use std::future::Future;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::Receiver,
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

fn unique_temp_prefix(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("relay-serve-mcp-{name}-{nanos}"))
}

/// Named pipes are a single OS-wide namespace, unlike per-test temp
/// directories, so every spawned `relay` process must get a pipe name
/// derived from its own isolated database path. Two sequential invocations
/// against the *same* database path (deliberately, in the lifecycle test)
/// get the same derived name, which is correct: each process only embeds a
/// daemon for its own lifetime, never overlapping. Only a bare suffix
/// crosses the environment variable boundary (never a full `\\.\pipe\...`
/// path) - see `default_pipe_name`'s doc comment in `relay_core`.
fn pipe_suffix_for(database_path: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    database_path.hash(&mut hasher);
    format!("relay-test-{:x}", hasher.finish())
}

fn spawn_relay_isolated(name: &str) -> Child {
    let prefix = unique_temp_prefix(name);
    let database_path = prefix.join("state.sqlite3");
    let mut command = Command::new(env!("CARGO_BIN_EXE_relay"));
    command
        .args(["serve-mcp"])
        .env("RELAY_CONFIG_PATH", prefix.join("config.toml"))
        .env("RELAY_DAEMON_PIPE_SUFFIX", pipe_suffix_for(&database_path))
        .env("RELAY_DATABASE_PATH", database_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().expect("failed to spawn relay binary")
}

#[test]
fn stdin_closed_immediately_exits_zero_with_empty_stdout() {
    let mut child = spawn_relay_isolated("eof");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("failed to wait on relay");

    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
    assert!(
        output.stdout.is_empty(),
        "stdout must stay protocol-clean: {:?}",
        output.stdout
    );
}

#[test]
fn piped_lines_get_one_response_each_then_exit_zero() {
    const LINES: usize = 16;

    let mut child = spawn_relay_isolated("piped");
    {
        let mut stdin = child
            .stdin
            .take()
            .expect("serve-mcp child must have piped stdin");
        for index in 0..LINES {
            writeln!(stdin, "{{\"line\":{index}}}").expect("failed writing to serve-mcp stdin");
        }
    }
    let output = child.wait_with_output().expect("failed to wait on relay");

    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let responses: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("every response line is valid JSON"))
        .collect();

    assert_eq!(responses.len(), LINES);
    for response in responses {
        assert_eq!(response["jsonrpc"], "2.0");
        // `{"line":N}` has no method/id: a structured invalid-request error.
        assert_eq!(response["error"]["code"], -32600);
    }
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(["frobnicate"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn relay binary");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("failed to wait on relay");

    assert_ne!(output.status.code(), Some(0));
    assert!(!output.stderr.is_empty());
}

#[test]
fn unknown_flag_exits_nonzero() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(["--bogus"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn relay binary");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("failed to wait on relay");

    assert_ne!(output.status.code(), Some(0));
    assert!(!output.stderr.is_empty());
}

// ---------------------------------------------------------------------------
// M9 Phase 04 conformance: interactive request/response sessions against the
// real binary, including durable-state verification through a direct store.
// ---------------------------------------------------------------------------

const CONFORMANCE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// One canonical HandoffTask v1 payload valid against the isolated config's
/// registered `relay` project.
const CONFORMANCE_TASK_JSON: &str = r#"{"protocol_version":1,"project":"relay","title":"Wire conformance handoff","source":{"kind":"conversational_ai","provider":"openai"},"task":{"goal":"Prove the serve-mcp wire transport.","requirements":["One task per call."],"constraints":["Never auto-run."],"verification":["Row is Ready in SQLite."]},"ambiguity_policy":"needs_clarification"}"#;

fn make_isolated_paths(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let prefix = unique_temp_prefix(name);
    let project_dir = prefix.join("project");
    std::fs::create_dir_all(&project_dir).expect("create temp project dir");
    let config_path = prefix.join("config.toml");
    // TOML literal strings ('...') keep Windows backslashes verbatim.
    let config = format!(
        "[projects.relay]\nname = \"Relay\"\npath = '{}'\ndefault_stack = \"chatgpt-codex\"\n",
        project_dir.display()
    );
    std::fs::write(&config_path, config).expect("write isolated config.toml");
    let database_path = prefix.join("state.sqlite3");
    (config_path, database_path)
}

struct InteractiveServer {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<String>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
}

fn spawn_interactive(
    config_path: &std::path::Path,
    database_path: &std::path::Path,
) -> InteractiveServer {
    let mut command = Command::new(env!("CARGO_BIN_EXE_relay"));
    command
        .args(["serve-mcp"])
        .env("RELAY_CONFIG_PATH", config_path)
        .env("RELAY_DAEMON_PIPE_SUFFIX", pipe_suffix_for(database_path))
        .env("RELAY_DATABASE_PATH", database_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("failed to spawn relay binary");

    let stdin = child
        .stdin
        .take()
        .expect("serve-mcp child must have piped stdin");
    let stdout = child
        .stdout
        .take()
        .expect("serve-mcp child must have piped stdout");
    let mut stderr = child
        .stderr
        .take()
        .expect("serve-mcp child must have piped stderr");

    let (sender, receiver) = std::sync::mpsc::channel();
    // Reader thread: forwards each stdout line so requests and responses can
    // interleave without deadlocking on pipe buffers.
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    // Drain stderr concurrently so diagnostics can never fill the pipe.
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    InteractiveServer {
        child,
        stdin: Some(stdin),
        responses: receiver,
        stderr_reader: Some(stderr_reader),
    }
}

impl InteractiveServer {
    fn send_line(&mut self, line: &str) {
        let stdin = self
            .stdin
            .as_mut()
            .expect("server stdin is still open for this phase of the test");
        writeln!(stdin, "{line}").expect("failed writing request to serve-mcp stdin");
        stdin.flush().expect("failed flushing serve-mcp stdin");
    }

    fn next_response(&self, context: &str) -> Value {
        let line = self
            .responses
            .recv_timeout(CONFORMANCE_RESPONSE_TIMEOUT)
            .unwrap_or_else(|error| panic!("timed out waiting for {context}: {error}"));
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("{context}: response line was not JSON: {error}"))
    }

    /// Sends a raw request line and asserts the response is a structured
    /// error with the expected code. `Some(id)` also asserts the echoed
    /// request id; parse errors legitimately respond with `"id":null`.
    fn expect_wire_error(
        &mut self,
        id: Option<i64>,
        raw_request: &str,
        expected_code: i64,
        context: &str,
    ) {
        self.send_line(raw_request);
        let response = self.next_response(context);
        match id {
            Some(id) => assert_eq!(response["id"], id, "{context}: response id mismatch"),
            None => assert!(
                response["id"].is_null(),
                "{context}: unparseable request must answer with null id"
            ),
        }
        let error = response
            .get("error")
            .unwrap_or_else(|| panic!("{context}: expected structured error, got: {response}"));
        assert_eq!(
            error["code"].as_i64(),
            Some(expected_code),
            "{context}: unexpected error code in {response}"
        );
        assert!(
            error["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{context}: error message must be present"
        );
    }

    /// Closes stdin (client disconnect) and asserts the process exits 0.
    /// Returns captured stderr for failure context.
    fn shutdown_expect_exit_zero(mut self, context: &str) -> Vec<u8> {
        drop(self.stdin.take());
        let status = self.child.wait().expect("failed waiting on serve-mcp exit");
        let stderr = self
            .stderr_reader
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        assert_eq!(
            status.code(),
            Some(0),
            "{context}: server must exit cleanly after client disconnect; stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
        stderr
    }
}

fn initialize_handshake(server: &mut InteractiveServer, protocol_version: &str, id: i64) {
    server.send_line(&format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"{protocol_version}","capabilities":{{}},"clientInfo":{{"name":"conformance-test","version":"0"}}}}}}"#
    ));
    let response = server.next_response("initialize handshake");
    assert_eq!(response["id"], id);
    assert_eq!(
        response["result"]["protocolVersion"], protocol_version,
        "server must echo the negotiated protocol version"
    );
    assert_eq!(response["result"]["serverInfo"]["name"], "relay");
    assert!(response["result"]["capabilities"]["tools"].is_object());

    // The initialized notification is silent; strict per-request pairing in
    // the callers asserts no stray response arrives for it.
    server.send_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
}

fn submit_handoff_call(id: i64, task_json: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"submit_handoff","arguments":{task_json}}}}}"#
    )
}

/// Submits one valid task over the wire and returns `(task_id, state)` from
/// the structured result.
fn submit_valid_task_over_the_wire(server: &mut InteractiveServer, id: i64) -> (String, String) {
    server.send_line(&submit_handoff_call(id, CONFORMANCE_TASK_JSON));
    let response = server.next_response("tools/call submit_handoff");
    assert_eq!(response["id"], id);
    assert!(
        response.get("error").is_none(),
        "valid submit must succeed, got: {response}"
    );
    let structured = &response["result"]["structuredContent"];
    let task_id = structured["task_id"]
        .as_str()
        .unwrap_or_else(|| panic!("structured result must carry task_id: {response}"))
        .to_owned();
    let state = structured["state"]
        .as_str()
        .unwrap_or_else(|| panic!("structured result must carry state: {response}"))
        .to_owned();
    (task_id, state)
}

fn block_on_store<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build single-threaded test runtime")
        .block_on(future)
}

#[test]
fn happy_path_over_the_wire_persists_ready_task_in_sqlite() {
    let (config_path, database_path) = make_isolated_paths("conformance-happy");
    let mut server = spawn_interactive(&config_path, &database_path);

    initialize_handshake(&mut server, "2025-06-18", 1);

    server.send_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let listing = server.next_response("tools/list");
    assert_eq!(listing["result"]["tools"][0]["name"], "submit_handoff");

    let (task_id_text, state) = submit_valid_task_over_the_wire(&mut server, 3);
    assert_eq!(state, "Ready", "successful submit lands the task Ready");
    let task_id =
        relay_core::TaskId::parse(&task_id_text).expect("result carries a well-formed TaskId");

    server.shutdown_expect_exit_zero("happy path");

    // Durability proof: reopen the SQLite database directly and verify the
    // task the wire response promised actually exists in Ready state.
    let store =
        block_on_store(relay_core::SqliteStore::open(&database_path)).expect("reopen database");
    let tasks = block_on_store(relay_core::list_stored_tasks(&store)).expect("list stored tasks");

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task_id);
    assert_eq!(tasks[0].state, relay_core::TaskState::Ready);
    assert_eq!(
        tasks[0].submission_transport.as_str(),
        "relay-mcp",
        "wire submissions carry relay-mcp provenance"
    );
}

#[test]
fn wire_error_cases_return_structured_errors_and_server_survives() {
    // JSON-RPC reserved codes exercised below.
    const PARSE_ERROR: i64 = -32700;
    const METHOD_NOT_FOUND: i64 = -32601;
    const UNKNOWN_PROJECT: i64 = -32001;
    const INVALID_HANDOFF: i64 = -32005;

    let (config_path, database_path) = make_isolated_paths("conformance-errors");
    let mut server = spawn_interactive(&config_path, &database_path);

    initialize_handshake(&mut server, "2025-03-26", 1);

    server.expect_wire_error(
        None,
        "{this is not json",
        PARSE_ERROR,
        "malformed JSON line",
    );

    server.expect_wire_error(
        Some(11),
        r#"{"jsonrpc":"2.0","id":11,"method":"tasks/run"}"#,
        METHOD_NOT_FOUND,
        "unknown method",
    );

    let unknown_field_payload = CONFORMANCE_TASK_JSON.replace(
        r#""title":"Wire conformance handoff","#,
        r#""title":"Wire conformance handoff","sneaky_field":true,"#,
    );
    server.expect_wire_error(
        Some(12),
        &submit_handoff_call(12, &unknown_field_payload),
        INVALID_HANDOFF,
        "invalid payload (unknown field)",
    );

    let ghost_project_payload =
        CONFORMANCE_TASK_JSON.replace(r#""project":"relay""#, r#""project":"ghost-project""#);
    server.expect_wire_error(
        Some(13),
        &submit_handoff_call(13, &ghost_project_payload),
        UNKNOWN_PROJECT,
        "unknown project",
    );

    // Survival proof in the SAME session: after every failure branch above,
    // a valid submit still succeeds end to end.
    let (task_id_text, state) = submit_valid_task_over_the_wire(&mut server, 14);
    assert_eq!(state, "Ready");

    server.shutdown_expect_exit_zero("error cases");

    // Exactly one task was persisted: none of the four failed submissions.
    let store =
        block_on_store(relay_core::SqliteStore::open(&database_path)).expect("reopen database");
    let tasks = block_on_store(relay_core::list_stored_tasks(&store)).expect("list stored tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id.to_string(), task_id_text);
}

/// One modern-era request line: required `_meta` protocol version attached.
fn modern_request(id: i64, method: &str, extra_params: serde_json::Value) -> String {
    let mut params = serde_json::json!({
        "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
    });
    if let (Some(object), Some(extra)) = (params.as_object_mut(), extra_params.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
}

#[test]
fn modern_stateless_session_discovers_and_submits_without_any_handshake() {
    let (config_path, database_path) = make_isolated_paths("conformance-modern");
    let mut server = spawn_interactive(&config_path, &database_path);

    // server/discover first — the stdio backward-compatibility probe.
    server.send_line(&modern_request(1, "server/discover", serde_json::json!({})));
    let discover = server.next_response("server/discover");
    assert_eq!(discover["id"], 1);
    let versions = discover["result"]["supportedVersions"]
        .as_array()
        .expect("discover result lists supportedVersions");
    assert_eq!(versions[0], "2026-07-28");
    assert!(
        versions.iter().any(|version| version == "2025-11-25"),
        "legacy baseline must be advertised for downgrade clients"
    );
    assert_eq!(
        discover["result"]["capabilities"]["tools"],
        serde_json::json!({})
    );
    assert_eq!(
        discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "relay"
    );

    // tools/list statelessly, with no initialize exchange whatsoever.
    server.send_line(&modern_request(2, "tools/list", serde_json::json!({})));
    let listing = server.next_response("modern tools/list");
    assert_eq!(listing["result"]["resultType"], "complete");
    assert_eq!(listing["result"]["tools"][0]["name"], "submit_handoff");

    // Submit the conformance task through the modern path.
    let modern_call = modern_request(
        3,
        "tools/call",
        serde_json::json!({
            "name": "submit_handoff",
            "arguments": serde_json::from_str::<serde_json::Value>(CONFORMANCE_TASK_JSON)
                .expect("conformance task is valid JSON"),
        }),
    );
    server.send_line(&modern_call);
    let response = server.next_response("modern tools/call submit_handoff");
    assert_eq!(response["result"]["resultType"], "complete");
    assert_eq!(response["result"]["isError"], false);
    let structured = &response["result"]["structuredContent"];
    let task_id_text = structured["task_id"].as_str().expect("task_id present");
    assert_eq!(structured["state"], "Ready");

    server.shutdown_expect_exit_zero("modern session");

    // Durability proof identical to the legacy path.
    let store =
        block_on_store(relay_core::SqliteStore::open(&database_path)).expect("reopen database");
    let tasks = block_on_store(relay_core::list_stored_tasks(&store)).expect("list stored tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, relay_core::TaskState::Ready);
    assert_eq!(tasks[0].submission_transport.as_str(), "relay-mcp");
    assert_eq!(tasks[0].id.to_string(), task_id_text);
}

#[test]
fn oversized_frame_is_rejected_without_breaking_framing() {
    // The cap lives in the read loop (MAX_FRAME_BYTES = 2 * 512 KiB payload
    // cap + envelope headroom), so an over-limit line must produce exactly
    // one structured error and leave the stream aligned for the next message.
    const INVALID_REQUEST: i64 = -32600;

    let (config_path, database_path) = make_isolated_paths("conformance-frame-cap");
    let mut server = spawn_interactive(&config_path, &database_path);

    initialize_handshake(&mut server, "2025-06-18", 1);

    let padding = "x".repeat(1_200_000);
    server.send_line(&format!(r#"{{"j":"{padding}"}}"#));
    let error_response = server.next_response("oversized frame");
    assert_eq!(error_response["error"]["code"], INVALID_REQUEST);

    // Framing survived the drain: a normal request still round-trips.
    let (task_id_text, state) = submit_valid_task_over_the_wire(&mut server, 20);
    assert_eq!(state, "Ready");

    server.shutdown_expect_exit_zero("frame cap");

    let store =
        block_on_store(relay_core::SqliteStore::open(&database_path)).expect("reopen database");
    let tasks = block_on_store(relay_core::list_stored_tasks(&store)).expect("list stored tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id.to_string(), task_id_text);
}

#[test]
fn lifecycle_two_sequential_sessions_share_one_database() {
    let (config_path, database_path) = make_isolated_paths("conformance-lifecycle");
    let first_config = config_path.clone();
    let first_database = database_path.clone();

    // Session one: handshake plus a valid submission, then clean disconnect.
    let mut session_one = spawn_interactive(&first_config, &first_database);
    initialize_handshake(&mut session_one, "2025-06-18", 1);
    let (first_task_id, first_state) = submit_valid_task_over_the_wire(&mut session_one, 2);
    assert_eq!(first_state, "Ready");
    session_one.shutdown_expect_exit_zero("session one disconnect");

    // Session two against the SAME database: both sessions work.
    let mut session_two = spawn_interactive(&config_path, &database_path);
    initialize_handshake(&mut session_two, "2024-11-05", 1);
    let (second_task_id, second_state) = submit_valid_task_over_the_wire(&mut session_two, 2);
    assert_eq!(second_state, "Ready");
    session_two.shutdown_expect_exit_zero("session two disconnect");

    assert_ne!(
        first_task_id, second_task_id,
        "each submit gets its own TaskId"
    );

    let store =
        block_on_store(relay_core::SqliteStore::open(&database_path)).expect("reopen database");
    let tasks = block_on_store(relay_core::list_stored_tasks(&store)).expect("list stored tasks");
    assert_eq!(tasks.len(), 2, "both sessions' submissions persisted");
    assert!(
        tasks
            .iter()
            .all(|task| task.state == relay_core::TaskState::Ready)
    );
    let persisted_ids: Vec<String> = tasks.iter().map(|task| task.id.to_string()).collect();
    assert!(persisted_ids.contains(&first_task_id));
    assert!(persisted_ids.contains(&second_task_id));
}
