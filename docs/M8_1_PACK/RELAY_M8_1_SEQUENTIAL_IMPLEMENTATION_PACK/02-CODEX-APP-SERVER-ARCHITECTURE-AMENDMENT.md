# Relay — Codex App-Server Architecture Amendment Plan

**Status:** Proposed architecture amendment  
**Date:** 2026-08-15  
**Scope:** Codex runtime integration, M6 runtime-session contract amendment, permissions, authentication, model/reasoning discovery, execution events, diff/activity, and Relay TUI integration.

---

# 1. Decision

Adopt **Codex app-server over local stdio JSONL** as Relay's preferred Codex rich-client integration.

Do not treat this as a replacement for Relay's scheduler, persistence, repository safety, or execution approval.

The architecture becomes:

```text
Relay TUI
   ↓
Relay Core
   ↓
M6 ExecutionScheduler
   ↓
Runtime contract
   ↓
relay-runtime-codex
   ↓
codex app-server --listen stdio://
   ↓
ChatGPT/Codex account
```

Relay remains the orchestrator.

Codex remains the coding runtime.

---

# 2. Why App-Server Fits Relay

The official Codex app-server interface is explicitly intended for rich clients and exposes:

```text
authentication
conversation/thread lifecycle
approvals
streamed agent events
model discovery
reasoning-effort options
sandbox policy
working directory
cancellation
diff updates
account/rate-limit state
```

This solves several current Relay problems at the correct layer.

---

# 3. M6 Contract Amendment

Do **not** redesign M6 scheduling.

Preserve:

```text
global concurrency
per-project concurrency
cross-project isolation
durable lifecycle authority
recovery
cancellation
repair bounds
persistence-failure behavior
```

Add only a bidirectional runtime-session control channel.

Current conceptual form:

```rust
RuntimeSession {
    events,
}
```

Target:

```rust
RuntimeSession {
    events,
    control,
}
```

with:

```rust
pub enum RuntimeControl {
    PermissionDecision { ... },
    UserInputResponse { ... }, // future-compatible, not required immediately
}
```

Existing runtimes may expose:

```text
PreapprovedOnly
```

while Codex app-server exposes:

```text
LiveApproval
```

---

# 4. Runtime Capability

Add:

```rust
pub enum RuntimePermissionSupport {
    PreapprovedOnly,
    LiveApproval,
}
```

and expose it in `RuntimeCapabilities`.

Example:

```text
Fake Runtime
  PreapprovedOnly

Codex exec fallback
  PreapprovedOnly

Codex app-server
  LiveApproval

Antigravity
  depends on documented provider support
```

---

# 5. App-Server Transport

Use:

```text
codex app-server --listen stdio://
```

or default stdio.

Do not use WebSocket for normal Relay local execution.

Why:

```text
local child process
no listening network socket
newline-delimited JSON
simpler lifecycle
smaller attack surface
```

WebSocket support remains out of scope.

---

# 6. Initialization

Immediately after process creation:

```text
initialize
initialized
```

Client info:

```json
{
  "name": "relay",
  "title": "Relay",
  "version": "<relay-version>"
}
```

Do not set:

```text
experimentalApi = true
```

for the initial implementation unless a required feature truly depends on it.

Prefer stable API surfaces first.

---

# 7. Authentication

Use app-server:

```text
account/read
account/login/start
account/login/completed
account/updated
```

Relay should prefer:

```text
ChatGPT managed authentication
```

for normal Codex subscription usage.

Supported user-facing states:

```text
Not signed in
ChatGPT
API key
Bedrock
other provider-supported modes
```

---

# 8. No Silent Billing-Mode Fallback

Permanent Relay rule:

> Relay must never silently switch a runtime from subscription-backed authentication to a separately metered API-key mode.

Example:

```text
ChatGPT auth unavailable
        ↓
BLOCK
```

not:

```text
OPENAI_API_KEY found
        ↓
silently switch billing mode
```

If the user explicitly configures API-key mode, show it clearly.

---

# 9. Runtime Dashboard — Authentication

Target:

```text
Codex

Authentication
  ✓ ChatGPT
  Account        user@example.com
  Plan           Business

Usage
  Primary        25%
  Resets         14:32

Billing mode
  ChatGPT plan
```

API key:

```text
Authentication
  API key

Billing mode
  Separate API usage
```

No secrets displayed.

---

# 10. Account / Plan Data

Use:

```text
account/read
account/updated
```

for:

```text
account type
email where available
planType
auth mode
```

Use:

```text
account/rateLimits/read
account/rateLimits/updated
```

for quota visibility.

Do not infer plan/billing from environment variables.

---

# 11. Model Discovery

Delete hard-coded Codex model catalogs.

Use:

```text
model/list
```

The response already supplies:

```text
model id
display name
isDefault
hidden
defaultReasoningEffort
supportedReasoningEfforts
input modalities
personality support
upgrade metadata
```

Relay can therefore render an accurate model selector.

---

# 12. Model Selector

Example:

```text
Codex model

> GPT-5.6 Sol            default
  GPT-5.x Codex
  ...

Reasoning
  Low
  Medium
  High
```

Reasoning options must come from:

```text
selected model.supportedReasoningEfforts
```

not a global hard-coded enum presented as universally valid.

Relay's internal neutral enum may remain.

---

# 13. Per-Run Model and Reasoning

Use app-server turn parameters.

Freeze into the Relay execution policy:

```text
model
effort
profile
sandbox
permissions
duration
repair budget
```

Then send on:

```text
turn/start
```

Example shape:

```json
{
  "threadId": "...",
  "model": "<selected-model>",
  "effort": "high",
  "cwd": "<trusted-project>",
  "sandboxPolicy": { ... }
}
```

The TUI must display the same frozen values.

---

# 14. Repository Boundary

Relay remains authoritative.

`cwd` comes only from:

```text
HandoffTask project key
↓
M2 Project Registry
↓
M3 repository eligibility
↓
trusted local project path
```

Never use a provider-supplied repository path as authority.

---

# 15. Sandbox Policy

For Implement:

```json
{
  "type": "workspaceWrite",
  "writableRoots": ["<trusted-project-root>"],
  "networkAccess": false
}
```

For Review:

```json
{
  "type": "readOnly"
}
```

No extra writable roots in initial Relay profiles.

---

# 16. Approval Policy

Do not hard-code an approval policy without checking managed requirements.

At runtime:

```text
configRequirements/read
```

may constrain:

```text
allowedApprovalPolicies
allowedSandboxModes
network requirements
```

Relay should:

```text
resolve desired policy
intersect with managed requirements
fail closed if incompatible
```

Do not bypass organization requirements.

---

# 17. Permission Profile API

`permissionProfile/list` is beta/experimental.

Do not make it a hard dependency in the first implementation.

Initial implementation should use the stable sandbox policy + approval request flows.

Later Relay versions may optionally use permission profiles after explicit experimental API adoption.

---

# 18. Live Approval Events

Codex app-server can ask Relay for:

```text
item/commandExecution/requestApproval
item/fileChange/requestApproval
item/permissions/requestApproval
```

Map each into:

```rust
RuntimeEvent::PermissionRequested(...)
```

---

# 19. Provider-Neutral Permission Request

Recommended:

```rust
pub struct RuntimePermissionRequest {
    pub request_id: String,
    pub kind: RuntimePermissionKind,
    pub reason: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<PathBuf>,
    pub requested: RuntimeRequestedPermissions,
    pub available_decisions: Vec<RuntimePermissionDecisionKind>,
}
```

---

# 20. Permission Ceiling

The provider may request permission.

Relay decides whether it is within the frozen execution envelope.

Example:

```text
Codex / Implement
workspace write = allowed

Codex asks to edit:
  apps/relay-tui/src/main.rs

→ eligible for approval
```

But:

```text
Codex asks to write:
  C:\Windows\...

→ deny
```

---

# 21. Relay Permission UI

Target:

```text
Permission requested

Codex wants to modify files:

  apps/relay-tui/src/main.rs
  apps/relay-tui/src/command_completion.rs

Scope
  Relay workspace

Reason
  Apply the requested correction.

> Allow once
  Allow for this run
  Deny
  Cancel execution
```

---

# 22. Command Approval

For command requests display:

```text
Command
  cargo test --workspace

cwd
  .

Reason
  Verify the implementation.
```

Normal UI shows the logical command.

Raw provider/wrapper details remain in `/logs raw`.

---

# 23. Network Approval

When `networkApprovalContext` is present:

```text
Network permission requested

Host
  crates.io

Protocol
  https
```

Do not present a confusing raw shell command as the primary context.

If Relay profile has network disabled:

```text
permission exceeds profile
```

and Relay must not grant it live.

---

# 24. Permission Tool Requests

`item/permissions/requestApproval` supports filesystem/network subsets.

Relay must respond with:

```text
requested ∩ Relay-approved ceiling
```

Never grant unrequested capability.

Never grant outside the Relay profile.

---

# 25. Pending Permission Scheduler State

Add:

```text
WAITING_PERMISSION
```

as a scheduler/execution substate.

Do not necessarily create a new durable `TaskState`.

The durable task can remain:

```text
Running
```

while the execution is paused.

The TUI must clearly show:

```text
! WAITING FOR PERMISSION
```

---

# 26. Permission Is Not Repair

Permanent lifecycle rule:

```text
PermissionBlocked
≠
ImplementationFailure
```

Do not spend repair attempts when:

```text
write denied
command denied
network denied
auth missing
sandbox incompatible
```

If live approval is unavailable:

```text
NeedsDecision
```

is appropriate.

---

# 27. Command / File Activity

Use app-server item events:

```text
item/started
item/completed
item/commandExecution/outputDelta
```

Map to Relay's provider-neutral activity model.

Command item already includes:

```text
command
cwd
status
exitCode
durationMs
```

So Relay does not need to scrape PowerShell wrapper output for normal presentation.

---

# 28. Logical Command Presentation

Normal:

```text
● Run
  cargo check --workspace
  cwd .
```

Raw diagnostics:

```text
/logs raw
```

may show:

```text
powershell.exe
cmd.exe
full argv
full absolute cwd
```

---

# 29. File Change Activity

Use:

```text
fileChange
```

items for provider activity.

But canonical repository truth remains:

```text
local Git state
```

Provider event:

```text
File changed
```

is a trigger/hint.

Relay Git inspection determines actual final diff.

---

# 30. Diff Updates

App-server provides:

```text
turn/diff/updated
```

Relay may use this as a live provider-side preview.

However:

```text
local Git diff
```

remains authoritative for:

```text
final Result
/diff
repository effects
```

This preserves M3 trust.

---

# 31. Plan Updates

App-server provides:

```text
turn/plan/updated
```

Relay may display:

```text
Plan

✓ Inspect runtime routing
● Update scheduler composition
◌ Add regression coverage
```

This is provider-visible plan data, not hidden chain-of-thought.

---

# 32. Reasoning

App-server exposes reasoning summaries and raw reasoning fields.

Relay policy:

```text
do not display hidden/raw reasoning text
```

Allowed:

```text
Analyzing...
Reasoning summary, only if explicitly intended by provider as user-visible summary
```

Preferred initial Relay behavior:

```text
● Analyzing
```

No raw reasoning blocks.

---

# 33. Agent Messages

Use:

```text
item/agentMessage/delta
agentMessage
```

for user-visible provider text.

Treat as Markdown only where safe/appropriate.

Render inside Relay's own Markdown renderer.

---

# 34. Cancellation

Map Relay cancellation to:

```text
turn/interrupt
```

Preserve M6 cancellation lifecycle semantics.

Do not rely only on OS process killing for normal cancellation.

If app-server becomes unresponsive:

```text
fallback process-tree termination
```

remains appropriate.

---

# 35. Thread Lifecycle

One Relay execution may use one Codex thread initially.

Suggested:

```text
start app-server process
initialize
thread/start
turn/start
stream
turn completed
shutdown
```

This is easiest to reason about.

Later optimization may reuse one app-server process across runs.

Do not introduce process reuse until execution isolation is proven.

---

# 36. Thread Persistence

Relay's canonical history remains SQLite.

Do not make Codex thread storage authoritative.

Store only necessary linkage:

```text
provider thread id
provider turn id
```

as optional execution metadata.

Relay TaskId / ExecutionId remain canonical.

---

# 37. Account Login UX

If not signed in:

```text
Codex

Status
  Not authenticated

> Sign in with ChatGPT
  Use API key
  Back
```

Default/highlight:

```text
Sign in with ChatGPT
```

because Relay's normal Codex experience is subscription-backed.

---

# 38. ChatGPT Login Flow

Use:

```text
account/login/start
type = chatgpt
```

Relay displays:

```text
Open browser to continue...
```

and waits for:

```text
account/login/completed
account/updated
```

Do not manually manage tokens.

---

# 39. Device-Code Flow

Support later or as fallback:

```text
chatgptDeviceCode
```

TUI:

```text
Visit
  https://...

Code
  ABCD-1234
```

This is useful when local browser callbacks are inconvenient.

---

# 40. Do Not Use External ChatGPT Tokens Initially

`chatgptAuthTokens` is experimental.

Do not use it for normal Relay authentication.

Managed ChatGPT auth is simpler and safer.

---

# 41. Rate Limits

Use:

```text
account/rateLimits/read
account/rateLimits/updated
```

Runtime dashboard may show:

```text
Codex usage

Primary
  31%

Resets
  14:32
```

Do not poll aggressively.

Fetch:

```text
runtime detail
doctor
explicit refresh
after rate-limit error
```

---

# 42. Usage Endpoint

`account/usage/read` may be used for optional diagnostics/usage display.

Do not make usage statistics part of execution correctness.

---

# 43. Result Runtime Identity

Result must display frozen execution metadata:

```text
Provider
Profile
Model
Effort
Sandbox
Permission envelope
```

Never derive historical Result values from the current global runtime default.

---

# 44. Result Scroll

Add scroll state for Result:

```text
Up / Down
PageUp / PageDown
Home / End
```

Long failure summaries must remain inspectable.

---

# 45. Execution Scroll

Live execution:

```text
follow tail by default
```

If user scrolls up:

```text
pause follow
```

Show:

```text
◌ paused · End to resume live
```

New events continue accumulating.

---

# 46. Runtime Dashboard

Codex detail target:

```text
Codex

Account
  ✓ user@example.com
  Plan          Business
  Auth          ChatGPT

Runtime
  Version       ...
  Interface     App Server

Model
  GPT-5.6 Sol

Reasoning
  Medium

Permissions
  Live approval ✓

Usage
  Primary       31%

> Change model
  Change reasoning
  Test runtime
  Sign out
```

---

# 47. Doctor

Codex checks:

```text
Executable
App-server startup
Initialization
Authentication
Model catalog
Sandbox compatibility
Managed config requirements
```

Do not run a paid agent turn merely for Doctor.

---

# 48. App-Server Schema Compatibility

At build/test time or compatibility-probe time, use:

```text
codex app-server generate-json-schema
```

or generated schemas appropriate to the installed Codex version.

Do not assume one forever-static wire schema.

Adapter should:

```text
identify version
parse known stable fields
tolerate additive unknown fields
fail closed on required incompatible fields
```

---

# 49. `configRequirements/read`

Use this before execution when relevant.

Managed environments may restrict:

```text
approval policy
sandbox mode
network
features
```

Relay must not attempt to bypass these.

If configured Relay profile conflicts:

```text
Runtime profile incompatible with organization requirements.
```

Disable Run.

---

# 50. Experimental API Rule

Initial implementation:

```text
experimentalApi = false
```

unless a required approved Relay feature cannot be implemented otherwise.

Features to defer initially:

```text
process/spawn
background terminals
experimental thread pagination
experimental permission-profile selection
dynamic tools
```

Stable/minimally-required approval flows are enough.

---

# 51. Dangerous App-Server Methods Relay Must Not Expose Casually

Do not wire these into normal Relay commands:

```text
thread/shellCommand
process/spawn
fs/remove
config writes
plugin install
marketplace changes
```

They may exist in app-server, but Relay does not automatically inherit every provider capability.

Relay's own safety model remains authoritative.

---

# 52. Runtime Adapter Allowlist

The Codex adapter should implement only a narrow RPC allowlist:

```text
initialize
thread/start
turn/start
turn/interrupt
model/list
account/*
configRequirements/read

approval responses

required event notifications
```

Add more only through explicit Relay features.

---

# 53. Regression Obligations for Reopening M6

All accepted M6/M7 tests remain required:

```text
global concurrency
per-project concurrency
cross-project isolation
bounded recovery
cancellation
persistence failure
pre-claim cancellation
scheduler driver liveness
```

Add:

```text
permission request does not block unrelated project
permission decision reaches only correct execution
denial consumes zero repair attempts
grant cannot exceed frozen envelope
old PreapprovedOnly runtimes still work
```

---

# 54. M6 Amendment Record

Do not rewrite historical M6 acceptance.

Record:

```text
M6 original accepted:
f068017...

M6 runtime-control amendment:
<new SHA>

Reason:
Real provider integration requires a bidirectional permission/control channel.

Unchanged:
scheduler semantics
persistence authority
concurrency
recovery
cancellation isolation

Added:
runtime control channel
permission requests
permission decisions
permission waiting state
```

---

# 55. Implementation Sequence

## A — Source-of-truth fixes first

```text
A1 Result scrolling
A2 Execution scrolling
A3 fix Result runtime/profile identity
A4 eliminate repair-on-permission behavior
A5 ensure codex/implement uses correct workspaceWrite sandbox
```

## B — M6 additive control contract

```text
B1 RuntimeControlHandle
B2 RuntimeEvent::PermissionRequested
B3 scheduler permission registry
B4 resolve_permission
B5 WAITING_PERMISSION UI status
B6 fixture-runtime tests
```

## C — Codex app-server client

```text
C1 child process + stdio JSONL
C2 initialize
C3 account/read
C4 model/list
C5 thread/start
C6 turn/start
C7 item/turn event stream
C8 turn/interrupt
```

## D — Live approvals

```text
D1 command approval
D2 file-change approval
D3 permissions approval
D4 serverRequest/resolved
D5 Relay permission modal
```

## E — Rich UX

```text
E1 command activity
E2 file changes
E3 plan updates
E4 agent messages
E5 live diff preview
E6 local-Git final diff
E7 rate-limit/account runtime dashboard
```

---

# 56. Dogfood Acceptance

Test a small Relay-on-Relay edit.

Expected:

```text
Codex app-server starts
ChatGPT auth recognized
model catalog loaded
selected model/effort sent
trusted cwd used
workspaceWrite policy sent
Run starts
activity streams
permission request surfaces in Relay if needed
human approves
same turn continues
files change
verification runs
Result persists actual profile/model/effort
no commit
no push
```

---

# 57. Hard Completion Gate

Do not call the migration complete if:

```text
[ ] Relay still hard-codes Codex models.
[ ] reasoning choices ignore model/list.
[ ] Result displays current default instead of execution snapshot.
[ ] codex/implement can accidentally run read-only.
[ ] permission request becomes implementation_failure.
[ ] permission request consumes repair attempts.
[ ] TUI communicates directly with app-server around Core/scheduler.
[ ] Relay can grant outside-workspace paths.
[ ] Relay silently falls back to API-key billing.
[ ] app-server experimental features are enabled without a need.
[ ] provider app-server history becomes canonical instead of Relay SQLite.
[ ] hidden raw reasoning is displayed.
[ ] unrelated project scheduling stops during approval wait.
```

---

# 58. Final Architecture Rule

> **Relay owns execution policy, project trust, scheduling, persistence, and human approvals. Codex app-server supplies a provider-native authenticated agent session, structured events, model capabilities, and approval requests.**

And:

> **The M6 amendment is additive: runtime sessions become bidirectional, while the accepted scheduler and durable-state semantics remain unchanged.**
