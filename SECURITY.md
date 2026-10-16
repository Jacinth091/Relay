# Security Policy

Relay is an **experimental hobby project**, not a hardened, audited, or production-ready security boundary. It can launch developer tools capable of modifying files and executing commands. Use it only with accounts, permissions, and repositories you are willing to expose to experimental software. Never disable sandbox restrictions merely to make Relay run.

## Reporting vulnerabilities privately

**Do not open a public issue** containing exploit steps, secrets, credentials, or non-public data. Use GitHub's private vulnerability reporting feature for this repository **once enabled by the maintainer** (the repository's **Security** tab, then **Report a vulnerability**).

If the private reporting button is not available, refrain from posting sensitive details publicly; contact the maintainer privately through an appropriate channel you already have. No response or fix timeline is guaranteed.

## Areas of concern

Relevant reports include arbitrary command execution, untrusted task/ingress escalation, path traversal, project routing bypass, cancellation/recovery boundary failures, privilege changes, malicious extensions, and credential leaks.

## Safety defaults

Do not place secrets in handoff payloads or public issues. Keep personal Relay mailbox repositories private. Work in an isolated, disposable Git repository first, use least-privilege accounts, and review actions taken by coding-agent runtimes.

Architecture context: [Security model](docs/security-model.md), [External process ADR](docs/adr/0005-external-process-extension-direction.md), and [Git/GitHub ADR](docs/adr/0007-cli-first-git-github.md).

This policy does not promise security support or coordinated disclosure response times.
