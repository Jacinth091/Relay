# Security Policy

Relay is an **experimental hobby project**. It is **not audited, hardened, or a
production security boundary**, and no security support or response time is
promised.

Relay can start developer tools and coding-agent runtimes that are able to
modify files and execute commands on your machine. Treat it as experimental
software:

- use accounts, permissions, and repositories you are willing to expose;
- start with a **disposable Git repository**;
- use least-privilege accounts and review what runtimes actually do;
- never disable sandbox or OS restrictions merely to make Relay run.

Submitting a handoff task does **not** start execution. Relay requires an
explicit human approval before any coding runtime is launched, and it never
pushes to a remote repository automatically.

## Reporting a vulnerability privately

**Do not open a public issue** that contains exploit steps, credentials, tokens,
private repository content, or non-public data.

Preferred channel: GitHub **private vulnerability reporting** for this
repository (the repository's **Security** tab, then **Report a vulnerability**).

If that button is unavailable, use a private channel you already share with the
maintainer rather than posting details publicly.

There is no obligation to publish a personal address, and no guaranteed fix,
disclosure, or response timeline.

## Areas of concern

Reports that are especially relevant to this project include:

- arbitrary command execution or command-injection through task or runtime input;
- path traversal, or task input that escapes trusted local project resolution;
- bypass of the trusted local project registry or repository remote checks;
- privilege or permission-grant escalation in the runtime permission broker;
- cancellation, recovery, or lease boundaries that allow conflicting writes;
- credential or secret leakage in logs, packets, or persisted state;
- unsafe behavior in an ingress, backplane, or planner adapter.

## Safety defaults to expect

- Tasks carry no filesystem paths; trusted workspace derives only from the local
  project registry.
- Unknown projects, unknown protocols, oversized inputs, and malformed payloads
  fail closed.
- `relay serve-mcp` (`submit_handoff`) only validates, resolves a trusted
  project, persists, and marks the task `READY`. It never auto-runs.
- Submitted `HandoffTask v1` payloads are treated as untrusted data.
- Runtime output is untrusted display/log content and is never executed as a shell
  command string.

## Related reading

- [Security model](docs/security-model.md)
- [ADR-0005: external process extension direction](docs/adr/0005-external-process-extension-direction.md)
- [ADR-0007: CLI-first Git/GitHub](docs/adr/0007-cli-first-git-github.md)
- [ADR-0020: Relay Architecture v2](docs/adr/0020-relay-architecture-v2.md)
