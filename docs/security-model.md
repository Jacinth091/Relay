# Security Model

Relay launches coding tools that can modify local repositories.

Security is a core feature.

---

## Trust Boundaries

### Trusted

Local Relay configuration controlled by the user.

Examples:

- project path;
- expected Git remote;
- allowed stack;
- runtime executable policy;
- recovery limits;
- validation profile.

### Untrusted

Treat these as untrusted:

- remote task text;
- GitHub Issue contents;
- runtime stdout/stderr;
- provider-generated extension values;
- clarification packets produced by runtimes;
- repository file contents;
- community extensions.

A task being authored by an AI does not make it trusted.

---

## Project Routing Rule

Remote:

```json
{ "project": "practicore" }
```

Local:

```text
practicore -> D:\Projects\PractiCore
```

Only local configuration provides the filesystem mapping.

---

## Repository Preflight

Before execution:

1. project key exists;
2. configured directory exists;
3. directory is a Git repository;
4. expected remote is configured when policy requires it;
5. actual repository-local remote matches normalized expected remote;
6. runtime profile exists;
7. runtime is available;
8. task extensions are supported/allowed;
9. project is not locked by conflicting execution;
10. user confirms Run.

Repository preflight invokes `git` directly with explicit arguments, never
through a shell. Relay removes inherited Git repository-selection environment
such as `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, `GIT_INDEX_FILE`,
`GIT_OBJECT_DIRECTORY`, and `GIT_ALTERNATE_OBJECT_DIRECTORIES` before
inspection commands run. Relay also removes Git command-scope config injection
environment such as `GIT_CONFIG_COUNT`, `GIT_CONFIG_PARAMETERS`, and indexed
`GIT_CONFIG_KEY_N` / `GIT_CONFIG_VALUE_N` values.

Remote lookup reads repository-local configuration only:

```text
git -C <trusted-path> config --local --get remote.origin.url
```

Status inspection disables optional locking/index refresh and repository
FSMonitor hooks:

```text
git -C <trusted-path> --no-optional-locks -c core.fsmonitor=false status --porcelain=v1 --untracked-files=all
```

### Remote Normalization

Repository remote comparison uses a normalized identity of:

```text
host/repository-path
```

For v0.1, Relay recognizes these common equivalent forms:

```text
https://github.com/owner/repo.git
https://github.com/owner/repo
git@github.com:owner/repo.git
ssh://git@github.com/owner/repo.git
```

Relay lowercases hosts for all remotes. Relay lowercases repository paths only
for `github.com`; paths for other hosts remain case-sensitive because Relay
does not assume self-hosted Git repository identity rules. Relay strips a final
`.git` suffix and rejects malformed, ambiguous, or whitespace-containing remote
strings instead of guessing.

---

## Process Injection

Never build a shell string from untrusted task text.

Use direct process APIs with:

- fixed executable;
- explicit argument array;
- task body through stdin/file when possible;
- explicit working directory.

---

## Commands From Tasks

The remote protocol does not allow arbitrary preflight/postflight shell commands.

Validation profiles are local configuration.

---

## Recovery Safety

Automatic repair is allowed only inside configured limits.

Policy must bound at least:

- repair attempt count;
- total execution time;
- project concurrency.

If the runtime cannot confidently classify a change as remaining within approved intent, escalate rather than continue.

---

## Clarification / Decision Packets

Runtime-generated packets are untrusted claims about repository state.

Relay may verify deterministic portions such as:

- changed-file list;
- Git status;
- test command results when Relay executed them.

The executor recommendation must never be treated as an automatically approved decision.

---

## Resolution Safety

A Resolution Packet may change implementation direction but still cannot:

- change the local project path;
- bypass repository validation;
- inject arbitrary shell commands;
- introduce secrets;
- silently enable push.

Local policy remains authoritative.

---

## Secrets

v0.1 should rely on existing CLI authentication where possible.

Do not store provider/GitHub secrets unless a future feature genuinely requires it.

---

## Source Control Policy

v0.1:

```text
read status        yes
read diff          yes
edit via runtime   yes
automatic repair   yes, bounded
commit             no automatic behavior
push               never automatic
```

Future commit/push support requires a separate ADR.

---

## Community Extension Isolation

External-process extensions are preferred over in-process dynamic libraries.

They remain untrusted processes and require explicit installation/configuration.

---

## MCP Submission Surface

`relay serve-mcp` is ingress-only: it can validate and persist handoffs, never
execute anything. Client-facing error messages never carry local filesystem
paths: local-file error variants are scrubbed to a generic message with full
detail kept on stderr diagnostics only; registry/protocol errors surface keys
and protocol facts, not paths. Legacy-era MCP requests are served leniently
before `notifications/initialized` (the specifications do not gate client
requests on initialize); the lifecycle state is tracked for diagnostics, and
every submission still passes the same fail-closed validation as local import.
