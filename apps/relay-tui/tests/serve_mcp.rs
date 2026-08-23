//! Process-boundary tests for `relay serve-mcp`.
//!
//! Each test runs the real binary against an isolated config/database so it
//! never touches the developer's local Relay state, and exercises the
//! JSON-RPC framing behavior visible from outside the process.

use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

fn unique_temp_prefix(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("relay-serve-mcp-{name}-{nanos}"))
}

fn spawn_relay_isolated(name: &str) -> Child {
    let prefix = unique_temp_prefix(name);
    let mut command = Command::new(env!("CARGO_BIN_EXE_relay"));
    command
        .args(["serve-mcp"])
        .env("RELAY_CONFIG_PATH", prefix.join("config.toml"))
        .env("RELAY_DATABASE_PATH", prefix.join("state.sqlite3"))
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
