# Relay — Permission Broker + Scrollable Execution/Result UX Implementation Plan

**Status:** DEV corrective implementation plan  
**Date:** 2026-08-15  
**Target:** M8.1 multi-runtime orchestration branch  
**Latest remote source inspected:** `45b8008bea8b3639d3cbf33d9aa84895fd674cfd`  
**Important:** the user's current local worktree appears to contain additional uncommitted/newer edits. Do not reset or discard them. Rebase this plan onto the actual local HEAD.

---

# 1. Why the Current Dogfood Run Failed

The visible failure is a permission-envelope failure, not a normal implementation defect.

Observed result:

```text
Profile: codex/default
State: failed
Outcome: implementation_failure
Repairs: 2

Summary:
...
patching was rejected because the repository is mounted read-only
and approvals are disabled.
...
rerun with workspace write permission
```

This means the coding runtime reached a state equivalent to:

```text
task requires edits
        ↓
provider effective filesystem = read-only
        ↓
provider asks / would need greater permission
        ↓
approval path unavailable or disabled
        ↓
provider cannot edit
        ↓
Relay interprets this as implementation failure
        ↓
repair attempt repeats same permission envelope
        ↓
same failure again
```

This behavior is wrong in three ways:

```text
1. Relay approved an Implement run but effective write was unavailable.
2. A permission problem was classified as implementation_failure.
3. Repair attempts were spent on a deterministic permission blocker.
```

---

# 2. Current Source Facts

The current Codex adapter already intends:

```text
codex/review
    → read-only

codex/implement
codex/default
codex/deep
    → workspace-write
```

and launches Codex with:

```text
--ask-for-approval never
--sandbox <resolved mode>
```

The CLI currently treats:

```text
codex exec
```

as a non-interactive/scripted execution surface.

That works for a fully pre-approved Relay execution envelope.

It is not the best interface when Relay itself wants to become the UI that:

```text
receives provider permission requests
shows them to the human
accepts / declines them
continues the same provider session
```

For that richer use case, Codex exposes `codex app-server`.

OpenAI documents app-server as the deep product-integration interface for:

```text
authentication
conversation history
approvals
streamed agent events
```

and it supports server-initiated approval requests.

---

# 3. Product Decision

Relay should support **two permission strategies**.

## Strategy A — Pre-approved execution envelope

Used by providers/integrations that do not support live approval bridging.

Before Run, Relay shows:

```text
Filesystem
  Read workspace     Allow
  Write workspace    Allow
  Outside workspace  Deny

Commands             Allow inside sandbox
Network              Deny
Commit               Deny
Push                 Deny
```

The human presses:

```text
Run
```

That is the permission approval.

The provider is launched with exactly that envelope.

If the provider cannot establish it:

```text
Run must fail as PermissionBlocked / NeedsDecision
```

not `ImplementationFailure`.

## Strategy B — Live provider permission broker

Used when the provider has a documented bidirectional approval protocol.

Provider:

```text
needs write / command / network / file-change permission
        ↓
RuntimeEvent::PermissionRequested
        ↓
Relay Core
        ↓
Relay TUI modal
        ↓
human decision
        ↓
RuntimeControl::PermissionDecision
        ↓
provider session resumes
```

Codex app-server supports this model directly.

---

# 4. Permanent Permission Rule

Freeze:

> **Relay owns permission decisions. Provider runtimes request capabilities; they do not independently expand the execution envelope.**

And:

> **A provider may never receive more permission than both the runtime profile ceiling and the human's explicit approval permit.**

---

# 5. Runtime Permission Ceiling

Every frozen Run policy needs a permission ceiling.

Add to `relay-runtime`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePermissionEnvelope {
    pub workspace_read: bool,
    pub workspace_write: bool,
    pub command_execution: bool,
    pub network: bool,
    pub additional_writable_roots: Vec<PathBuf>,
}
```

Do not include:

```text
commit
push
remote mutation
```

as generic filesystem permissions.

Those remain separate Relay safety policies and stay OFF.

---

# 6. Profile → Permission Envelope

Examples:

## Review

```text
workspace_read          true
workspace_write         false
command_execution       true or provider-specific safe subset
network                 false
additional roots        none
```

## Implement

```text
workspace_read          true
workspace_write         true
command_execution       true
network                 false
additional roots        none
```

## Deep

Initially same capability boundary as Implement:

```text
workspace_read          true
workspace_write         true
command_execution       true
network                 false
additional roots        none
```

Deep changes intelligence/budget, not filesystem privilege.

---

# 7. Extend ExecutionRunPolicy

Target:

```rust
pub struct ExecutionRunPolicy {
    pub runtime_selection: RuntimeExecutionSelection,
    pub permissions: RuntimePermissionEnvelope,
    pub max_duration: Duration,
    pub max_repair_attempts: u32,
}
```

If the runtime-model/reasoning plan has not landed yet:

```rust
pub struct ExecutionRunPolicy {
    pub runtime_profile_id: String,
    pub permissions: RuntimePermissionEnvelope,
    pub max_duration: Duration,
    pub max_repair_attempts: u32,
}
```

Do not create two competing policy types.

Merge with the current local model/reasoning changes.

---

# 8. Freeze Permissions at Run Approval

At Task Review:

```text
Profile
  Codex / Implement

Permissions
  Workspace read        ✓
  Workspace write       ✓
  Commands              ✓
  Network               ×
  Outside workspace     ×

> Run
```

When Run is pressed, freeze this envelope.

Changing global profile later must not change an active execution.

---

# 9. Add Permission Support Capability

Extend runtime capabilities:

```rust
pub enum RuntimePermissionSupport {
    PreapprovedOnly,
    LiveApproval,
}
```

Then:

```rust
pub struct RuntimeCapabilities {
    pub streaming_output: bool,
    pub cancellation: bool,
    pub deterministic_outcome: bool,
    pub permission_support: RuntimePermissionSupport,
}
```

Examples:

```text
Codex app-server
  LiveApproval

provider with only batch CLI
  PreapprovedOnly
```

Do not pretend every provider supports live permissions.

---

# 10. Provider-Neutral Permission Request

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePermissionRequest {
    pub request_id: String,
    pub kind: RuntimePermissionKind,
    pub reason: Option<String>,
    pub command: Option<String>,
    pub working_directory: Option<PathBuf>,
    pub requested: RuntimeRequestedPermissions,
    pub available_decisions: Vec<RuntimePermissionDecisionKind>,
}
```

Kinds:

```rust
pub enum RuntimePermissionKind {
    CommandExecution,
    FileChange,
    Filesystem,
    Network,
}
```

---

# 11. Requested Permissions

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequestedPermissions {
    pub filesystem: Vec<RequestedFilesystemPermission>,
    pub network: Vec<RequestedNetworkPermission>,
}
```

Filesystem:

```rust
pub struct RequestedFilesystemPermission {
    pub path: PathBuf,
    pub access: FilesystemAccess,
}
```

```rust
pub enum FilesystemAccess {
    Read,
    Write,
}
```

Network:

```rust
pub struct RequestedNetworkPermission {
    pub host: String,
    pub port: Option<u16>,
    pub protocol: Option<String>,
}
```

---

# 12. Permission Decisions

Provider-neutral:

```rust
pub enum RuntimePermissionDecision {
    ApproveOnce,
    ApproveForSession,
    Deny,
    CancelExecution,
}
```

For subset-based provider requests:

```rust
pub struct RuntimePermissionGrant {
    pub decision: RuntimePermissionDecision,
    pub granted: RuntimeRequestedPermissions,
}
```

Relay may grant only:

```text
requested ∩ approved ceiling
```

Never more.

---

# 13. Make RuntimeSession Bidirectional

Current:

```rust
pub struct RuntimeSession {
    pub session_id: String,
    pub events: mpsc::Receiver<RuntimeEvent>,
}
```

Change to:

```rust
pub struct RuntimeSession {
    pub session_id: String,
    pub events: mpsc::Receiver<RuntimeEvent>,
    pub control: RuntimeControlHandle,
}
```

Control:

```rust
#[derive(Clone)]
pub struct RuntimeControlHandle {
    tx: mpsc::Sender<RuntimeControl>,
}
```

```rust
pub enum RuntimeControl {
    PermissionDecision {
        request_id: String,
        grant: RuntimePermissionGrant,
    },
}
```

Cancellation may remain on the existing CancellationToken.

---

# 14. Add RuntimeEvent::PermissionRequested

```rust
pub enum RuntimeEvent {
    Activity(RuntimeActivity),

    PermissionRequested(RuntimePermissionRequest),

    Output {
        stream: RuntimeOutputStream,
        bytes: Vec<u8>,
    },

    Outcome(RuntimeOutcome),

    InvalidOutput {
        reason: String,
    },
}
```

If `RuntimeActivity` wiring is not yet landed, do not block permission support on it.

PermissionRequested is a separate control-plane event.

---

# 15. Core Permission Event

Add:

```rust
CoreExecutionEvent::PermissionRequested {
    task_id: TaskId,
    execution_id: ExecutionId,
    request: RuntimePermissionRequest,
}
```

Add:

```rust
CoreExecutionEvent::PermissionResolved {
    task_id: TaskId,
    execution_id: ExecutionId,
    request_id: String,
    decision: RuntimePermissionDecision,
}
```

Core does not render the request.

---

# 16. Scheduler Permission Handle Registry

The scheduler needs to route UI decisions back to the active RuntimeSession.

Extend ActiveExecution:

```rust
struct ActiveExecution {
    project_key: ProjectKey,
    cancellation: CancellationToken,
    runtime_control: Option<RuntimeControlHandle>,
    pending_permission: Option<RuntimePermissionRequest>,
}
```

Because the control handle is only known after `runtime.start()`:

```text
start runtime
↓
register RuntimeControlHandle in active execution
↓
receive permission request
↓
store pending request
```

---

# 17. Scheduler Permission API

Add:

```rust
pub async fn resolve_permission(
    &self,
    task_id: TaskId,
    request_id: &str,
    grant: RuntimePermissionGrant,
) -> Result<(), ExecutionSchedulerError>
```

Validation:

```text
task active
pending request exists
request IDs match
grant is subset of request
grant is within frozen permission ceiling
```

Then forward:

```text
RuntimeControl::PermissionDecision
```

---

# 18. Do Not Let TUI Talk Directly to Provider

Wrong:

```text
TUI
↓
Codex JSON-RPC
```

Correct:

```text
TUI
↓
ExecutionScheduler::resolve_permission
↓
RuntimeControlHandle
↓
provider adapter
```

This preserves M6/Core ownership.

---

# 19. Permission Waiting State

Do not classify a pending permission as:

```text
FAILED
```

Do not spend a repair attempt.

Add scheduler UI state:

```rust
SchedulerTaskStatus::WaitingPermission
```

The durable TaskState may remain:

```text
Running
```

while the provider session is paused, if changing durable lifecycle is undesirable.

But the execution phase should be updated/persisted to:

```text
waiting_permission
```

if the current store supports a safe phase-only update.

If durable phase-only update is not currently supported:

```text
keep TaskState Running
emit explicit scheduler status
```

and document that pending permission is an execution substate.

Do not introduce M9 clarification workflow.

---

# 20. Permission Is Not Repair

Critical rule:

```text
PermissionBlocked
≠
ImplementationFailure
```

A permission denial/request must not enter:

```text
ImplementationFailure
→ Repairing
→ Running
```

because no code repair can solve missing filesystem rights.

---

# 21. Add RuntimeOutcome Detail

Current RuntimeOutcome has:

```text
kind
summary
```

Add a structured detail:

```rust
pub enum RuntimeOutcomeDetail {
    PermissionBlocked {
        reason: String,
        requested: Option<RuntimeRequestedPermissions>,
    },

    ProviderFailure {
        category: Option<String>,
    },
}
```

or a smaller:

```rust
pub enum RuntimeFailureCategory {
    Permission,
    Configuration,
    Provider,
}
```

Keep existing RuntimeOutcomeKind for durable milestone compatibility.

---

# 22. Map Permission Block to NeedsDecision

If the provider cannot pause for live approval:

```text
permission blocked
↓
RuntimeOutcomeKind::NeedsDecision
↓
CoreExecutionEvent::DecisionNeeded
↓
TaskState::NeedsDecision
```

Summary:

```text
Workspace write permission is required.
```

No repair attempt.

The Result view offers:

```text
Retry with Write
Keep blocked
Cancel task
```

This is a runtime permission decision, not a task clarification.

---

# 23. Existing Codex `exec` Strategy

For the current batch/non-interactive path, keep a simple rule:

## Review

```text
--sandbox read-only
--ask-for-approval never
```

## Implement / Deep

```text
--sandbox workspace-write
--ask-for-approval never
```

Why `never`?

Because the human already approved the Relay execution envelope before launch.

The provider should not open an interactive prompt outside Relay.

If workspace-write cannot actually be established:

```text
block before Run when detectable
or
return NeedsDecision / PermissionBlocked
```

Do not silently retry.

---

# 24. Effective Write Check Before Run

Task Review should distinguish:

```text
Configured
Effective
```

Example:

```text
Workspace write

Configured   ✓ Allow
Effective    ? Unknown
```

or:

```text
Effective    ✓ Verified
```

If effective status is definitively:

```text
Mismatch
Unavailable
```

disable Run.

Do not launch an implementation task that Relay already knows cannot write.

---

# 25. Fix False Verification

Current real-runtime capability logic must not use:

```text
working_directory.is_dir()
→ write Verified
```

Directory existence proves only that the directory exists.

Use:

```text
Configured
Unknown
Mismatch
Verified
Unavailable
```

honestly.

For the current `exec` adapter, `Configured` or `Unknown` is acceptable until a safe provider-native verification exists.

---

# 26. Correct Codex Interface for Live Approval

If Relay wants the provider to pause and ask Relay for permissions **during a run**, migrate the Codex runtime worker to:

```text
codex app-server --listen stdio://
```

instead of trying to make `codex exec` interactive.

This is the recommended long-term Codex adapter for Relay.

---

# 27. Why App Server Fits Relay Better

Codex app-server is designed for rich clients.

It provides:

```text
bidirectional JSON-RPC
streamed agent events
command approvals
file-change approvals
permission requests
authentication/account state
```

Relay is a rich local orchestration client.

This is a better semantic match than CI-style `codex exec`.

---

# 28. Codex Approval Events to Map

App-server can emit:

```text
item/commandExecution/requestApproval
item/fileChange/requestApproval
item/permissions/requestApproval
```

Map each to:

```text
RuntimeEvent::PermissionRequested
```

---

# 29. Command Approval Mapping

Provider request can contain:

```text
command
cwd
reason
available decisions
network approval context
additional permissions
```

Relay renders:

```text
Permission requested

Codex wants to run:

  cargo test --workspace

cwd
  .

Reason
  Verification requires executing workspace tests.

Permissions
  Command execution       requested
  Workspace access        within approved project

> Allow once
  Allow for this run
  Deny
  Cancel execution
```

---

# 30. File-Change Approval Mapping

Provider request:

```text
file change approval
grantRoot
reason
```

Relay renders:

```text
Write permission requested

Files
  apps/relay-tui/src/command_completion.rs
  apps/relay-tui/src/main.rs

Scope
  Relay workspace

> Allow once
  Allow writes for this run
  Deny
  Cancel execution
```

Do not approve roots outside the trusted project.

---

# 31. Filesystem Permission Request Mapping

For app-server `item/permissions/requestApproval`:

```text
requested filesystem paths
requested network targets
cwd
reason
```

Relay computes:

```text
requested
∩
profile ceiling
∩
trusted project boundary
```

Only the intersection can be granted.

---

# 32. Additional Write Roots

Current product rule:

```text
additional writable roots = none
```

For M8.1:

```text
request outside trusted root
→ deny
→ explain
```

Do not add an "Allow any path" button.

A future explicit project configuration may permit trusted additional roots.

---

# 33. Network Permission

Current built-in profiles:

```text
network = false
```

If provider asks for:

```text
github.com:443
```

Relay displays:

```text
Network access requested
github.com:443

Current profile:
Network Off
```

For M8.1:

```text
Deny
or
Retry with a separately approved network-enabled policy
```

Do not dynamically widen network if the frozen profile forbids it.

---

# 34. Permission Decision Hierarchy

Relay should distinguish:

## Within frozen envelope

Example:

```text
Implement profile already permits workspace write
provider asks to approve a file change in trusted workspace
```

Allowed options:

```text
Allow once
Allow for this run
Deny
Cancel
```

## Beyond frozen envelope

Example:

```text
Review profile
provider asks for write
```

Do not show:

```text
Allow for this run
```

as an immediate live grant.

Show:

```text
Permission exceeds current profile.

> Retry using Codex / Implement
  Deny
  Cancel
```

This preserves the meaning of profiles.

---

# 35. Permission Modal

Add a focused modal/overlay.

Example:

```text
╭ Permission requested ─────────────────────────────╮
│ Codex wants to modify files in this repository.   │
│                                                   │
│ Filesystem                                        │
│   Write workspace          requested              │
│                                                   │
│ Reason                                            │
│   Apply the command-completion fix.               │
│                                                   │
│ > Allow once                                      │
│   Allow for this run                              │
│   Deny                                            │
│   Cancel execution                                │
╰───────────────────────────────────────────────────╯
```

This is one of the few places where a border/modal is appropriate.

---

# 36. Permission Modal Input

Use:

```text
Up / Down
Enter
Esc
```

Esc:

```text
Deny
```

or:

```text
close only if request remains clearly pending
```

Prefer:

```text
Esc = Deny
```

with a one-line confirmation if ambiguity is a concern.

Do not allow regular slash commands to accidentally dismiss the prompt.

---

# 37. Permission Indicator in Execution View

While waiting:

```text
! WAITING FOR PERMISSION
```

Current:

```text
Codex / Implement
```

Activity:

```text
! Write permission requested

  apps/relay-tui/src/command_completion.rs
  apps/relay-tui/src/main.rs
```

The scheduler should release no additional same-project execution slot beyond existing M6 policy assumptions unless explicitly designed otherwise.

The active execution still counts toward concurrency.

---

# 38. Do Not Block Unrelated Projects

Permission wait in:

```text
Project A
```

must not stop:

```text
Project B
```

if global scheduler slots permit.

Preserve M6 cross-project isolation.

---

# 39. Permission Timeout

Provider requests may remain pending.

Add bounded local timeout or provider-provided timeout support.

Initial default:

```text
no Relay auto-approval
```

If provider supplies:

```text
autoResolutionMs
```

display:

```text
Provider request expires in 28s
```

Relay may auto-deny on expiry.

Never auto-approve.

---

# 40. Permission Audit Event

Persist permission decisions as execution events when practical.

Suggested event payload:

```json
{
  "type": "permission_decision",
  "request_id": "...",
  "kind": "file_change",
  "decision": "approve_once",
  "scope": "turn",
  "provider": "codex"
}
```

Do not persist:

```text
credentials
secrets
unbounded command output
```

---

# 41. Result Screen Profile Bug

The screenshot shows:

```text
Header:
codex/implement

Result:
Profile: codex/default
```

Current source has a `runtime_label()` path that derives the Result runtime from:

```rust
AppRuntime::default().identity()
```

instead of the frozen execution policy.

Fix this.

Result must display:

```text
the actual execution record
```

not:

```text
current app default
```

---

# 42. Persist Runtime Selection Snapshot

Result data should come from execution persistence.

Store:

```text
provider
profile
model
reasoning/effort
permission envelope
runtime version
```

Then:

```text
Result
History
Tasks
```

all show the exact configuration that actually ran.

---

# 43. Scrollability — Current Gap

Current Diff and Logs already maintain:

```text
diff_scroll
logs_scroll
```

and render with Ratatui `.scroll(...)`.

Result has no:

```text
result_scroll
```

and Execution has no:

```text
execution_scroll
```

Long Result content therefore extends beyond the viewport with no way to inspect the hidden portion.

---

# 44. Unify Scroll State

Do not add a separate loose `u16` forever for every view.

Introduce:

```rust
#[derive(Debug, Clone, Default)]
pub struct ScrollState {
    pub offset: u16,
    pub follow_tail: bool,
}
```

AppState:

```rust
execution_scroll: ScrollState,
result_scroll: ScrollState,
diff_scroll: ScrollState,
logs_scroll: ScrollState,
```

If refactoring all at once is risky:

```text
first add execution_scroll/result_scroll
then migrate Diff/Logs
```

---

# 45. Generic Scroll Commands

Utility:

```rust
impl ScrollState {
    fn line_up(&mut self)
    fn line_down(&mut self)
    fn page_up(&mut self, viewport_height: u16)
    fn page_down(&mut self, viewport_height: u16, content_height: u16)
    fn home(&mut self)
    fn end(&mut self, content_height: u16, viewport_height: u16)
}
```

Clamp offsets.

Do not allow unbounded `u16` growth.

---

# 46. Result Scrolling

On Result screen:

```text
Up / Down
PageUp / PageDown
Home / End
```

Scroll content.

Slash command mode remains available when not consuming navigation.

Suggested footer hint:

```text
↑↓ scroll  PgUp/PgDn page  Home/End
```

Keep muted.

---

# 47. Execution Scrolling

Execution is dynamic.

Default:

```text
follow_tail = true
```

New activity keeps the viewport at the bottom.

If user presses:

```text
Up
PageUp
```

set:

```text
follow_tail = false
```

and preserve viewport while new events arrive.

Show:

```text
◌ paused · End to resume live
```

Press:

```text
End
```

sets:

```text
follow_tail = true
```

and jumps to latest content.

This is the expected terminal-log UX.

---

# 48. Diff / Logs Scroll Improvements

Keep existing Up/Down.

Add:

```text
PageUp
PageDown
Home
End
```

Use the same ScrollState.

Logs default:

```text
follow_tail = true
```

when viewing active execution.

Diff:

```text
follow_tail = false
```

because it is a static document.

---

# 49. Scroll Reset Rules

When entering a new:

```text
Result
Diff
Task
Runtime detail
```

reset offset to:

```text
0
```

Execution:

```text
follow_tail = true
```

When reopening the same screen intentionally, preserving position is optional.

Initial implementation should reset for predictability.

---

# 50. Result Presentation

The current screenshot is too dense.

Target:

```text
RESULT — NEEDS DECISION

Task
  Fix contextual task command eligibility

Runtime
  Codex / Implement
  Model      ...
  Reasoning  ...
  Sandbox    Workspace write

Blocking issue
  Workspace write was not available to the provider.

No source files were changed.

Attempts
  Implementation       1
  Repairs              0

Repository
  feat/m8-codex-runtime
  dirty before run

Next
  > Retry with workspace write
    View logs
    View diff
    Tasks
```

Permission blocks should not consume repair count.

---

# 51. Outcome Classification

Add provider-neutral categories.

Suggested:

```rust
pub enum RuntimeBlockerKind {
    Permission,
    Authentication,
    Configuration,
    UserDecision,
}
```

Or integrate with `RuntimeOutcomeDetail`.

Result screen can then show:

```text
FAILED
```

only for true failure.

Permission:

```text
NEEDS DECISION
```

Authentication:

```text
BLOCKED
```

Configuration:

```text
BLOCKED
```

---

# 52. No Automatic Permission Escalation

Do not implement:

```text
provider fails read-only
→ Relay silently restarts as workspace-write
```

The user explicitly owns capability escalation.

Relay may recommend:

```text
Retry with Codex / Implement
```

but cannot silently expand.

---

# 53. Pre-Approved Write Shortcut

For ordinary coding tasks, avoid excessive prompts.

If:

```text
selected profile = Implement
Task Review says Workspace write = Allow
```

then Run approval already grants workspace write.

The provider should not need a second Relay confirmation for every normal edit when the provider protocol supports session-level approval.

For Codex app-server:

```text
file-change request inside trusted workspace
+
frozen profile already allows workspace-write
```

Relay may offer:

```text
Allow for this run
```

as the first permission prompt.

Future user setting:

```text
Auto-approve requests already inside frozen envelope
```

could be considered later.

Do not add auto-approve in this remediation unless explicitly approved.

---

# 54. Provider Permission Modes in Runtime Dashboard

Runtime Detail:

```text
Permissions

Relay envelope
  Workspace write      Allowed
  Network              Off

Provider bridge
  Live approvals       ✓ Supported
```

For providers without live approvals:

```text
Provider bridge
  Live approvals       × Not supported
  Mode                 Pre-approved envelope
```

This is provider-aware without breaking Core neutrality.

---

# 55. Codex App-Server Adapter Architecture

Do not embed app-server logic into the TUI.

Recommended:

```text
runtimes/relay-runtime-codex/

lib.rs
client.rs
protocol.rs
approvals.rs
activity.rs
process.rs
```

`process.rs`:

```text
spawn codex app-server --listen stdio://
stdin/stdout piped
stderr piped
CREATE_NO_WINDOW on Windows
process-group isolation on Unix
```

---

# 56. App-Server Session Lifecycle

Conceptual:

```text
spawn app-server
↓
initialize
↓
thread/start
↓
turn/start with Relay task intent
↓
receive streamed notifications + server requests
↓
send approval decisions
↓
receive terminal outcome
↓
shutdown child
```

Do not expose app-server JSON-RPC to Core.

Adapter translates it.

---

# 57. Codex Approval Mapping

Provider decisions:

Command execution may support provider-native:

```text
accept
acceptForSession
decline
cancel
```

Relay mapping:

```text
ApproveOnce       → accept
ApproveForSession → acceptForSession
Deny              → decline
CancelExecution   → cancel
```

Do not initially support automatic execpolicy amendments.

That can be a later feature.

---

# 58. Permission Subset Mapping

For `item/permissions/requestApproval`:

Relay receives requested:

```text
filesystem
network
```

Validate against frozen envelope.

Respond with only the allowed subset.

Never grant permissions the provider did not request.

Never grant permissions outside Relay ceiling.

---

# 59. File Approval Mapping

For provider file-change approval:

If requested change is inside trusted workspace and profile permits write:

```text
ApproveOnce / ApproveForSession available
```

If outside:

```text
Deny only
```

plus:

```text
Requested path is outside Relay's trusted project boundary.
```

---

# 60. Network-Specific UX

Codex app-server can identify network approval context.

Render network prompts separately:

```text
Network permission requested

Host
  crates.io

Protocol
  https

Current profile
  Network Off

This exceeds the approved execution profile.

> Deny
  Cancel execution
```

Do not display a meaningless raw shell command if provider identifies it as a network approval.

---

# 61. Current `codex exec` Fallback

Do not remove `codex exec` immediately if app-server work needs staged rollout.

Provide adapter mode:

```rust
pub enum CodexIntegrationMode {
    Exec,
    AppServer,
}
```

During transition:

```text
AppServer
  preferred for Relay interactive execution

Exec
  fallback for deterministic non-interactive runs
```

But do not let both implementations drift indefinitely.

The intended final Relay desktop/TUI integration should be AppServer.

---

# 62. Provider-Neutral Compatibility

Antigravity or future runtimes may implement:

```text
LiveApproval
```

only if their documented machine interface supports it.

Otherwise:

```text
PreapprovedOnly
```

and permission failure maps to NeedsDecision/retry.

Do not scrape provider UIs to simulate permission prompts.

---

# 63. Tests — Permission Envelope

Required:

```text
Review profile
→ workspace_write false

Implement
→ workspace_write true

Deep
→ workspace_write true

network
→ false

additional roots
→ empty
```

---

# 64. Tests — Permission Ceiling

Request:

```text
write trusted workspace
```

Implement:

```text
grant allowed
```

Review:

```text
grant rejected
```

Request:

```text
write C:\Windows
```

all current profiles:

```text
grant rejected
```

Request:

```text
network github.com
```

network-off profile:

```text
grant rejected
```

---

# 65. Tests — No Repair on Permission

Fixture runtime returns:

```text
NeedsDecision
detail PermissionBlocked
```

Expected:

```text
repair_attempts == 0
TaskState == NeedsDecision
DecisionNeeded event emitted
```

No RepairStarted.

---

# 66. Tests — Live Permission Round Trip

Fixture live runtime:

```text
PermissionRequested
↓
scheduler stores pending request
↓
UI/test calls resolve_permission
↓
runtime receives decision
↓
runtime emits Success
```

Verify same execution continues.

---

# 67. Tests — Denial

```text
PermissionRequested
↓
Deny
↓
provider returns declined/NeedsDecision
```

No repair.

No new execution ID unless user later retries.

---

# 68. Tests — Session Approval

Two file-change requests inside allowed workspace.

User chooses:

```text
ApproveForSession
```

Provider adapter/provider session should not require the same approval again when provider-native semantics handle that.

Relay must still enforce the frozen ceiling.

---

# 69. Tests — Scroll Result

Long Result body:

```text
100 lines
viewport 20
```

Test:

```text
Down
PageDown
End
Home
```

selected content changes appropriately.

Offset clamps.

---

# 70. Tests — Scroll Execution

Active execution appends lines.

Default:

```text
follow tail
```

User scrolls up:

```text
follow_tail false
```

new events:

```text
viewport does not jump
```

End:

```text
follow_tail true
```

---

# 71. Tests — Result Runtime Identity

Frozen execution:

```text
codex/implement
```

Global runtime later becomes:

```text
antigravity/implement
```

Result still shows:

```text
codex/implement
```

Never `codex/default` unless that was actually the frozen execution profile.

---

# 72. Manual Dogfood Acceptance

Use the current dogfood task again after correction.

Expected:

```text
Task Review

Runtime
  Codex / Implement

Permissions
  Workspace write   Allow

Effective
  Workspace write   Verified / honest Unknown
```

Press Run.

If provider asks:

```text
Permission modal appears in Relay
```

Choose:

```text
Allow for this run
```

Expected:

```text
provider continues same execution
files can be modified
```

If no live request is needed because the pre-approved workspace-write envelope is effective:

```text
provider edits successfully without extra prompt
```

Both are acceptable.

---

# 73. Manual Result Acceptance

After a long failed/successful result:

```text
Up / Down
PageUp / PageDown
Home / End
```

must work.

No text should be permanently hidden below the terminal viewport.

---

# 74. Manual Execution Acceptance

During long execution:

```text
PageUp
```

reads old activity.

New output continues arriving.

Viewport stays where user placed it.

Indicator:

```text
◌ paused · End to resume live
```

End resumes live following.

---

# 75. Failure UX Acceptance

Force a read-only permission mismatch.

Expected:

```text
NEEDS DECISION
```

not:

```text
FAILED / implementation_failure / Repairs: 2
```

Result explains:

```text
Workspace write permission is required.
```

---

# 76. Recommended Implementation Order

## Phase A — Immediate dogfood correctness

```text
A1 fix Result runtime-profile source
A2 add Result scrolling
A3 add Execution scrolling/follow-tail
A4 add permission envelope to Run policy
A5 ensure codex/implement actually launches workspace-write
A6 remove false write Verified
A7 map permission blocker to NeedsDecision
A8 do not spend repair attempts on permission blocker
```

Dogfood again.

## Phase B — Provider-neutral permission control plane

```text
B1 RuntimePermissionRequest
B2 RuntimeControlHandle
B3 RuntimeSession bidirectional control
B4 Core PermissionRequested/Resolved
B5 scheduler pending-permission state
B6 TUI permission modal
B7 ceiling validation
```

Use fixture runtime first.

## Phase C — Codex app-server

```text
C1 stdio JSON-RPC client
C2 initialization
C3 thread/start + turn/start
C4 streamed activity
C5 command approval
C6 file approval
C7 permission request
C8 outcomes
C9 cancellation
C10 account/runtime probe reuse
```

Dogfood.

## Phase D — consolidate

```text
D1 decide app-server vs exec fallback policy
D2 update Runtime Detail
D3 permission audit persistence
D4 docs
```

---

# 77. Recommended Commit Sequence

```text
1. fix[Jacinth]: classify runtime permission blockers without repair

2. feat[Jacinth]: make execution and result views scrollable

3. feat[Jacinth]: add provider-neutral runtime permission broker

4. feat[Jacinth]: surface execution permission decisions in Relay TUI

5. feat[Jacinth]: integrate Codex app-server approvals

6. test[Jacinth]: cover permission ceilings and live approval flow
```

---

# 78. Files Expected to Change

Likely:

```text
crates/relay-runtime/src/lib.rs
crates/relay-runtime/src/profile.rs
crates/relay-runtime/src/activity.rs

crates/relay-core/src/execution.rs

crates/relay-store/src/lib.rs
crates/relay-store/migrations/...   only if permission audit/runtime metadata needs schema

runtimes/relay-runtime-codex/src/lib.rs
runtimes/relay-runtime-codex/src/client.rs
runtimes/relay-runtime-codex/src/protocol.rs
runtimes/relay-runtime-codex/src/approvals.rs

apps/relay-tui/src/main.rs
apps/relay-tui/src/events.rs
apps/relay-tui/src/theme.rs

prefer new modules:
apps/relay-tui/src/scroll.rs
apps/relay-tui/src/permission_view.rs
```

Do not create all modules blindly.

Use them to reduce `main.rs` coupling when beneficial.

---

# 79. Quality Gates

Run:

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
git status --short
```

Normal tests must not consume live Codex usage.

App-server protocol tests use fixture JSON-RPC streams.

---

# 80. Hard Completion Gate

Do NOT call this complete if any are true:

```text
[ ] codex/implement can execute with effective read-only without preflight warning.
[ ] permission denial becomes implementation_failure.
[ ] permission denial consumes repair attempts.
[ ] provider permission prompt cannot be surfaced inside Relay.
[ ] Relay can grant outside-workspace write under current profiles.
[ ] Relay can grant network while profile says network off.
[ ] TUI talks directly to provider instead of scheduler/runtime boundary.
[ ] Result still reports AppRuntime::default() instead of actual execution profile.
[ ] Result content cannot scroll.
[ ] Execution history cannot scroll.
[ ] scrolling active execution always snaps back to bottom.
[ ] provider interactive TUI appears over Relay.
[ ] permission request blocks unrelated project execution.
[ ] Relay silently expands permission without human approval.
```

---

# 81. Final UX Target

Normal Run:

```text
Task Review

Codex / Implement
Workspace write  ✓
Network          ×

> Run
```

If no extra provider approval is needed:

```text
● Running
● Edit ...
✓ Verification
```

If provider needs an approval inside the already-approved envelope:

```text
! Permission requested

Codex wants to modify:
  apps/relay-tui/src/main.rs

> Allow once
  Allow for this run
  Deny
```

If it asks outside the envelope:

```text
! Permission exceeds profile

Requested
  Write C:\outside-project

Current profile
  Workspace only

> Deny
  Cancel execution
```

And a permission failure must end as:

```text
NEEDS DECISION
```

not:

```text
FAILED
Repairs: 2
```

The permanent rule is:

> **Relay approves capabilities; providers request them; the scheduler enforces the ceiling; the TUI makes the human decision visible and reversible.**
