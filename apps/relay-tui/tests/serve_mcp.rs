use std::io::Write;
use std::process::{Child, Command, Stdio};

fn spawn_relay(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn relay binary")
}

#[test]
fn stdin_closed_immediately_exits_zero_with_empty_stdout() {
    let mut child = spawn_relay(&["serve-mcp"]);
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
fn piped_lines_are_consumed_then_exit_zero() {
    let mut child = spawn_relay(&["serve-mcp"]);
    {
        let mut stdin = child
            .stdin
            .take()
            .expect("serve-mcp child must have piped stdin");
        for index in 0..16 {
            writeln!(stdin, "{{\"line\":{index}}}").expect("failed writing to serve-mcp stdin");
        }
    }
    let output = child.wait_with_output().expect("failed to wait on relay");

    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
    assert!(output.stdout.is_empty());
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let mut child = spawn_relay(&["frobnicate"]);
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("failed to wait on relay");

    assert_ne!(output.status.code(), Some(0));
    assert!(!output.stderr.is_empty());
}

#[test]
fn unknown_flag_exits_nonzero() {
    let mut child = spawn_relay(&["--bogus"]);
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("failed to wait on relay");

    assert_ne!(output.status.code(), Some(0));
    assert!(!output.stderr.is_empty());
}
