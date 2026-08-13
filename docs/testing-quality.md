# Testing and Code Quality

Relay controls source-modifying processes, so correctness at boundaries matters more than maximizing line coverage.

---

## Required Local Checks

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

---

## Test Layers

### Unit Tests

For:

- task protocol parsing;
- clarification/resolution parsing;
- state transitions;
- recovery-policy classification;
- project-key validation;
- capability logic;
- extension validation;
- remote normalization;
- work-disposition logic.

### Integration Tests

For:

- temporary Git repositories;
- expected-remote checks;
- fake runtime executables;
- stdout/stderr streaming;
- process cancellation;
- repair loops;
- SQLite migrations;
- ingress parsing;
- clarification publication;
- resolution ingestion;
- restart/recovery behavior.

### Contract Tests

Every runtime implementation should satisfy common runtime contract tests.

Every ingress implementation should satisfy common ingress contract tests.

### TUI Tests

Test important state/update behavior separately from rendering.

---

## Fake Runtimes

Build a test helper executable that can simulate:

- successful execution;
- failure exit;
- slow output;
- stderr;
- verification failure then repair success;
- repeated repair failure;
- `NEEDS_CLARIFICATION`;
- `NEEDS_DECISION`;
- cancellation;
- invalid output;
- huge output.

This allows recovery/clarification behavior to be tested without consuming coding-agent usage.

---

## Security Tests

Must include:

- unknown project rejection;
- path traversal attempts;
- malicious task strings;
- remote mismatch;
- unsupported extension rejection;
- shell metacharacters remaining inert task text;
- malicious resolution fields;
- repair-limit enforcement;
- concurrent project-lock behavior.

---

## Performance Expectations

Relay is not a compute-heavy application.

Benchmark:

- startup;
- task list load;
- repository preflight;
- large output handling;
- repair-loop event handling;
- SQLite history queries;
- TUI responsiveness.

Do not micro-optimize ordinary domain code without evidence.

---

## Code Review Standard

Prefer:

- explicit types;
- narrow interfaces;
- boring control flow;
- documented safety assumptions;
- tests at external boundaries.

Avoid clever abstractions before a second implementation proves the need.
