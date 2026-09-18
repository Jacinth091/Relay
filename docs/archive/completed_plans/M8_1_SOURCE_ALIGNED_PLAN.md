# Relay — M8.1 Source-Aligned Multi-Runtime Orchestration + Terminal UX Implementation Plan

**Status:** DEV implementation/remediation plan  
**Date:** 2026-08-15  
**Current source baseline:** `459a438839c64d8863045270cab9e337cebb8068`  
**Repository:** `Jacinth091/Relay`  
**Knowledge authority:** `Jacinth091/relay-knowledge`  
**Purpose:** Replace the current partially-wired M8.1 implementation with a source-aligned design in which runtime switching is real, provider adapters are provider-native, Relay owns the terminal and execution envelope, and the command-first TUI matches the current UX guidance.

---

# 1. Executive Decision

The current M8.1 branch should **not** receive another surface-only patch.

The remaining problems are connected:

```text
runtime switching is cosmetic
        +
runtime profiles are mostly descriptive
        +
provider capability reports are not truthful
        +
Antigravity uses the wrong machine contract
        +
runtime activity is defined but not wired
        +
TUI still renders mostly plain legacy strings
        +
input/event loop is only partly event-driven
```

The correction should establish one coherent architecture:

```text
trusted Runtime Selection
        ↓
ExecutionRunPolicy frozen at local Run approval
        ↓
ONE M6 ExecutionScheduler
        ↓
RuntimeRouter
        ↓
provider-native runtime adapter
        ↓
RuntimeEvent::Activity + bounded raw output
        ↓
CoreExecutionEvent
        ↓
Relay-owned execution view
```

The key product rule remains:

> **Relay Core is provider-neutral. Runtime adapters are provider-native. Relay UX is provider-aware.**

The key terminal rule remains:

> **Relay owns the terminal. Provider runtimes are non-interactive workers.**

The key interaction rule remains:

> **Commands are primary. Keybinds are accelerators.**

---

# 2. Source Authorities Used for This Plan

Implementation must be checked against the actual source, not just this document.

Read first:

```text
relay-knowledge/
  10-DEVELOPMENT/00-DEVELOPMENT-CONTRACT.md
  10-DEVELOPMENT/reference/TERMINAL-UX-GUIDELINES.md
  10-DEVELOPMENT/modules/M8-codex-runtime.md
  30-REVIEW/M8-REVIEW.md

Relay/
  docs/adr/0019-multi-runtime-orchestration.md
  docs/RUNTIME_ORCHESTRATION_CONTEXT.md

  crates/relay-core/src/execution.rs
  crates/relay-core/src/project_registry.rs

  crates/relay-runtime/src/lib.rs
  crates/relay-runtime/src/profile.rs
  crates/relay-runtime/src/integration.rs
  crates/relay-runtime/src/router.rs
  crates/relay-runtime/src/activity.rs

  runtimes/relay-runtime-codex/src/lib.rs
  runtimes/relay-runtime-antigravity/src/lib.rs

  apps/relay-tui/src/main.rs
  apps/relay-tui/src/command.rs
  apps/relay-tui/src/events.rs
  apps/relay-tui/src/theme.rs

  crates/relay-store/src/lib.rs
  crates/relay-store/migrations/
```

Provider CLI behavior is temporally unstable.

Before finalizing either adapter, Dev must also re-check:

```text
installed CLI --help
installed CLI --version
current official provider documentation
```

The adapter must test the exact invocation Relay actually uses.

---

# 3. Current Baseline — What Is Correct and Must Be Preserved

The current branch already contains useful foundations.

Preserve:

```text
crates/relay-runtime/src/profile.rs
  RuntimeProfile
  EffectiveRuntimeProfile
  ReasoningEffort
  FilesystemCapability

crates/relay-runtime/src/integration.rs
  RuntimeIntegration
  RuntimeProbe
  RuntimeCapabilityReport
  CapabilityStatus

crates/relay-runtime/src/activity.rs
  RuntimeActivity
  FileChangeKind

crates/relay-runtime/src/router.rs
  provider registry concept

apps/relay-tui/src/command.rs
  fixed slash-command registry
  no implicit shell

apps/relay-tui/src/events.rs
  EventStream
  KeyEventKind filtering
  InputContext
  frame clock
  bulk Event::Paste

apps/relay-tui/src/theme.rs
  semantic palette
  semantic diff formatting

runtimes/*
  CREATE_NO_WINDOW on Windows
  piped stdin/stdout/stderr
  process-tree cancellation
  terminal-output sanitizer
```

Do not discard these and restart from scratch.

Correct their wiring and ownership.

---

# 4. Current Baseline — P0 Defects Found in Actual Source

These are not optional polish.

## P0-01 — Runtime switching is currently cosmetic

Current production scheduler is constructed with:

```rust
Arc::new(AppRuntime::default())
```

and production `AppRuntime::default()` is:

```rust
AppRuntime::Codex(CodexRuntime::default())
```

`/runtime use antigravity/implement` only mutates:

```text
active_runtime_provider
active_profile_id
```

It does not replace or route the scheduler runtime.

Therefore:

```text
UI says Antigravity
execution still uses Codex
```

This must be fixed before runtime switching can be considered implemented.

---

## P0-02 — The Core scheduler has no trusted per-run profile selection

`ExecutionScheduler::submit(task_id)` currently queues only:

```text
TaskId
ProjectKey
```

`run_task()` then calls:

```rust
runtime.identity()
```

and uses that one scheduler runtime identity to:

```text
write executions.runtime_profile
construct RuntimeRequest.profile_id
```

This makes per-run runtime/profile switching impossible without changing the queue metadata.

The correct fix is an **additive M8.1 execution-policy field**, not a second scheduler.

---

## P0-03 — `/diff <task>` and `/logs <task>` can execute a READY task

Current command handling uses:

```rust
state.run_task_selector(&task).await
```

for:

```text
/run
/diff
/logs
```

but `run_task_selector()` calls:

```rust
submit_selected_task().await
```

Therefore a read-only command such as:

```text
/diff <READY_TASK>
```

can submit execution.

This violates:

```text
IMPORT != EXECUTE
inspection != execute
explicit Run only
```

Fix immediately.

---

## P0-04 — Runtime profiles are descriptive but not authoritative

Example:

```text
codex/review
codex/implement
codex/deep
```

resolve to different:

```text
filesystem
reasoning
duration
repair budget
```

but the Codex command builder still always uses:

```text
workspace-write
network=false
internal CodexRuntimeConfig.model_override
```

rather than the selected `EffectiveRuntimeProfile`.

The same problem exists in Antigravity.

A profile shown in the TUI must be the profile the adapter actually executes.

---

## P0-05 — Capability verification reports false certainty

Both real adapters currently treat:

```text
working_directory.is_dir()
```

as enough to report:

```text
filesystem_write = Verified
```

That is false.

Directory existence only proves Relay found a directory.

It does not prove the provider established effective write capability.

---

## P0-06 — Antigravity machine invocation is still incorrect

Current adapter remains shaped like Codex:

```text
agy --headless exec --json ...
```

Current Antigravity CLI provides a provider-native non-interactive **print mode**:

```text
-p / --print
```

with machine-readable:

```text
--output-format stream-json
```

and structured final-result validation through:

```text
--json-schema
```

The current official CLI also exposes provider-native:

```text
--model
--effort
--mode
--sandbox
models
```

The adapter must be rebuilt around the actual Antigravity machine interface, not a copied Codex contract.

---

# 5. Non-Negotiable Accepted Invariants

The implementation may add plumbing needed by M8.1, but must not change these semantics.

## Project trust

```text
HandoffTask project key
        ↓
Project Registry
        ↓
trusted local path
```

A task never supplies its local execution directory.

## Repository safety

M3 repository eligibility remains mandatory before execution.

## Import vs Run

```text
IMPORT != EXECUTE
```

Only explicit local Run approval submits work.

## Persistence

SQLite remains canonical for durable lifecycle state.

## Scheduler

One M6 scheduler remains authoritative for:

```text
global concurrency
per-project concurrency
queueing
cancellation
repair loop
deadline
cross-project isolation
```

Do not create one scheduler per provider.

## Runtime process

Provider execution:

```text
explicit executable
+
explicit argument array
+
trusted cwd
```

No untrusted shell command construction.

## Repository mutation policy

```text
automatic commit = OFF
automatic push   = OFF
remote mutation  = OFF
```

unless explicitly approved in a future milestone.

---

# 6. Target Runtime Architecture

Implement this exact ownership model:

```text
                         TUI / App composition
                                  │
                 resolve trusted runtime selection
                                  │
                                  ↓
                        ExecutionRunPolicy
                     frozen before submit
                                  │
                                  ↓
                    ExecutionScheduler<RuntimeRouter>
                                  │
                 queue/concurrency remain M6
                                  │
                                  ↓
                           RuntimeRouter
                    resolve request.profile_id
                     ┌────────────┴────────────┐
                     ↓                         ↓
                CodexRuntime            AntigravityRuntime
                     │                         │
              Codex-native CLI          AGY-native CLI
                     │                         │
                     └────────────┬────────────┘
                                  ↓
                         RuntimeEvent stream
                                  ↓
                        CoreExecutionEvent
                                  ↓
                         Relay execution UI
```

The scheduler should no longer be rebuilt merely because the default runtime changes.

A runtime switch affects **future queued executions**, not the runtime object owned by the scheduler.

---

# 7. Add an Immutable Per-Run Execution Policy

Add a provider-neutral policy in `relay-core` or a Core-owned execution module.

Recommended:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRunPolicy {
    pub runtime_profile_id: String,
    pub max_duration: Duration,
    pub max_repair_attempts: u32,
}
```

This contains only information Core needs to enforce the run.

Do not put:

```text
Codex flags
Antigravity mode strings
provider credentials
provider settings JSON
```

inside Core.

---

# 8. Extend Scheduler Submission Additively

Preserve existing API for M6/Fake Runtime compatibility:

```rust
pub async fn submit(&self, task_id: TaskId)
```

Add:

```rust
pub async fn submit_with_policy(
    &self,
    task_id: TaskId,
    policy: ExecutionRunPolicy,
) -> Result<(), ExecutionSchedulerError>
```

Legacy `submit()` can internally create a policy from:

```text
runtime.identity().profile_id
scheduler hard limits
```

so M6 tests remain valid.

Production TUI must use:

```text
submit_with_policy(...)
```

---

# 9. Queue the Policy With the Task

Change internal:

```rust
QueuedExecution {
    task_id,
    project_key,
}
```

to:

```rust
QueuedExecution {
    task_id,
    project_key,
    run_policy,
}
```

Once queued:

```text
profile ID
duration
repair budget
```

are immutable for that execution.

Changing `/runtime use ...` after queueing must not alter queued or active tasks.

---

# 10. Preserve Scheduler Hard Ceilings

`ExecutionLimits` remains the M6 scheduler safety ceiling.

Interpret:

```text
ExecutionLimits.max_duration
ExecutionLimits.max_repair_attempts
```

as hard maxima.

`ExecutionRunPolicy` may request values **at or below** those ceilings.

Validate on submit:

```text
run_policy.max_duration <= scheduler.max_duration
run_policy.max_repair_attempts <= scheduler.max_repair_attempts
```

If a profile exceeds the hard ceiling:

```text
fail submission
```

or deterministically clamp only if the UI clearly displays the effective clamped value.

Prefer fail/validate during profile resolution so Task Review shows the true effective envelope.

---

# 11. Rename the Production Limit Concept

Current TUI contains Codex-specific:

```text
CODEX_DEFAULT_MAX_DURATION_SECS
CODEX_MAX_DURATION_SECS
RELAY_CODEX_MAX_DURATION_SECS
production_codex_execution_limits()
```

Once multiple runtimes are real, make scheduler ceilings provider-neutral.

Recommended:

```text
RELAY_MAX_EXECUTION_DURATION_SECS
```

with a hard upper bound.

Keep `RELAY_CODEX_MAX_DURATION_SECS` only as a temporary compatibility alias if existing local users depend on it.

Built-in profile defaults remain provider/profile-specific:

```text
Review      20m
Implement   30m
Deep        60m
```

The scheduler ceiling must be at least large enough for the largest enabled built-in profile.

---

# 12. Use Run Policy in `run_task()`

Remove use of scheduler runtime identity as the selected execution profile.

Current:

```text
identity = runtime.identity()
runtime_profile = identity.profile_id
request.profile_id = identity.profile_id
```

Replace with:

```text
runtime_profile = queued.run_policy.runtime_profile_id
request.profile_id = queued.run_policy.runtime_profile_id
deadline = queued.run_policy.max_duration
repair budget = queued.run_policy.max_repair_attempts
```

Still enforce the global hard ceiling.

Persist:

```text
executions.runtime_profile
```

from the frozen run policy.

---

# 13. Make `RuntimeRouter` the Real Runtime

`RuntimeRouter` currently exists only as a registry.

Make it implement:

```rust
Runtime
```

Execution dispatch:

```rust
fn start(
    &self,
    request: RuntimeRequest,
    cancellation: CancellationToken,
) -> RuntimeStartFuture
```

should:

```text
parse provider from request.profile_id
        ↓
lookup registered runtime
        ↓
fail closed if unknown
        ↓
delegate runtime.start(request, cancellation)
```

Do not choose based on:

```text
active TUI screen
global mutable default
task title
provider output
```

The request already contains the frozen trusted profile.

---

# 14. Harden Runtime Router Registration

Current `register()` silently accepts an arbitrary provider key.

Change registration to validate consistency:

```text
registered provider ID
runtime.identity().runtime_id
integration.descriptor().provider_id
```

must agree where meaningful.

Return:

```rust
Result<(), RuntimeRouterError>
```

instead of silently inserting inconsistent entries.

Also make provider listing deterministic:

```text
sorted by provider_id/display name
```

rather than raw `HashMap` iteration order.

---

# 15. Production TUI Scheduler Composition

Remove production dependence on:

```rust
AppRuntime::default() == Codex
```

Target production composition:

```rust
let router = build_runtime_router(runtime_settings)?;
let scheduler = ExecutionScheduler::new(
    relay.clone(),
    store.clone(),
    Arc::new(router),
    production_execution_limits(),
);
```

The scheduler is created once.

`/runtime use ...` must not require scheduler rebind just to select a different provider for future runs.

---

# 16. Test Composition Must Never Invoke Live Providers

Normal tests must not call Codex or Antigravity.

Options:

```text
A. test-only RuntimeRouter containing deterministic fixture runtimes
B. a fake runtime registered under test provider IDs
C. retain isolated M6 FakeRuntime scheduler tests separately
```

Preferred:

create small test adapters implementing `Runtime` with identities:

```text
codex
antigravity
```

but delegating deterministic fake behavior.

Then test:

```text
codex/implement     → codex fixture
antigravity/implement → antigravity fixture
```

without real usage.

---

# 17. Runtime Selection Must Be a Pure Trusted-Local Resolution Step

Introduce a pure resolver at the application/composition layer.

Conceptual input:

```rust
RuntimeSelectionInput {
    explicit_one_run_profile: Option<String>,
    project_profile: Option<String>,
    legacy_project_stack: Option<String>,
    global_default_profile: String,
    task_requested_stack: Option<String>,
}
```

Output:

```rust
ResolvedRuntimeSelection {
    profile: EffectiveRuntimeProfile,
    source: RuntimeSelectionSource,
    task_stack_note: Option<...>,
}
```

---

# 18. Selection Precedence

Use the ADR precedence:

```text
1. explicit one-run local selection
2. trusted project runtime-profile default
3. legacy TrustedProject.default_stack mapping
4. Relay global default runtime profile
5. fail closed
```

The task's requested stack is **not** a privilege source.

It is a compatibility/request signal evaluated after trusted selection.

---

# 19. Requested Stack Compatibility

Do not keep provider-specific stack rejection inside each adapter.

Instead resolve centrally.

Suggested aliases:

```text
chatgpt-codex     → codex
codex             → codex

antigravity       → antigravity
agy               → antigravity
```

Rules:

```text
requested stack absent
  → selected trusted runtime allowed

requested stack matches selected provider
  → allowed

requested stack differs and selection came from global/project default
  → block with actionable mismatch

requested stack differs and user explicitly chose a one-run override
  → allow only after Task Review visibly shows:
       Requested: <stack>
       Local override: <profile>
```

The human owns local execution choice.

The remote task cannot silently force the runtime.

---

# 20. Runtime Profile Must Actually Drive Adapter Configuration

Every adapter `start()` must begin by:

```text
resolve request.profile_id
        ↓
validate provider matches adapter
        ↓
obtain EffectiveRuntimeProfile
        ↓
translate profile into provider-native command/config
```

Never execute using a hard-coded "implement" command while displaying "review" or "deep".

---

# 21. Keep Provider Profiles Small

Initial built-ins:

```text
<provider>/review
<provider>/implement
<provider>/deep
```

Common Relay meaning:

## Review

```text
read-oriented
lower/medium effort
shorter duration
small repair budget
no commit/push
```

## Implement

```text
workspace edits
higher effort
bounded duration
normal repair budget
network off by default
no extra write roots
no commit/push
```

## Deep

```text
workspace edits
higher reasoning/effort
longer bounded duration
larger but bounded repair budget
network still off by default
```

Do not add a "full machine access" profile.

---

# 22. Runtime Settings Must Not Break the M2 Project Config

Current M2 `ProjectRegistry` TOML uses:

```rust
#[serde(deny_unknown_fields)]
```

at the top-level configuration structure.

Adding:

```toml
[runtime]
...
```

to the same file would make current M2 parsing fail unless Project Registry is redesigned.

Do **not** casually change the accepted M2 config schema for this feature.

---

# 23. Add a Separate Trusted Runtime Settings File

Recommended platform path:

```text
Windows
%APPDATA%\Relay\runtimes.toml

XDG
$XDG_CONFIG_HOME/relay/runtimes.toml

fallback
~/.config/relay/runtimes.toml
```

Environment override:

```text
RELAY_RUNTIME_CONFIG_PATH
```

This is trusted local configuration.

No second database is required.

---

# 24. Runtime Settings v1 Schema

Keep v1 intentionally small.

Example:

```toml
version = 1

[runtime]
default_profile = "codex/implement"

[projects]
relay = "antigravity/implement"

[providers.codex]
executable = "codex"

[providers.antigravity]
executable = "agy"

[profile_overrides."codex/implement"]
model = "default"
reasoning_effort = "high"
max_duration_secs = 1800

[profile_overrides."antigravity/implement"]
model = "default"
reasoning_effort = "high"
max_duration_secs = 1800
```

For v1, allow only non-boundary overrides such as:

```text
model
reasoning/effort
duration within hard ceiling
repair budget within hard ceiling
```

Do not allow this initial settings file to arbitrarily grant:

```text
network
extra writable roots
full machine access
auto commit
auto push
```

Those need a separate explicit safety decision.

---

# 25. Runtime Settings Ownership

Recommended code:

```text
apps/relay-tui/src/runtime_settings.rs
```

This module owns:

```text
platform path
load/save
schema version
global default
project override
provider executable overrides
safe profile overrides
validation
```

Provider-neutral profile types remain in `relay-runtime`.

Provider CLI details remain in provider crates.

---

# 26. `/runtime use` Semantics

Define clearly:

```text
/runtime use <profile>
```

means:

> Set the Relay **global default profile for future Run approvals** and persist it to trusted local runtime settings.

It does not:

```text
change active executions
change already queued executions
mutate HandoffTask
rewrite project defaults
```

After success:

```text
Default runtime changed to Antigravity / Implement.
Existing queued/running work is unchanged.
```

---

# 27. Per-Task One-Run Override

Task Review needs an ephemeral override.

Add to AppState:

```rust
task_profile_overrides: HashMap<TaskId, String>
```

or a selected-task review state.

The override is consumed/frozen into `ExecutionRunPolicy` when the user presses Run.

It does not mutate the task.

It does not become the project/global default.

---

# 28. Project Runtime Default

Use the separate runtime settings file:

```toml
[projects]
relay = "codex/implement"
practicore = "antigravity/implement"
```

The existing M2 `default_stack` remains supported as a compatibility fallback.

Do not repurpose project `default_stack` into a complex provider profile object.

---

# 29. Fix `/diff` and `/logs` Before Anything Else

Split the current helper.

Add pure:

```rust
fn resolve_task_selector(&self, selector: &str) -> Result<TaskId, TaskSelectorError>
```

Add:

```rust
fn select_task_by_selector(&mut self, selector: &str) -> Result<TaskId, ...>
```

Then:

```text
/run
  resolve selector
  select task
  build/confirm ExecutionRunPolicy
  submit_with_policy

/diff
  resolve selector
  select task
  READ-ONLY git inspection

/logs
  resolve selector
  select task
  READ-ONLY log view
```

`/diff` and `/logs` must never call:

```text
submit
submit_with_policy
submit_selected_task
```

---

# 30. Add a Regression for the Read-Only Command Boundary

Test:

```text
READY task exists
scheduler queue empty

/diff <task>
→ task remains READY
→ scheduler queue remains empty
→ no ExecutionQueued event

/logs <task>
→ task remains READY
→ scheduler queue remains empty
→ no ExecutionQueued event

/run <task>
→ exactly one execution queued
```

This is a release gate.

---

# 31. Codex Adapter — Use the Selected Effective Profile

Current Codex command builder always requests workspace-write.

Refactor:

```rust
command_spec(
    request,
    effective_profile,
    schema_path,
    result_path,
)
```

Map:

```text
FilesystemCapability::ReadOnly
  → current supported Codex read-only sandbox

FilesystemCapability::WorkspaceWrite
  → workspace-write
  → no extra writable roots
```

Network:

```text
profile.network == false
→ Relay-controlled effective Codex network disabled
```

Do not permit extra roots in current built-ins.

---

# 32. Codex Model Catalog

Remove hard-coded stale examples such as:

```text
o3
o3-mini
```

as the permanent catalog.

Model availability changes.

Preferred approach:

```text
1. provider-supported machine-readable model discovery, if current CLI supports it
2. otherwise "Default" only
3. optionally accept a trusted custom local model override after validation
```

`Default` means:

```text
do not pass --model
```

and let the installed Codex/account choose.

Never let task text set the model.

---

# 33. Codex Reasoning/Effort Translation

`RuntimeProfile.reasoning_effort` must map through a provider-native, currently supported Codex configuration mechanism.

Do not assume a flag from memory.

Implementation process:

```text
inspect current codex exec --help
inspect current official Codex CLI config docs
build mapping
add argv/config regression
```

If selected model does not support the requested effort:

```text
profile resolution fails before Run
```

or show:

```text
Configured: High
Effective: Unknown/Unsupported
```

and block if required.

---

# 34. Codex Capability Verification

Delete:

```text
directory exists → workspace write Verified
```

Initial safe behavior:

```text
filesystem_read
  Configured / Verified only when provider contract supports certainty

filesystem_write
  Configured when profile requests write
  Verified only by provider-supported evidence
  Unknown otherwise

command_execution
  Configured / Verified based on provider contract/probe

network
  Configured disabled
```

A conservative `Unknown` is acceptable.

False `Verified` is not.

---

# 35. Antigravity Adapter — Replace the Existing Invocation

Current adapter must stop using:

```text
--headless exec --json --output-last-message
```

Build around current provider-native print mode.

Conceptual required capabilities:

```text
agy -p / --print
--cwd <trusted repository>
--output-format stream-json
--json-schema <Relay schema>
--sandbox
--model <provider model> when explicitly selected
--effort <provider effort> when selected
--mode=<provider mode>
```

Final exact argument order and prompt transport must be generated from:

```text
installed agy --help
official current docs
```

and locked with tests.

---

# 36. Antigravity Prompt Transport — Important Windows Rule

Antigravity's documented print mode commonly accepts the prompt as an argument.

Relay currently supports `.cmd` launchers through:

```text
cmd.exe /D /C call ...
```

If untrusted task text is placed in argv behind `cmd.exe`, shell metacharacters become a safety concern.

Therefore:

```text
if AGY supports stdin prompt transport
  → prefer stdin

else if native agy.exe is available
  → passing prompt as one direct Command argument is acceptable

else if only agy.cmd is available and prompt must be argv
  → do NOT proceed until argument-boundary safety is proven against cmd.exe
  → prefer fail closed over shell-interpreting task text
```

Add adversarial tests for:

```text
&
|
>
<
^
%
"
$(...)
; 
newlines
```

No task content may become a second shell command.

---

# 37. Antigravity Safety Settings

Current Antigravity print mode honors persisted provider settings.

Relevant provider safety state includes:

```text
toolPermission
artifactReviewPolicy
allowNonWorkspaceAccess
enableTerminalSandbox
permissions rules
agent mode
```

Relay must not blindly inherit a configuration that broadens its approved envelope.

For initial M8.1:

```text
--sandbox
  required for profiles that execute commands

allowNonWorkspaceAccess
  must be false/effectively disabled

extra workspace directories
  none

dangerously skip permissions
  NEVER

provider interactive approval
  must not appear over Relay
```

If Relay cannot guarantee a required boundary through documented overrides + validated settings:

```text
RuntimeProbe.compatible = false
or
CapabilityStatus::Mismatch / Unavailable
```

and Run is blocked.

---

# 38. Do Not Rewrite the User's Provider Settings Automatically

Relay may inspect documented provider safety keys.

Relay must not silently:

```text
change ~/.gemini/antigravity-cli/settings.json
set always-proceed
disable security
modify credentials
```

If the user's provider settings prevent non-interactive operation, Runtime Detail should say exactly what prerequisite is unmet.

Example:

```text
Antigravity / Implement unavailable

Required
  Terminal sandbox         On
  Outside workspace        Denied

Current
  Outside workspace        Allowed ×

Fix the Antigravity safety setting, then run:
/runtime test antigravity
```

---

# 39. Antigravity Modes

Provider-native mapping should be explicit.

Potential mapping, subject to current CLI verification:

```text
antigravity/implement
  mode = accept-edits

antigravity/deep
  mode = accept-edits
  higher effort

antigravity/review
  mode = plan or another provider-native non-writing mode
```

Do not claim Review is read-only merely because `mode=plan` sounds read-only.

The effective capability report must prove or conservatively report the boundary.

---

# 40. Antigravity Structured Stream

Parse current `stream-json` provider events.

Provider stream currently supplies typed events including:

```text
init
step_update
result
```

Translate into Relay activity.

Examples:

```text
init
  → RuntimeActivity::SessionStarted

step_update tool call
  → CommandStarted / CommandProgress / FileChanged / AgentMessage

terminal result
  → structured Relay outcome validation
```

Unknown well-formed event variants:

```text
tolerate / diagnostic
```

Malformed NDJSON:

```text
InvalidOutput
```

Do not dump provider JSON directly into normal UI.

---

# 41. Antigravity Model Discovery

Use provider-native:

```text
agy models
```

only if the installed CLI supports a parseable stable form.

If model output is human-only and unstable:

```text
Default only
```

is acceptable for v0.1.

Do not hard-code guessed future model IDs.

---

# 42. Runtime Account Identity

`RuntimeProbe.account_identity` is optional by design.

Populate only when a supported machine-readable provider command exposes it.

Do not:

```text
scrape provider TUI
read tokens
read browser sessions
read OS keyring secrets
parse credential files
```

If unavailable:

```text
Authentication   ✓ Ready
Account          identity unavailable
```

is correct.

---

# 43. Add Runtime Activity to the Actual Event Stream

`RuntimeActivity` currently exists but is unused.

Extend:

```rust
pub enum RuntimeEvent {
    Activity(RuntimeActivity),
    Output { ... },
    Outcome(...),
    InvalidOutput { ... },
}
```

Raw `Output` remains for bounded logs.

`Activity` becomes the primary execution presentation input.

---

# 44. Forward Runtime Activity Through Core

Add:

```rust
CoreExecutionEvent::RuntimeActivity {
    task_id: TaskId,
    execution_id: ExecutionId,
    activity: RuntimeActivity,
}
```

`collect_runtime_events()` should:

```text
RuntimeEvent::Activity
  → send CoreExecutionEvent::RuntimeActivity

RuntimeEvent::Output
  → bounded raw log/output path

RuntimeEvent::Outcome
  → existing lifecycle handling
```

Lifecycle authority remains unchanged.

Activity cannot set TaskState directly.

---

# 45. TUI Execution View Model

Expand:

```rust
ExecutionViewState
```

to hold:

```rust
activity: VecDeque<RuntimeActivity>
activity_count/byte bound
current_activity
changed_files_snapshot
verification summaries
raw output/log buffer
```

Suggested bounds:

```text
recent activity items     100–200
raw log                   existing 16 KiB or another explicit bound
per-activity text         bounded
```

Do not retain unlimited agent output.

---

# 46. Provider Activity Is Not Repository Truth

`RuntimeActivity::FileChanged` is a hint.

Canonical file-change display comes from trusted local Git state.

At safe transition points:

```text
ExecutionStarted
runtime FileChanged activity
VerificationStarted
ExecutionFinished
```

refresh a read-only repository status snapshot.

Provider event:

```text
"Modified src/lib.rs"
```

does not override:

```text
git status
```

---

# 47. Safe Git Inspection Helper

Extract TUI-local read-only Git inspection to:

```text
apps/relay-tui/src/git_views.rs
```

Use direct `git` executable args.

Support:

```text
git status --porcelain=v1 -z
git diff --no-ext-diff
git diff --cached --no-ext-diff
git diff --stat
```

No shell.

No Git mutation.

Untracked files should be visible in file status even if patch rendering for untracked files is deferred.

---

# 48. Diff View Must Be Read-Only

`/diff [task]`:

```text
resolve task
resolve trusted project
run read-only git inspection
open Diff
```

It must never:

```text
queue
run
cancel
stage
reset
checkout
commit
push
```

The same applies to `/logs`.

---

# 49. Execution Metadata Persistence

Current schema already stores:

```text
executions.runtime_profile
```

Once run-policy plumbing is fixed, this records the correct selected profile.

For historical fidelity, add an additive migration:

```text
0002_execution_runtime_metadata.sql
```

Recommended nullable column:

```sql
ALTER TABLE executions
ADD COLUMN runtime_metadata_json TEXT;
```

Store a bounded safe snapshot such as:

```json
{
  "provider": "antigravity",
  "profile": "antigravity/implement",
  "model": "default",
  "reasoning_effort": "high",
  "filesystem": "workspace_write",
  "network": false,
  "max_duration_secs": 1800,
  "max_repair_attempts": 2,
  "runtime_version": "..."
}
```

Do not store:

```text
access tokens
account credentials
raw environment
unbounded logs
provider auth files
```

Existing rows remain valid with `NULL`.

Do not rewrite migration `0001`.

---

# 50. Runtime Health State in TUI Should Become Provider-Neutral

Current AppState stores separate:

```text
CodexRuntimeHealth
AntigravityRuntimeHealth
```

This does not scale.

Replace TUI-facing runtime health state with:

```rust
enum RuntimeProbeState {
    NotChecked,
    Checking,
    Ready(RuntimeProbe),
    Failed(String),
}
```

stored:

```rust
BTreeMap<String, RuntimeProbeState>
```

The router's `RuntimeIntegration` objects provide probes.

Provider-specific details can remain in `RuntimeProbe.notes`.

---

# 51. Runtime Probe Scheduling

Startup:

```text
probe selected/default provider only
```

`/runtimes`:

```text
probe all providers concurrently, bounded
```

`/runtime test codex`:

```text
probe Codex only
```

`/runtime test antigravity`:

```text
probe Antigravity only
```

Current behavior ignores the provider argument and triggers a full probe.

Fix it.

Do not probe every render frame.

---

# 52. Header Must Reflect the Selected Provider Only

Current header can show an active marker if **either** provider is ready.

That is misleading.

Header:

```text
Relay                         Antigravity ● Implement
```

must derive health from:

```text
active selected/default provider
```

If Codex is selected and unavailable while Antigravity is healthy:

```text
Codex × Not installed
```

not:

```text
Codex ●
```

---

# 53. Remove Duplicate Product Header From Home Content

The outer renderer already renders:

```text
Relay <runtime>
```

`activity_home()` currently also starts with another `Relay ...` line.

Home content should begin with activity, not a second header.

---

# 54. Runtime Dashboard View Model

Create a real view model.

Recommended:

```rust
RuntimeListItem {
    provider_id,
    display_name,
    probe_state,
    active_profile,
    model_label,
    effort_label,
    is_global_default,
}
```

Render:

```text
Runtimes

> Codex CLI
  ● Ready
  Profile        Codex / Implement
  Model          Default
  Reasoning      High

  Antigravity
  ! Needs configuration
  Profile        Antigravity / Implement
  Model          Default
  Effort         High

Default
  Codex / Implement
```

Use semantic styled spans.

---

# 55. Runtime Detail Must Use Real Capability Reports

Remove hard-coded:

```text
Workspace read: ✓
Workspace write: ✓
Commands: ✓
```

Build detail from:

```text
RuntimeProbe
EffectiveRuntimeProfile
RuntimeCapabilityReport
```

Example:

```text
Runtime / Antigravity

Status
  Installation       ✓
  Authentication     ✓
  Compatibility      ✓
  Account            unavailable

Profile
  Antigravity / Implement

AI
  Model              Default
  Effort             High

Capabilities
  Workspace read     ✓ Verified
  Workspace write    ? Unknown
  Commands           ● Configured
  Network            – Disabled
  Extra write roots  none

Limits
  Duration           30m
  Repairs            2
```

Never show a success checkmark for `Unknown`.

---

# 56. Task Review Must Be the Execution Envelope

Before Run:

```text
Task

ID           81a7...
Project      relay
State        READY
Repository   ✓ eligible

Task request
Stack        chatgpt-codex

Execution
Profile      Antigravity / Implement
Selection    one-run local override

AI
Model        Default
Effort       High

Capabilities
Read workspace       ✓ Verified
Edit workspace       ? Unknown
Run commands         ● Configured
Network              – Off
Extra write roots    none
Commit               – Off
Push                 – Off

Limits
Duration             30m
Repairs              2

> Run
  Change runtime/profile
  Details
  Back
```

If selected profile conflicts with requested stack:

```text
! Task requested Codex.
  You selected Antigravity for this Run.
```

Require explicit Run after showing it.

---

# 57. Command Semantics Cleanup

Keep:

```text
/home
/new
/tasks
/projects
/project add [path]
/runtimes
/runtime <provider>
/runtime use <profile>
/runtime test <provider>
/settings
/doctor
/run <task>
/cancel <task>
/diff [task]
/logs [task]
/help
/quit
```

Reconsider `/runtime profile <profile>`.

If it does not have a unique clear semantic, remove it rather than retaining overlapping commands.

A small command language is preferable.

---

# 58. Validate Runtime Commands Before Mutating State

Current `/runtime use anything` can set arbitrary strings.

Required:

```text
/runtime use <profile>
  → router/integration resolves profile
  → provider exists
  → profile valid
  → persist setting
  → update AppState
```

Unknown:

```text
/runtime use foo/bar
```

returns:

```text
Unknown runtime profile 'foo/bar'.
Try /runtimes.
```

No state mutation.

---

# 59. Project Add Needs Explicit Modes

Current discovered flow still shares the manual form state.

Create:

```rust
enum ProjectAddMode {
    Detecting,
    Candidate,
    NoRepository,
    Advanced,
}
```

Candidate actions:

```text
Add this repository
Edit details
Choose another directory
Advanced setup
Cancel
```

NoRepository actions:

```text
Choose another directory
Advanced setup
Cancel
```

Only `Advanced` shows the six-field form.

---

# 60. Detection Failure Must Not Auto-Enter the Manual Form

Current code calls:

```rust
start_add_project()
```

on discovery failure.

Change to:

```text
No Git repository detected

C:\selected\path

> Choose another directory
  Advanced setup
  Cancel
```

The manual form is opt-in.

---

# 61. Project Default Runtime in Discovery

Current discovered project suggests:

```text
chatgpt-codex
```

for legacy `default_stack`.

Keep that value only for M2 compatibility if desired.

Separately show:

```text
Runtime profile
  <resolved global default>
```

and allow the user to set a project runtime default in the runtime settings layer.

Do not make the M2 stack field carry all M8.1 profile semantics.

---

# 62. TUI Structural Refactor

`main.rs` is currently responsible for too many concerns.

Refactor incrementally after P0 correctness.

Target:

```text
apps/relay-tui/src/

main.rs
  terminal bootstrap only

app.rs
  AppState
  command dispatch
  screen transitions

command.rs
  RelayCommand parser/registry

events.rs
  EventStream
  input normalization
  liveness/frame loop

runtime_settings.rs
  trusted local runtime settings

runtime_manager.rs
  router composition
  probe orchestration
  profile selection

project_discovery.rs
  Git discovery

git_views.rs
  read-only status/diff

theme.rs
  semantic palette

view_model/
  home.rs
  tasks.rs
  projects.rs
  runtimes.rs
  task_review.rs
  execution.rs
  result.rs

ui/
  shell.rs
  home.rs
  tasks.rs
  projects.rs
  project_add.rs
  runtimes.rs
  runtime_detail.rs
  settings.rs
  doctor.rs
  task_review.rs
  execution.rs
  result.rs
  diff.rs
  logs.rs
```

Do not split just to inflate file count.

The important separation is:

```text
domain state
command semantics
provider management
view model
rendering
event pump
```

---

# 63. Semantic Theme Must Be Used, Not Merely Defined

Current `Theme` is a good foundation.

Convert normal screens from large plain Strings to:

```text
Vec<Line>
Span::styled
```

Use semantic roles:

```text
accent
success
warning
error
muted
text
selection
diff_added
diff_removed
```

---

# 64. Required Styled Screens

Apply theme to:

```text
Header
Home
Tasks
Projects
Project Candidate
Runtimes
Runtime Detail
Settings
Doctor
Task Review
Execution
Result
Diff
Logs warnings/errors
Command prompt
Command suggestions
```

Examples:

```text
Running              accent
Ready / Done         success
Waiting              warning/muted
Needs Decision       warning
Needs Clarification  warning
Failed               error
Durability Failure   error
Unknown capability   warning
Verified capability  success
Selected item        selection
IDs/paths/time        muted
```

Color never replaces text.

---

# 65. Activity Home

Target:

```text
Relay                              Codex ● Implement

2 active · 1 waiting · 1 attention

● relay
  M8.1 Runtime routing
  Codex / Implement · RUNNING · 02:14

● practicore
  Placement validation
  Antigravity / Implement · VERIFYING · 01:03

◌ relay
  Runtime docs
  WAITING · project slot

! practicore
  Migration redesign
  NEEDS DECISION

────────────────────────────────────────
> /_
```

No zero-counter wall.

No duplicate Relay heading.

No giant border.

---

# 66. Execution Screen Must Use RuntimeActivity

Target:

```text
relay · Runtime routing

Antigravity / Implement
Default model · High · Write · Offline

● RUNNING                                  02:14
  Attempt 1/2

Current
  ● Running cargo test --workspace

Activity
  ✓ Inspected repository
  ✓ Modified runtime router
  ● Running cargo test --workspace

Files
  M crates/relay-runtime/src/router.rs
  M apps/relay-tui/src/app.rs

Verification
  ● cargo test --workspace

────────────────────────────────────────
> /_

/cancel <task>   /diff   /logs
```

Normal view is not raw stdout.

---

# 67. Logs Remain Diagnostic

`/logs` contains bounded:

```text
stdout
stderr
provider diagnostics
actual wrapper details when needed
```

Sanitize terminal control sequences before rendering.

Do not write provider output directly to terminal.

---

# 68. Result Screen

Target:

```text
RESULT — SUCCESS

Task
  Runtime routing

Runtime
  Antigravity / Implement
  Default · High

Files changed
  M router.rs
  M app.rs

Changes
  +82  -31

Verification
  ✓ cargo fmt --check
  ✓ cargo test --workspace

Repairs
  0

Repository
  modified
  no commit
  no push

Summary
  Added trusted per-run routing through the RuntimeRouter.

> /diff
  /logs
  /tasks
```

Failure should emphasize:

```text
blocking capability
configured
effective
repository effects
```

---

# 69. Finish the Input/Event Loop Properly

Current key-kind filtering is good.

Keep:

```text
Release ignored
Press dispatched
printable Repeat suppressed in text contexts
navigation/delete Repeat controlled
```

Do not add time-based text debounce.

---

# 70. Add Bounded Ready-Event Draining

Current loop handles one `reader.next()` event per select branch.

Implement a bounded drain after receiving one event.

Conceptual:

```text
receive first terminal event
        ↓
handle
        ↓
drain immediately-ready events
        ↓
max 128 events or ~1–2 ms
        ↓
return to select
        ↓
draw at frame tick
```

Use a hard bound so runtime/scheduler work is not starved.

---

# 71. Decouple `state.tick()` From Every Key

Current:

```text
each loop iteration
→ poll health
→ tick scheduler
→ wait for next event
```

Rapid keys therefore repeatedly invoke application polling.

Use separate timers:

```text
frame tick       ~33 ms
liveness tick    ~50–100 ms
elapsed tick     1 s
```

Liveness tick calls the accepted M7 scheduler maintenance.

Do not make keyboard throughput depend on scheduler snapshot I/O.

---

# 72. Preserve M7 Scheduler Liveness

Whatever event-loop refactor is used, retain:

```text
queued work exists
AND scheduler driver absent
AND rebind not pending
→ start scheduler driver
```

Add a regression test after event-loop refactor.

---

# 73. `tick()` Should Report Whether State Changed

Recommended:

```rust
async fn tick(&mut self) -> TickOutcome
```

or:

```rust
async fn tick(&mut self) -> bool
```

so the event loop marks:

```text
dirty = true
```

only when application state changed.

Do not redraw continuously during idle.

---

# 74. Fix `has_active_execution()`

Current implementation considers:

```text
Screen::Execution
```

an active execution.

A user merely looking at an old Execution screen should not force perpetual redraw.

Use actual scheduler/execution activity:

```text
scheduler_driver.is_some()
or active scheduler snapshot exists
```

---

# 75. Input Trace Must Not Use `eprintln!`

Current developer input trace writes directly to stderr while Relay owns alternate-screen/raw terminal state.

Remove it or replace with a safe diagnostic sink.

Preferred for now:

```text
remove RELAY_TUI_INPUT_TRACE
```

unless Dev needs it.

If retained:

```text
bounded in-memory metadata ring
```

or asynchronous file logging.

Never log actual typed/pasted task text.

---

# 76. Runtime Settings View

`/settings` should show effective trusted Relay settings.

Example:

```text
Settings

Runtime
  Default profile          Codex / Implement

Scheduler
  Global concurrency       2
  Per-project              1
  Hard max duration        60m

Safety
  Explicit Run             Required
  Auto commit              Off
  Auto push                Off

Storage
  Projects                 %APPDATA%\Relay\config.toml
  Runtimes                 %APPDATA%\Relay\runtimes.toml
  SQLite                   %APPDATA%\Relay\state.sqlite3
```

Editing may use focused selectors.

Do not expose provider secrets.

---

# 77. Doctor View

Use the generic runtime manager.

Example:

```text
Relay Doctor

Core
  ✓ Project config
  ✓ SQLite
  ✓ Git

Codex
  ✓ Installed
  ✓ Authenticated
  ✓ Compatible
  ? Account identity unavailable

Antigravity
  ✓ Installed
  ✓ Authenticated
  ! Safety prerequisite mismatch
    allowNonWorkspaceAccess must be disabled

Projects
  ✓ relay

No secrets are displayed.
```

Do not include unrelated future M10/M12 noise as the main Doctor content.

Future integrations can be a muted footer if needed.

---

# 78. Runtime Switch UX

Global switch:

```text
/runtime use antigravity/implement
```

Result:

```text
Default runtime
  Antigravity / Implement ✓

Future Run approvals will use this profile unless the project or task review overrides it.
```

Task Review one-run change:

```text
Change runtime/profile
```

selector:

```text
> Codex / Implement
  Codex / Review
  Codex / Deep
  Antigravity / Implement
  Antigravity / Review
  Antigravity / Deep
```

Only show profiles that resolve successfully.

Mark unhealthy providers:

```text
Antigravity / Implement    × unavailable
```

and prevent selection unless user opens details.

---

# 79. Runtime Selection Must Be Visible on Every Active Task

Tasks/Home execution rows should retain the runtime profile chosen at queue time.

Do not render active task runtime from the **current global default**.

Store it in:

```text
ExecutionViewState
scheduler queued metadata
execution record
```

Example:

```text
relay
M8.1 runtime work
Antigravity / Implement · RUNNING
```

even if the user later switches global default back to Codex.

---

# 80. Command Completion

Current prefix filtering is a useful base.

Enhance suggestions with runtime data:

```text
/runtime use c
  codex/review
  codex/implement
  codex/deep
```

and READY tasks:

```text
/run 81
  81a7... relay — Runtime routing
```

Keep suggestions bounded.

Do not implement shell/path completion except the explicit `/project add <path>` editor.

---

# 81. Quality — Runtime Router Unit Tests

Required:

```text
register codex + antigravity
duplicate/mismatched registration fails
unknown profile provider fails closed
codex/implement routes to codex fixture
antigravity/implement routes to antigravity fixture
provider list deterministic
```

---

# 82. Quality — Scheduler Run Policy Tests

Required:

```text
legacy submit() still works
submit_with_policy() stores profile
queued profile immutable after external default changes
run request contains selected profile
execution record stores selected profile
duration <= hard ceiling
repair budget <= hard ceiling
invalid policy rejected
```

Also retain all accepted M6:

```text
global concurrency
per-project concurrency
cross-project isolation
cancellation
bounded recovery
persistence failure
```

---

# 83. Quality — Runtime Switching Integration Tests

Use deterministic provider fixtures.

Scenario:

```text
global default = codex/implement
Task A submitted
global default switched to antigravity/implement
Task B submitted

A → Codex fixture
B → Antigravity fixture
```

Active Task A must not migrate.

---

# 84. Quality — Multi-Provider Concurrency

With:

```text
max_parallel = 2
per_project = 1
```

use:

```text
Project A + Codex
Project B + Antigravity
```

Both may execute concurrently.

No global provider mutex.

Cancellation of one must not kill the other.

---

# 85. Quality — Stack Compatibility Tests

Test:

```text
task stack absent
→ local default allowed

task requests chatgpt-codex
local default codex/implement
→ allowed

task requests chatgpt-codex
project default antigravity/implement
no explicit override
→ blocked

task requests chatgpt-codex
user explicitly selects antigravity/implement
→ Task Review warning + explicit Run allows
```

---

# 86. Quality — `/diff` / `/logs` Safety Tests

Mandatory P0 regression:

```text
READY task
/diff <task>
→ READY
→ not queued

READY task
/logs <task>
→ READY
→ not queued

/run <task>
→ queued exactly once
```

---

# 87. Quality — Codex Profile Tests

For each built-in profile:

```text
review
implement
deep
```

verify generated command/config matches the profile.

Test:

```text
task text cannot change argv
no extra writable roots
network disabled by default
model Default omits explicit override
unsupported profile fails
capability Unknown is not displayed as Verified
```

No live Codex in normal tests.

---

# 88. Quality — Antigravity Adapter Tests

Use fixture process/streams.

Required:

```text
current machine-mode argv shape
prompt transport safety
native executable path
.cmd fallback policy
stream-json init
stream-json step_update
stream-json result
unknown event
malformed JSON
schema failure
provider non-zero exit
cancel
large output bounds
terminal-control sanitization
auth unavailable
unsafe provider settings
model discovery parser
```

No live Antigravity in normal tests.

---

# 89. Quality — Event Loop Tests

Required:

```text
Press + Release → one mutation
Repeat text ignored
Repeat arrows allowed
bulk Paste exact
100 rapid key events preserve input
bounded batch returns to scheduler
frame draws far fewer times than key count
idle does not continuously redraw
M7 scheduler liveness still passes
```

Use a fake/injectable clock where practical.

Avoid timing-flaky tests.

---

# 90. Quality — TUI View Model Tests

Test semantic view models instead of full escape-code screenshots.

Examples:

```text
Running → accent
Done → success
Failed → error
NeedsDecision → warning

Runtime Ready → success
NotAuthenticated → warning/error
Capability Verified → success
Capability Unknown → warning
Capability Mismatch → error

Diff + line → diff_added
Diff - line → diff_removed
```

---

# 91. Manual Windows Acceptance — Keyboard

Mandatory before DEV completion.

Use the actual Windows Terminal / PowerShell setup.

Type:

```text
abcdef
committee
bookkeeper
/runtime
/runtime use antigravity/implement
/project add
```

Test:

```text
hold printable key
hold Backspace
hold Down
tap Enter once
tap Esc once
large paste
```

Expected:

```text
no doubled characters
no dropped intended repeated letters
no accidental duplicate submit
no hypersensitive navigation
no character-by-character paste crawl
```

---

# 92. Manual Windows Acceptance — Runtime Switching

Use safe fixture/dogfood tasks.

1. Set:

```text
/runtime use codex/implement
```

2. Queue Task A.

3. Switch:

```text
/runtime use antigravity/implement
```

4. Queue Task B.

Verify:

```text
Task A execution record = codex/implement
Task B execution record = antigravity/implement
```

not merely TUI labels.

---

# 93. Manual Antigravity Acceptance

Use a disposable worktree.

Required:

```text
[ ] provider machine mode confirmed
[ ] Relay remains visible for entire run
[ ] no AGY TUI
[ ] no interactive auth prompt
[ ] no interactive permissions prompt
[ ] structured activity appears in Relay
[ ] repository boundary remains trusted
[ ] cancel kills only AGY execution tree
[ ] Relay remains responsive after cancellation
[ ] terminal raw/alternate-screen state remains valid
```

Do not perform this before provider compatibility/safety probes pass.

---

# 94. Manual Codex Dogfood — M8 Review Requirement

M8 still has a required low-risk Relay-on-Relay Codex dogfood acceptance.

After M8/M8.1 corrections:

```text
use a disposable Relay worktree
use a small documentation-only task
Run through Relay
record:
  TaskId
  ExecutionId
  runtime profile
  Codex version
  final durable state
  outcome
  changed files
  verification
  no commit
  no push
```

This closes the original R-M8-05 acceptance blocker.

---

# 95. Manual Multi-Runtime Dogfood

After Codex dogfood succeeds, run one small Antigravity task in a disposable worktree.

The purpose is not to consume usage unnecessarily.

Verify:

```text
real provider routing
terminal isolation
structured activity
runtime/profile persistence
repository diff
result view
cancel isolation where practical
```

---

# 96. Recommended Implementation Order

Do not start with color polish.

Implement in this order:

## Phase A — P0 correctness

```text
A1 split task selector from Run submission
A2 fix /diff and /logs side effects
A3 add ExecutionRunPolicy
A4 add submit_with_policy
A5 queue immutable policy
A6 make run_task use queued policy
A7 make RuntimeRouter implement Runtime
A8 compose scheduler with RuntimeRouter
A9 make runtime switching execute the selected provider
```

Stop and run full tests.

## Phase B — runtime settings / selection

```text
B1 runtimes.toml
B2 global default
B3 project profile overrides
B4 one-run task override
B5 requested-stack compatibility
B6 runtime command validation
```

Stop and run full tests.

## Phase C — provider correctness

```text
C1 Codex profile-driven command builder
C2 Codex model/effort cleanup
C3 truthful Codex capabilities
C4 Antigravity print/stream-json adapter
C5 Antigravity safety settings validation
C6 Antigravity model discovery/default
C7 terminal/process isolation regression
```

Stop and run full tests.

## Phase D — activity plumbing

```text
D1 RuntimeEvent::Activity
D2 CoreExecutionEvent::RuntimeActivity
D3 adapter event normalization
D4 ExecutionViewState activity
D5 Git-backed changed-file state
```

Stop and run full tests.

## Phase E — TUI architecture

```text
E1 generic runtime probes
E2 runtime dashboard/detail
E3 Task Review execution envelope
E4 project add modes
E5 activity Home
E6 execution/result/diff/logs
E7 semantic spans across views
```

Stop and run full tests.

## Phase F — responsiveness

```text
F1 bounded input drain
F2 liveness tick separation
F3 dirty-state reporting
F4 active-execution redraw correction
F5 remove unsafe eprintln trace
```

Stop and run full tests.

## Phase G — dogfood

```text
G1 Windows input
G2 Codex Relay-on-Relay
G3 Antigravity low-risk run
G4 multi-provider deterministic concurrency
```

Only then prepare Review submission.

---

# 97. Recommended Commit Sequence

Keep the history reviewable.

Suggested:

```text
1. fix[Jacinth]: keep read-only task commands non-executing

2. feat[Jacinth]: carry trusted runtime policy through scheduler

3. feat[Jacinth]: route executions through provider runtime router

4. feat[Jacinth]: persist trusted runtime selection settings

5. fix[Jacinth]: make Codex execution profile-driven

6. fix[Jacinth]: use Antigravity provider-native machine mode

7. feat[Jacinth]: normalize provider execution activity

8. refactor[Jacinth]: separate Relay TUI orchestration surfaces

9. feat[Jacinth]: complete runtime dashboards and execution UX

10. perf[Jacinth]: batch terminal input and liveness ticks
```

Do not force an exact commit count if cohesive changes differ.

Each commit should compile or be a clearly documented dependent step on the feature branch.

---

# 98. Files Expected to Change

Likely:

```text
crates/relay-core/src/execution.rs

crates/relay-runtime/src/lib.rs
crates/relay-runtime/src/router.rs
crates/relay-runtime/src/profile.rs
crates/relay-runtime/src/integration.rs
crates/relay-runtime/src/activity.rs

crates/relay-store/src/lib.rs
crates/relay-store/migrations/0002_execution_runtime_metadata.sql

runtimes/relay-runtime-codex/src/lib.rs
runtimes/relay-runtime-antigravity/src/lib.rs

apps/relay-tui/src/main.rs
apps/relay-tui/src/app.rs
apps/relay-tui/src/command.rs
apps/relay-tui/src/events.rs
apps/relay-tui/src/theme.rs
apps/relay-tui/src/runtime_settings.rs
apps/relay-tui/src/runtime_manager.rs
apps/relay-tui/src/project_discovery.rs
apps/relay-tui/src/git_views.rs

apps/relay-tui/src/view_model/*
apps/relay-tui/src/ui/*
```

Do not create files merely because they are named here.

Use the split where it materially reduces coupling.

---

# 99. Explicit Non-Goals

Do not implement:

```text
GitHub Issue ingress
External Binding behavior
MCP / Relay Direct
M9 clarification resolution workflow
automatic provider installation
provider credential storage
account switching
quota circumvention
generic shell prompt
generic /exec
auto commit
auto push
full-machine-write runtime profile
provider TUI embedding
chain-of-thought display
new scheduler architecture
```

---

# 100. Required Quality Gates

Run:

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
git status --short
```

Normal tests must not launch real Codex or Antigravity agent turns.

Live provider runs are separate manual acceptance.

---

# 101. Hard "Do Not Call This Complete If..." Gate

M8.1 is **NOT DEV-complete** if any are true:

```text
[ ] /runtime use changes only UI strings.
[ ] scheduler production runtime remains AppRuntime::default() Codex.
[ ] queued task does not freeze its selected runtime profile.
[ ] /diff or /logs can queue a READY task.
[ ] RuntimeRouter exists but is not the execution router.
[ ] adapter ignores RuntimeRequest.profile_id.
[ ] codex/review still launches workspace-write.
[ ] model catalogs contain guessed/stale hard-coded models as authoritative.
[ ] directory existence is reported as Verified write capability.
[ ] Antigravity still uses --headless exec copied from Codex.
[ ] AGY provider TUI can appear over Relay.
[ ] unsafe Antigravity persisted settings can broaden the approved boundary.
[ ] runtime detail uses hard-coded capability checkmarks.
[ ] active task runtime is rendered from current global default.
[ ] /runtime use accepts invalid profiles.
[ ] /runtime test ignores its provider argument.
[ ] discovery failure automatically drops into the six-field project form.
[ ] RuntimeActivity is defined but normal execution UI still uses only raw logs.
[ ] semantic theme exists but major views remain unstyled Strings.
[ ] event loop does not bound/drain rapid terminal input.
[ ] state.tick() is still driven once per keyboard event.
[ ] eprintln! can write input diagnostics over the alternate-screen TUI.
[ ] Windows keyboard acceptance has not been rerun.
[ ] Codex Relay-on-Relay dogfood is still missing.
[ ] Antigravity real routing/terminal isolation has not been exercised.
```

---

# 102. Required DEV Completion Report

Return exactly enough evidence for Review to independently verify.

```text
M8.1 — Multi-Runtime Orchestration + Terminal UX

BASE
459a438839c64d8863045270cab9e337cebb8068

SUCCESSOR
<SHA>

COMMITS
...

DEPENDENCY CONTRACTS
M2:
unchanged

M3:
unchanged

M4:
unchanged

M5:
additive migration only / list exact change

M6:
scheduler concurrency/lifecycle unchanged
additive per-run policy metadata plumbing

M7:
explicit Run unchanged

==================================================
P0 CORRECTNESS
==================================================

/diff non-executing:
PASS

/logs non-executing:
PASS

ExecutionRunPolicy:
IMPLEMENTED

submit_with_policy:
IMPLEMENTED

queued runtime immutability:
PASS

RuntimeRouter actually executes selected provider:
PASS

==================================================
RUNTIME SELECTION
==================================================

global default:
...

project override:
...

legacy stack fallback:
...

one-run override:
...

requested-stack mismatch behavior:
...

invalid profile:
FAIL CLOSED

==================================================
CODEX
==================================================

profile-driven sandbox:
PASS

model strategy:
...

reasoning strategy:
...

writable-root boundary:
PASS

capability reporting:
...

structured activity:
...

==================================================
ANTIGRAVITY
==================================================

installed version:
...

verified machine invocation:
...

prompt transport:
...

stream format:
stream-json

structured result schema:
PASS

sandbox:
...

provider settings validation:
...

outside-workspace policy:
...

model strategy:
...

effort:
...

mode:
...

terminal takeover:
NO

capability reporting:
...

==================================================
PERSISTENCE
==================================================

execution runtime profile:
PASS

runtime metadata snapshot:
...

backward compatibility:
PASS

==================================================
TUI
==================================================

command-first shell:
PASS

runtime dashboard:
PASS

runtime detail:
PASS

Task Review execution envelope:
PASS

project discovery:
PASS

activity Home:
PASS

execution activity:
PASS

Diff:
PASS

Logs:
PASS

Result:
PASS

semantic theme:
PASS

==================================================
RESPONSIVENESS
==================================================

KeyEventKind policy:
PASS

bulk Paste:
PASS

bounded rapid-key drain:
PASS

frame coalescing:
PASS

liveness tick separation:
PASS

idle redraw:
PASS

==================================================
MANUAL WINDOWS ACCEPTANCE
==================================================

single typing:
PASS

repeated-letter words:
PASS

held printable:
PASS

held Backspace:
PASS

held navigation:
PASS

large paste:
PASS

Enter once:
PASS

Esc once:
PASS

==================================================
DOGFOOD
==================================================

Codex Relay-on-Relay:
PASS
TaskId:
ExecutionId:
Profile:
Changed files:
Verification:
No commit:
No push:

Antigravity:
PASS
TaskId:
ExecutionId:
Profile:
Changed files:
Verification:
Terminal remained Relay-owned:
No commit:
No push:

==================================================
QUALITY
==================================================

cargo metadata:
PASS

cargo tree:
PASS

cargo fmt:
PASS

cargo clippy:
PASS

cargo test:
PASS

test count:
...

live providers invoked by normal tests:
NO

==================================================
SCOPE
==================================================

GitHub ingress:
NOT IMPLEMENTED

MCP:
NOT IMPLEMENTED

M9 resolution:
NOT IMPLEMENTED

auto commit:
OFF

auto push:
OFF

==================================================
DEV VERDICT
==================================================

PASS — READY FOR REVIEW
```

---

# 103. Final Architecture Acceptance

The implementation is correct when this statement is literally true in source code:

```text
/run
  ↓
trusted runtime/profile resolution
  ↓
ExecutionRunPolicy
  ↓
M6 scheduler queue
  ↓
RuntimeRouter
  ↓
selected provider adapter
  ↓
provider-native non-interactive worker
  ↓
normalized activity + bounded logs
  ↓
Relay TUI
```

And these are impossible:

```text
/runtime use antigravity
but scheduler still executes Codex

/diff task
but task begins executing

provider says write capability
and Relay displays Verified without evidence

AGY launches its own terminal UI

global runtime changes
and an already queued task silently switches provider
```

The permanent product contract is:

> **Relay owns the execution envelope, the scheduler, the repository boundary, and the terminal experience. Providers execute inside that envelope through native machine interfaces.**

The permanent UX contract is:

> **Relay is a responsive command-oriented orchestration CLI: concise activity, truthful runtime state, semantic visual hierarchy, and explicit user control without debug-console leakage.**
