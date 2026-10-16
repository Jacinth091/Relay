# Relay — Multi-Runtime Orchestration + Terminal UX Overhaul Implementation Plan

**Status:** Proposed implementation plan  
**Date:** 2026-08-15  
**Intended owner:** Relay Dev Branch / Codex implementation agent  
**Proposed work unit:** `M8.1 — Multi-Runtime Orchestration + Terminal UX Overhaul`  
**Prerequisite:** Finish the M8 Codex Runtime remediation and establish a clean Codex baseline before the multi-runtime expansion is considered complete.  
**Primary references:**

- `10-DEVELOPMENT/reference/TERMINAL-UX-GUIDELINES.md`
- `30-REVIEW/M8-REVIEW.md`
- `10-DEVELOPMENT/modules/M8-codex-runtime.md`
- `RELAY_RUNTIME_ORCHESTRATION_EXECUTION_UX_IMPLEMENTATION_CONTEXT.md`

---

# 1. Purpose

Relay already has the core execution foundation:

```text
trusted project
    ↓
validated HandoffTask
    ↓
durable local state
    ↓
repository preflight
    ↓
bounded scheduler
    ↓
runtime abstraction
    ↓
executor
```

The next problem is orchestration and product UX.

The current terminal client still behaves too much like:

```text
debug dashboard
+ runtime launcher
+ raw execution log
+ manual forms
```

The target is:

```text
modern developer CLI
        +
multi-project orchestration console
        +
provider-aware runtime management
        +
safe execution control plane
```

The user should be able to understand, at a glance:

```text
which projects are active
which tasks are running
which runtime is assigned to each task
which runtime/profile/model a READY task will use
whether each runtime is installed and authenticated
which account is active when safely exposed by the provider
which capabilities the selected profile grants
whether those capabilities are configured, verified, unknown, or mismatched
what the executor is currently doing
which files changed
which verification passed or failed
what needs human attention
```

Relay should not require the user to infer these facts from raw provider logs.

---

# 2. Product Principle

Freeze this principle:

> **Relay Core is runtime-neutral. Relay runtime integrations are provider-native. Relay UX is provider-aware.**

Meaning:

```text
Relay Core / Scheduler
    does not care whether execution uses Codex or Antigravity.

Codex adapter
    may use Codex-native models, reasoning settings, sandbox controls,
    structured output, health probes, and event semantics.

Antigravity adapter
    may use Antigravity-native models, effort settings, execution modes,
    permissions, health probes, and event semantics.

Relay TUI
    may present provider-specific settings where useful while still
    exposing a common Relay execution envelope.
```

Do not force all providers into the lowest common denominator merely to call Relay provider-agnostic.

Provider-neutral orchestration and provider-specific optimization are both required.

---

# 3. Governance and Sequencing

Recommended sequence:

```text
M8 Codex Runtime remediation
    ↓
R-M8-01..R-M8-04 corrected
    ↓
required Relay-on-Relay Codex dogfood
    ↓
M8 Review PASS
    ↓
M8 Master ACCEPTED
    ↓
M8.1 Multi-Runtime Orchestration + Terminal UX Overhaul
```

If this work begins before formal M8 acceptance, preserve two hard checkpoints:

```text
Checkpoint A
M8 Codex blockers fixed and independently testable

Checkpoint B
multi-runtime orchestration and TUI overhaul begins
```

Do not make Review untangle runtime correctness fixes from a broad UI rewrite.

---

# 4. Frozen Architecture Boundaries

This work must preserve the accepted contracts below.

## M2 — Trusted Project Registry

The trusted local path comes from Relay local configuration.

Remote/task content does not choose a local path.

## M3 — Repository Safety

Execution still requires current repository inspection and eligibility.

Runtime selection does not bypass repository safety.

## M4 — HandoffTask

`IMPORT != EXECUTE` remains true.

A HandoffTask may request a logical stack but cannot grant itself privileged runtime capabilities.

## M5 — Persistence

SQLite remains canonical durable task/execution state.

TUI view state is never lifecycle authority.

## M6 — Scheduler / Runtime Contract

The scheduler remains authoritative for:

```text
global concurrency
per-project concurrency
bounded recovery
cancellation
timeouts
state transitions
cross-project isolation
```

Do not create a second scheduler.

## M7 — Execution UX Semantics

Explicit local Run approval remains mandatory.

The new command shell may provide `/run`, but `/run` must invoke the same accepted execution path.

## M8 — Provider Runtime Boundary

Provider adapters execute an already-approved runtime request.

Adapters do not resolve project paths from task text and do not decide durable TaskState.

---

# 5. Target Architecture

```text
                         Relay TUI
                            │
            ┌───────────────┼────────────────┐
            │               │                │
       Command Shell    Runtime UI      Execution UI
            │               │                │
            └───────────────┬┴────────────────┘
                            ↓
                       Relay Core
                            ↓
                   ExecutionScheduler
                            ↓
                       RuntimeRouter
                  ┌─────────┴─────────┐
                  ↓                   ↓
             CodexRuntime     AntigravityRuntime
                  │                   │
             Codex CLI          Antigravity CLI
```

The scheduler continues to depend on the accepted `Runtime` contract.

The router delegates to a trusted provider/profile selected by Relay.

---

# 6. Runtime Concepts

## 6.1 Runtime Provider

Initial providers:

```text
codex
antigravity
fake              development/testing only
```

## 6.2 Runtime Profile

A trusted local execution policy describing:

```text
provider runtime
profile ID
model selection
reasoning/effort setting
filesystem capability
network capability
provider execution mode
extra writable roots
approval ownership
maximum execution duration
repair budget
provider-specific safe options
```

Profiles are local policy.

A task must not silently modify privileged profile fields.

## 6.3 Runtime Selection

Recommended precedence:

```text
explicit trusted one-run selection
        ↓
project default runtime profile
        ↓
Relay global default runtime profile
        ↓
fail closed if no safe profile resolves
```

## 6.4 Runtime Health

Common states:

```text
Checking
Ready
NotInstalled
NotAuthenticated
Incompatible
Misconfigured
Unavailable
```

## 6.5 Runtime Account

Optional provider-reported information:

```text
email
display name
plan/tier
organization/workspace
```

Only expose values through supported provider machine-readable interfaces.

Never scrape credential stores, browser state, keychains, authentication files, or interactive terminal frames merely to obtain identity.

If unavailable, show:

```text
Authentication    ready
Account           identity unavailable from provider CLI
```

## 6.6 Configured vs Effective Capability

Relay must distinguish:

```text
Configured
    what the Runtime Profile requests

Effective
    what the provider can actually establish
```

Capability status should support:

```text
Configured
Verified
Unknown
Mismatch
Unavailable
```

Never claim effective capability merely because Relay supplied a flag.

---

# 7. Runtime Management Interface

Do not overload the existing `Runtime` execution trait unless necessary.

Preferred separation:

```rust
trait Runtime {
    // existing M6 execution contract
}

trait RuntimeIntegration {
    fn descriptor(&self) -> RuntimeDescriptor;

    async fn probe(&self) -> RuntimeProbe;

    fn profiles(&self) -> Vec<RuntimeProfileDescriptor>;

    async fn models(&self) -> RuntimeModelCatalog;

    fn resolve_profile(
        &self,
        selection: &TrustedRuntimeSelection,
    ) -> Result<EffectiveRuntimeProfile, RuntimeConfigError>;

    async fn verify_capabilities(
        &self,
        profile: &EffectiveRuntimeProfile,
        context: &RuntimeCapabilityContext,
    ) -> RuntimeCapabilityReport;
}
```

Exact names may differ.

The architectural split must remain:

```text
Runtime            execution
RuntimeIntegration discovery / health / profiles / models / capabilities
```

If the current `Runtime` trait is not object-safe, do not redesign it solely for dynamic dispatch. Use a small explicit provider enum/router instead.

---

# 8. Runtime Router

Add a trusted router.

Conceptually:

```rust
enum RegisteredRuntime {
    Codex(CodexRuntime),
    Antigravity(AntigravityRuntime),
}

struct RuntimeRouter {
    runtimes: RuntimeRegistry,
    profiles: RuntimeProfileRegistry,
}
```

`RuntimeRouter` implements the accepted M6 `Runtime` contract.

Execution flow:

```text
RuntimeRequest.profile_id
        ↓
trusted profile lookup
        ↓
provider resolved
        ↓
provider-native adapter
```

Do not select providers from arbitrary task prose.

Do not convert unknown HandoffTask extensions into provider arguments.

---

# 9. Runtime Registry

Support:

```text
list runtimes
lookup runtime
probe runtime
resolve default runtime/profile
resolve selected profile
```

Provider adapters remain independent of Relay Core domain types where practical.

---

# 10. Runtime Profile Persistence

Use trusted local Relay configuration.

Do not create a second database solely for runtime profiles.

Illustrative configuration:

```toml
[runtime]
default_profile = "codex/implement"

[runtimes.codex]
executable = "codex"

[runtimes.codex.profiles.review]
model = "default"
reasoning_effort = "medium"
filesystem = "read-only"
network = false
max_duration_secs = 1200
repair_attempts = 1

[runtimes.codex.profiles.implement]
model = "default"
reasoning_effort = "high"
filesystem = "workspace-write"
network = false
max_duration_secs = 1800
repair_attempts = 2

[runtimes.antigravity]
executable = "agy"

[runtimes.antigravity.profiles.review]
model = "default"
effort = "medium"
mode = "plan"
filesystem = "read-only"
network = false
max_duration_secs = 1200

[runtimes.antigravity.profiles.implement]
model = "default"
effort = "high"
mode = "accept-edits"
filesystem = "workspace-write"
network = false
max_duration_secs = 1800
repair_attempts = 2
```

This schema is conceptual.

During implementation, verify current provider CLI semantics before freezing provider-specific mappings.

Do not blindly expose provider CLI flags as persistent Relay configuration.

---

# 11. Built-In Profiles

Keep the initial set small.

## Review

```text
purpose             inspection / analysis / review
filesystem          read-only
network             off
repair budget       small
```

## Implement

```text
purpose             normal approved code changes
filesystem          workspace-write
network             off by default
extra write roots   none
commit              off
push                off
repair budget       bounded
```

## Deep

```text
purpose             difficult debugging / complex implementation
filesystem          workspace-write
reasoning/effort    higher when provider supports it
maximum duration    longer but bounded
network             still off unless explicitly trusted
```

Do not add an unrestricted full-machine profile by default.

---

# 12. Provider-Specific Fields

## Codex

Potential Relay-facing controls:

```text
model
reasoning effort
sandbox/filesystem mode
network
extra writable roots
execution duration
repair budget
```

## Antigravity

Potential Relay-facing controls:

```text
model
effort
agent/execution mode
filesystem permissions
network
execution duration
repair budget
provider-specific safe settings
```

Before implementing provider mappings:

```text
inspect the installed CLI --help
check official provider documentation
add a compatibility regression
fail closed if a required capability is unsupported
```

---

# 13. Codex Baseline Requirement

Before the orchestration expansion is considered complete, close the current M8 blockers:

```text
R-M8-01 valid Codex argv
R-M8-02 Relay-controlled writable boundary
R-M8-03 realistic bounded production duration
R-M8-04 useful safe JSONL activity
R-M8-05 successful Relay-on-Relay dogfood
```

Then make Codex implement the new runtime management layer.

Codex health should expose:

```text
installation
version
authentication
compatibility
structured output readiness
machine event stream readiness
required safety controls
account identity when safely supported
model catalog when safely supported
```

---

# 14. Antigravity Adapter

Add:

```text
runtimes/relay-runtime-antigravity/
```

Dependency direction:

```text
relay-runtime-antigravity
        ↓
relay-runtime
```

It must not depend on `relay-core`.

Responsibilities:

```text
CLI discovery
version probe
authentication probe
compatibility probe
provider profile translation
model/effort translation
trusted working-directory launch
headless/structured output parsing
stdout/stderr streaming
cancellation
terminal outcome mapping
safe provider diagnostics
```

Do not put project routing in the adapter.

---

# 15. Antigravity Discovery

Probe without starting a real coding turn when possible.

Conceptually:

```text
agy --version
agy --help
provider-supported authentication/status command
provider-supported model/status command
```

Implementation must inspect the installed CLI and current official documentation.

Health states:

```text
Not installed
Installed but unauthenticated
Authenticated but incompatible
Ready
```

Do not treat browser login merely launching as successful authentication. Wait for a provider-supported success check.

---

# 16. Antigravity Account Information

Display account information when safely available:

```text
Account
  user@example.com

Plan
  <provider reported plan>
```

Do not:

```text
scrape interactive terminal frames
read provider credential files
read browser sessions
read OS keychain secrets
print access tokens
```

If only authentication readiness is available, that is sufficient.

---

# 17. Relay Capability Envelope

Common Relay vocabulary:

```text
Read workspace
Edit workspace
Run commands
Network
Additional writable roots
Outside-workspace access
Commit
Push
Remote mutation
```

Provider adapters translate this to their native controls.

Example:

```text
Relay profile
  Edit workspace = yes
  Network = no

Codex adapter
  provider-native workspace-write policy

Antigravity adapter
  provider-native editing mode / permissions
```

Provider flags are not the primary user vocabulary.

---

# 18. Effective Capability Verification

Add a provider-specific capability report:

```rust
RuntimeCapabilityReport {
    filesystem_read,
    filesystem_write,
    command_execution,
    network,
    extra_writable_roots,
    provider_notes,
}
```

Rules:

1. Prefer provider-native machine-readable capability introspection.
2. Use controlled provider-specific probes only when safe and bounded.
3. Never weaken the sandbox to make verification pass.
4. Do not claim `Verified` if Relay only knows the configured value.
5. If a required capability is definitely mismatched before execution, block or degrade Run clearly.
6. If verification is impossible, show `Effective: Unknown`.

Do not waste repair attempts on a known capability mismatch.

---

# 19. Terminal Product Architecture

Permanent top-level surfaces:

```text
HOME
TASKS
PROJECTS
RUNTIMES
SETTINGS
```

Supporting views:

```text
Task Review
Execution
Result
Diff
Logs
Doctor
Project Add
Runtime Detail
Profile Editor
```

The command prompt remains available across normal views.

---

# 20. Command-First Interaction

Primary commands:

```text
/home

/new
/tasks
/run <task>
/cancel <task>

/projects
/project add [path]

/runtimes
/runtime <provider>
/runtime use <provider-or-profile>
/runtime test <provider>
/runtime profile <provider-or-profile>

/settings
/doctor

/diff [task]
/logs [task]

/help
/quit
```

Exact syntax may evolve.

Keep command families small and discoverable.

---

# 21. Relay Prompt Is Not a Shell

These must never execute:

```text
git status
cargo test
rm -rf .
cmd /C ...
powershell -Command ...
sh -c ...
```

Unknown text should produce:

```text
Unknown Relay command. Type /help.
```

No generic `/exec`, `/shell`, or `/git` command in this work.

---

# 22. Home — Orchestration Dashboard

Replace raw scheduler counter dumps.

Target active state:

```text
 Relay                              Codex ●  Implement

 2 active · 1 waiting · 1 attention

 ● relay
   Fix runtime routing
   Codex / Implement · RUNNING · 02:14

 ● practicore
   Placement validation
   Antigravity / Implement · VERIFYING · 01:03

 ◌ relay
   Runtime documentation
   WAITING · project slot

 ! practicore
   Migration redesign
   NEEDS DECISION

──────────────────────────────────────────────

> /_
```

Idle:

```text
 Relay                              Codex ●  Implement

 No active work.

 Projects  2
 Tasks     12

 Recent
 ✓ relay        M8 Codex Runtime
 ✓ practicore   Fix reassignment

──────────────────────────────────────────────

> /_
```

First run:

```text
 Relay                              No runtime selected

 Welcome to Relay.

 No trusted projects are configured.

 Try:
   /project add
   /runtimes
   /help

──────────────────────────────────────────────

> /_
```

Only show meaningful non-zero activity.

---

# 23. Runtime Dashboard

`/runtimes`:

```text
 Runtimes

 > Codex CLI
   ● Ready
   Account        authenticated
   Profile        Codex / Implement
   Model          Default
   Reasoning      High

   Antigravity
   ● Ready
   Account        user@example.com
   Profile        Antigravity / Implement
   Model          Default
   Effort         High

   Fake Runtime
   ○ Development only

──────────────────────────────────────────────

Default: Codex / Implement

> /_
```

Use concise statuses and never display secrets.

---

# 24. Runtime Detail

`/runtime codex`:

```text
 Runtime / Codex

 Status
   Installation         ✓
   Version              ...
   Authentication       ✓ ready
   Account              available / unavailable
   Compatibility        ✓

 Active profile
   Codex / Implement

 AI
   Model                Default
   Reasoning            High

 Capabilities
   Workspace read       ✓ configured / verified
   Workspace write      ✓ configured / verified
   Commands             ✓
   Network              ×
   Extra write roots    none
   Commit               ×
   Push                 ×

 Limits
   Duration             30m
   Repairs              2

 > Change profile
   Set default
   Test prerequisites
```

`/runtime antigravity` uses the same Relay hierarchy while exposing provider-native fields such as effort or mode where useful.

---

# 25. Fast Runtime Switching

Commands:

```text
/runtime use codex
/runtime use codex/implement
/runtime use antigravity
/runtime use antigravity/implement
```

Also provide a selector:

```text
 Select runtime

 > Codex / Implement
   Codex / Review
   Antigravity / Implement
   Antigravity / Review
```

Changing the default runtime does not affect already-running executions.

Do not migrate queued/running work silently.

---

# 26. Per-Run Runtime Selection

Task Review should expose the actual execution envelope:

```text
 Task

 ID           ...
 Project      relay
 State        READY
 Repository   ✓ eligible

 Runtime
   Antigravity / Implement

 AI
   Model       Default
   Effort      High

 Permissions
   Read workspace        ✓
   Edit workspace        ✓
   Run commands          ✓
   Network               ×
   Outside workspace     ×
   Commit                ×
   Push                  ×

 Limits
   Maximum duration      30m
   Repairs               2

 > Run
   Change runtime/profile
   Details
   Back
```

Changing the runtime/profile is a trusted local decision.

Do not rewrite the HandoffTask to encode the local privilege choice.

---

# 27. Runtime Selection Persistence

```text
one-run selection
    applies only to the execution

project default
    persists in trusted local project configuration

global default
    persists in Relay local config
```

Do not store secrets in project config.

---

# 28. Settings

`/settings`:

```text
 Settings

 General
   Default runtime/profile      Codex / Implement

 Scheduler
   Global concurrency           2
   Per-project concurrency      1

 Execution
   Duration                     profile-controlled
   Repair policy                profile-controlled

 Safety
   Explicit Run approval        On
   Automatic commit             Off
   Automatic push               Off

 Storage
   Config                       ...
   SQLite                       ...

 Runtimes
   Codex                        ● Ready
   Antigravity                  ● Ready

 > Runtime settings
   Scheduler settings
   Safety
```

Persist trusted settings through existing Relay configuration ownership.

Do not create a new settings database.

---

# 29. Runtime Profile Editor

Codex example:

```text
 Profile / Codex / Implement

 Model               Default
 Reasoning           High
 Filesystem          Workspace write
 Network             Off
 Extra write roots   None
 Duration            30m
 Repairs             2
```

Antigravity example:

```text
 Profile / Antigravity / Implement

 Model               Default
 Effort              High
 Mode                Accept edits
 Filesystem          Workspace write
 Network             Off
 Duration            30m
 Repairs             2
```

Always preserve:

```text
Auto commit           Off
Auto push             Off
Remote mutation       Off
```

unless later explicitly authorized.

---

# 30. Doctor

`/doctor` should cover Relay plus all configured runtimes:

```text
 Relay Doctor

 Core
   ✓ Config readable
   ✓ SQLite available
   ✓ Git available

 Codex
   ✓ Installed
   ✓ Authenticated
   ✓ Compatible
   ✓ Structured output
   ✓ Required safety controls

 Antigravity
   ✓ Installed
   ✓ Authenticated
   ✓ Headless execution compatible
   ✓ Structured output compatible
   ? Account identity unavailable

 Projects
   ✓ relay
   ✓ practicore

 No blocking issues.
```

If missing:

```text
Antigravity
  × Not installed

Suggested action:
Install the provider CLI, then run `/runtime test antigravity`.
```

Do not automatically install provider CLIs in this work.

---

# 31. Discovery-First Project Onboarding

`/project add` should no longer open six blank fields by default.

Normal flow:

```text
/project add
    ↓
inspect current directory
    ↓
detect Git root
    ↓
detect origin remote
    ↓
suggest project key/name
    ↓
derive metadata when safe
    ↓
user confirmation
```

Example:

```text
 Add Project

 Detected repository

 Name       Relay
 Key        relay
 Path       C:\Users\PCC\Desktop\My Projects\Relay
 Remote     github.com/Jacinth091/Relay
 Stack      chatgpt-codex

 > Add this repository
   Edit details
   Choose another directory
   Advanced setup
   Cancel
```

Manual configuration remains available under `Advanced`.

No trusted Registry write occurs during detection.

---

# 32. New Task UX

`/new`:

```text
 New Task

 > Paste task
   Import task file
   Back
```

Do not require users to memorize `F` and `V`.

Import remains separate from Run.

---

# 33. Tasks View

`/tasks`:

```text
 Tasks

 > RUNNING      relay        Fix runtime routing
   VERIFYING    practicore   Placement fix
   WAIT:PROJ    relay        Runtime docs
   READY        relay        Add Antigravity adapter
   DONE         relay        M8 Codex Runtime

──────────────────────────────────────────────

> /_
```

Add runtime/profile as secondary metadata when space allows.

---

# 34. Runtime Event Normalization

The TUI must not parse provider JSON directly.

Pipeline:

```text
provider machine stream
      ↓
provider adapter
      ↓
Relay-normalized activity
      ↓
execution view model
      ↓
TUI
```

If the existing `RuntimeEvent` cannot express the UX cleanly, add the smallest provider-neutral activity model needed.

Possible activities:

```text
SessionStarted
Analyzing
CommandStarted
CommandUpdated
CommandCompleted
FileChanged
VerificationStarted
VerificationUpdated
VerificationCompleted
AgentMessage
Warning
ProviderDiagnostic
```

Do not expose hidden chain-of-thought.

Provider adapters translate safe machine events into Relay activities.

Lifecycle authority remains with Core.

---

# 35. Execution Screen

Target:

```text
 relay · Fix runtime routing

 Codex / Implement
 Default model · High · Write · Offline

 ● RUNNING                                    02:14
   Attempt 1/2

 Current
   ● Running cargo test --workspace

 Activity
   ✓ Inspected repository
   ✓ Read runtime adapter
   ✓ Modified runtime router
   ● Running cargo test --workspace

 Files
   M crates/relay-runtime/src/router.rs
   M apps/relay-tui/src/runtime.rs

 Verification
   ● cargo test --workspace

──────────────────────────────────────────────

> /_

Commands:
  /cancel <task>
  /diff
  /logs
```

The main execution view should show logical actions, not full provider shell wrappers.

---

# 36. Logical Command Rendering

Default:

```text
cargo test --workspace
git diff --check
rg "RuntimeRouter"
```

Not:

```text
C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -Command ...
```

Raw provider details remain available in Logs.

Do not falsify the underlying executed command.

---

# 37. Canonical Files Changed

Provider file-change events are useful activity hints but not repository truth.

Canonical changed-file state comes from the trusted local Git repository.

At appropriate lifecycle points, refresh read-only repository status and show:

```text
M README.md
A runtimes/relay-runtime-antigravity/src/lib.rs
D obsolete-file.rs
```

No staging, reset, checkout, commit, or push is introduced.

---

# 38. Diff Viewer

Add:

```text
/diff [task]
```

Source:

```text
trusted local Git repository state
```

Capabilities:

```text
changed-file list
+/- totals
scrollable patch
bounded memory
large-diff truncation with an explicit notice
```

Provider-generated diff summaries are not canonical.

The view is read-only.

---

# 39. Logs

Add:

```text
/logs [task]
```

Logs may contain:

```text
stdout
stderr
provider diagnostics
raw logical command details
bounded provider activities
```

Raw logs remain secondary.

All retention remains bounded.

---

# 40. Result View

Success:

```text
 RESULT — SUCCESS

 Task
   Fix runtime routing

 Runtime
   Codex / Implement
   Default model · High

 Files changed
   M router.rs
   A antigravity/lib.rs

 Changes
   +142  -18

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
   Added trusted multi-runtime routing and provider health integration.

 > View diff
   View logs
   Tasks
```

Capability failure example:

```text
 RESULT — IMPLEMENTATION FAILURE

 Blocking capability
   Edit workspace

 Configured
   workspace-write

 Effective
   read-only

 Repository changes
   none
```

Durability uncertainty remains explicit and never becomes a fabricated terminal state.

---

# 41. Semantic Visual System

Use a small semantic theme:

```text
accent
success
warning
error
muted
diff-added
diff-removed
```

Markers:

```text
● active
◌ waiting
! attention
✓ success
× failure
```

Do not rely on color alone.

---

# 42. Layout Philosophy

Reduce giant full-terminal borders.

Prefer:

```text
header
whitespace
compact sections
activity rows
persistent command prompt
```

On narrow terminals prioritize:

```text
task/runtime state
attention
command prompt
current activity
```

Secondary metadata may truncate.

---

# 43. Input Modes

Use explicit modes:

```text
CommandPrompt
Selection
TextEditor
Confirmation
```

Normal typing goes to the Relay command prompt.

Arrow/Enter/Esc remain useful for focused selection.

Legacy single-letter shortcuts may remain temporarily as accelerators, but must no longer be the primary product vocabulary.

---

# 44. Paste Responsiveness

Fix fallback paste behavior.

Target:

```text
receive first event
    ↓
drain immediately queued input events
    ↓
apply batch
    ↓
draw once
```

Requirements:

```text
large paste does not visibly type character by character
runtime events do not starve
cancellation remains responsive
idle CPU remains bounded
```

---

# 45. Event-Driven TUI Loop

Redraw should respond to:

```text
terminal input
runtime activity
scheduler event
runtime health update
elapsed-time tick
configuration change
```

Use a bounded periodic tick only for timers/spinners/liveness.

Do not spin continuously.

---

# 46. Runtime Health Refresh

Refresh health:

```text
at startup
on /runtime test
when provider settings change
after authentication may have changed
```

Do not probe every frame.

Cache the latest result.

---

# 47. Authentication UX

Relay does not own provider credentials.

Allowed:

```text
Recheck authentication
Show provider login instructions
```

Not in this work:

```text
credential storage
account switching
token copying
automatic login automation
```

---

# 48. Run Eligibility

The TUI's friendly preflight may consider:

```text
TaskState == READY
project resolves
repository eligible
runtime/profile resolves
runtime installed
runtime authenticated
runtime compatible
required capabilities are not known-mismatched
scheduler rebind is safe
task is not already queued/running
```

Core remains authoritative when execution actually begins.

---

# 49. Scheduler Integration

Initial design:

```text
one accepted M6 scheduler
        +
RuntimeRouter
        +
multiple provider adapters
```

Do not create one scheduler per provider.

Do not add hidden provider-global locks unless a verified provider limitation requires an explicit architecture decision.

Codex and Antigravity tasks may run concurrently when M6 limits allow.

---

# 50. Runtime Immutability Per Execution

Once claimed, an execution freezes:

```text
ExecutionId
runtime provider
profile ID
profile snapshot
model snapshot
reasoning/effort snapshot
```

Changing global defaults must not alter active execution.

Repair attempts should normally remain on the same provider/profile.

Do not silently migrate providers mid-execution.

---

# 51. Execution Persistence Metadata

Persist enough non-secret metadata to explain history:

```text
runtime provider ID
profile ID
model selection snapshot
reasoning/effort snapshot
configured capability summary
runtime version
terminal outcome
repair count
```

Do not persist:

```text
credentials
tokens
provider auth files
unlimited raw logs
```

If schema changes are required, add a new migration.

Never rewrite accepted M5 migrations.

---

# 52. Backward Compatibility

Existing M5/M6/M7 records must remain readable.

Historical execution with missing runtime metadata:

```text
Runtime: legacy / unavailable
```

Existing project configs without runtime defaults resolve through the new global default.

If no safe runtime is available, READY tasks remain READY and Run is blocked with an actionable message.

---

# 53. Runtime Profile Resolution Tests

Test:

```text
explicit one-run profile wins
project default wins over global
global used when no project default
unknown profile fails
provider unavailable fails
task cannot inject privileged profile
profile/provider mismatch fails
```

---

# 54. Codex Tests

Retain all M8 regressions.

Add deterministic coverage for:

```text
health probe
profile mapping
model mapping
reasoning mapping
capability report
runtime router delegation
account unavailable handling
```

Normal tests must not consume real Codex usage.

---

# 55. Antigravity Tests

Normal tests must not consume live Antigravity usage.

Use:

```text
command builder tests
fixture process tests
machine-stream parser tests
structured result tests
cancellation fixture
health fixture
model/effort mapping
profile/mode mapping
```

Live tests must be explicitly gated or ignored.

---

# 56. Runtime Router Tests

Test:

```text
Codex profile → Codex adapter
Antigravity profile → Antigravity adapter
unknown provider → fail closed
active execution remains on original provider
different providers can execute concurrently under M6 limits
no accidental global provider lock
```

---

# 57. Runtime Dashboard Tests

Test view models for:

```text
both providers ready
one provider missing
unauthenticated
incompatible
account available
account unavailable
configured capability
verified capability
capability mismatch
default profile marker
```

---

# 58. Command Parser Tests

Required commands:

```text
/home
/new
/tasks
/projects
/project add
/runtimes
/runtime codex
/runtime antigravity
/runtime use codex/implement
/runtime use antigravity/implement
/runtime test codex
/settings
/doctor
/run <task>
/cancel <task>
/diff
/logs
/help
/quit
```

Shell-looking input must remain inert.

---

# 59. Project Onboarding Tests

Use temporary Git repositories.

Test:

```text
current directory detection
subdirectory resolves repository root
origin remote detection
GitHub metadata derivation
non-GitHub remote
key suggestion
confirmation required
advanced/manual fallback
no Registry write during discovery
```

---

# 60. Execution UX Tests

Test:

```text
activity stream
logical command rendering
file-change display
verification state
runtime/profile display
cancel/background
needs clarification
needs decision
persistence uncertainty
```

No chain-of-thought display.

---

# 61. Diff Tests

Test:

```text
modified file
added file
deleted file
large diff truncation
+/- counts
trusted repository as source of truth
provider diff not treated as canonical
```

---

# 62. Paste/Event Loop Tests

Test:

```text
bulk Paste event
rapid key-event batch
bounded redraw behavior where testable
runtime events not starved
cancel input processed promptly
bounded idle loop
```

---

# 63. Manual Acceptance — Runtime Management

```text
1. Start Relay.
2. /runtimes.
3. Confirm Codex prerequisite state.
4. Confirm Antigravity prerequisite state.
5. Open Codex detail.
6. Open Antigravity detail.
7. Switch default runtime.
8. Restart Relay.
9. Confirm default persists.
10. Use a one-run runtime override without changing the global default.
```

---

# 64. Manual Acceptance — Project Onboarding

From inside the Relay repository:

```text
/project add
```

Expected:

```text
Git root detected
path suggested
remote suggested
project key suggested
metadata shown
explicit confirmation required
```

No mandatory six-field form.

---

# 65. Manual Acceptance — Multi-Runtime Execution

Use two low-risk projects or worktrees:

```text
Task A — Project A — Codex / Implement
Task B — Project B — Antigravity / Implement
```

With M6 global capacity permitting two:

```text
A and B may run concurrently.
```

Verify:

```text
each runtime receives only its trusted project
cancel A does not kill B
provider output does not cross tasks
results identify the correct provider/profile
```

Keep live usage minimal.

---

# 66. Manual Acceptance — Runtime Switching

Create READY tasks and run one with Codex and another with Antigravity.

Before each Run, Task Review must clearly display the selected provider/profile and execution envelope.

Do not switch providers silently after execution starts.

---

# 67. Manual Acceptance — Effective Capability

For each provider:

```text
Review profile
  must not claim workspace write

Implement profile
  should configure workspace write

Effective capability
  must be shown honestly as Verified / Unknown / Mismatch
```

A definite mismatch should block/degrade Run instead of wasting repair attempts.

---

# 68. Manual Acceptance — UX

Verify:

```text
[ ] persistent command prompt
[ ] /help discoverable
[ ] Home is activity-oriented
[ ] runtime dashboard understandable
[ ] no raw zero-counter wall
[ ] no giant manual project form by default
[ ] runtime switching easy
[ ] task runtime/profile visible before Run
[ ] execution activity readable
[ ] logical commands visible
[ ] raw logs secondary
[ ] diff inspectable
[ ] result summarizes changes and verification
[ ] large paste responsive
```

---

# 69. Security Regression Checklist

```text
[ ] task cannot choose cwd
[ ] task cannot choose executable
[ ] task cannot choose arbitrary provider args
[ ] task cannot escalate runtime profile
[ ] task cannot enable network
[ ] task cannot add writable roots
[ ] task cannot enable commit
[ ] task cannot enable push
[ ] Relay prompt cannot execute arbitrary shell input
[ ] provider credentials are not persisted
[ ] account display cannot expose secrets
[ ] no shell interpolation of task text
[ ] repository preflight remains mandatory
[ ] cancellation remains isolated
[ ] scheduler limits remain authoritative
[ ] durable lifecycle remains authoritative
```

---

# 70. Non-Goals

Do not add:

```text
GitHub Issue ingress
External Binding behavior
MCP / Relay Direct
generic shell console
generic /exec
generic /git
auto commit
auto push
runtime marketplace
automatic CLI installation
credential management
provider account switching
full-machine write profile
chain-of-thought display
M9 resolution workflow
scheduler redesign
persistence redesign
```

---

# 71. Recommended Code Surfaces

Likely changes:

```text
crates/relay-runtime/
    provider-neutral runtime management/profile/activity types
    RuntimeRouter if appropriate

runtimes/relay-runtime-codex/
    RuntimeIntegration implementation
    profile translation
    health/model/capability probes
    normalized activity mapping

runtimes/relay-runtime-antigravity/
    new provider adapter
    execution
    health/profile/model/capability probes
    structured event/result mapping

crates/relay-core/
    minimal trusted runtime selection/profile resolution only if domain ownership requires it
    execution metadata persistence integration if needed

crates/relay-store/
    additive migration/API only if runtime metadata persistence requires it

apps/relay-tui/
    command shell
    activity Home
    runtime dashboard/detail
    settings/profile editor
    doctor
    discovery-first project onboarding
    task review envelope
    execution activity
    diff/logs/result
    event/paste loop improvements
```

Do not move provider CLI flag knowledge into `relay-core`.

---

# 72. Suggested TUI Organization

Avoid continuing to grow a single huge `main.rs`.

Recommended direction:

```text
apps/relay-tui/src/

main.rs
app.rs
commands.rs
project_discovery.rs

ui/
    mod.rs
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

view_model/
    mod.rs
    runtime.rs
    execution.rs
```

Do not force the exact file split if the current code suggests a smaller safe refactor.

The goal is separation of concerns, not file-count inflation.

---

# 73. Implementation Order

## Stage 0 — Finish M8

```text
R-M8-01 argv
R-M8-02 writable-root safety
R-M8-03 production timeout
R-M8-04 meaningful JSONL
R-M8-05 dogfood
```

Freeze a clean Codex baseline.

## Stage 1 — Runtime Management Types

```text
RuntimeDescriptor
RuntimeHealth / RuntimeProbe
RuntimeProfile
EffectiveRuntimeProfile
RuntimeModelCatalog
RuntimeCapabilityReport
RuntimeIntegration
```

## Stage 2 — Codex Integration

Adapt Codex to the new management layer.

Prove no M6/M7 regression.

## Stage 3 — Runtime Router

Allow provider/profile routing through the accepted Runtime boundary.

Initially register Codex only.

## Stage 4 — Antigravity Adapter

Implement:

```text
discovery
authentication
compatibility
profile translation
headless execution
activity mapping
structured result
cancellation
```

## Stage 5 — Runtime Config Persistence

Add global/project defaults and provider profiles with backward-compatible loading.

## Stage 6 — Persistent Command Shell

Implement command-first TUI foundation.

## Stage 7 — Activity Home

Replace the debug dashboard.

## Stage 8 — Runtime Dashboard + Settings + Doctor

Expose prerequisites, account state, profiles, and easy switching.

## Stage 9 — Project Onboarding

Discovery-first `/project add`.

## Stage 10 — Task Review Envelope

Show runtime/profile/model/reasoning/capabilities before Run.

## Stage 11 — Execution UX

Normalized activity, files, commands, verification.

## Stage 12 — Diff + Logs + Result

Read-only diff, bounded logs, polished result.

## Stage 13 — Input/Event Loop

Paste batching and event-driven redraw.

## Stage 14 — Dogfood

Run representative low-risk Codex and Antigravity tasks.

---

# 74. Commit Strategy

Do not submit this as one opaque commit.

Recommended sequence:

```text
1. fix[Jacinth]: close M8 Codex review blockers

2. feat[Jacinth]: add runtime orchestration profiles

3. feat[Jacinth]: add trusted runtime router

4. feat[Jacinth]: add Antigravity runtime adapter

5. feat[Jacinth]: add command-first orchestration shell

6. feat[Jacinth]: add runtime dashboard and settings

7. feat[Jacinth]: improve execution activity and repository UX

8. perf[Jacinth]: batch terminal input and redraws
```

Adjust scopes to the actual diff.

---

# 75. Quality Gates

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
git status --short
```

Normal workspace tests must never consume live Codex or Antigravity usage.

Live provider tests must be explicitly gated.

---

# 76. Dev Completion Report

Return:

```text
branch
base SHA
candidate SHA
commit sequence
changed files

Runtime management architecture
RuntimeRouter architecture
RuntimeIntegration API
profile schema
profile precedence
config migration/backward compatibility

Codex:
  health
  account
  model/reasoning
  capability verification
  execution

Antigravity:
  health
  account
  model/effort
  capability verification
  execution

TUI:
  command shell
  Home
  runtimes
  runtime detail
  settings
  doctor
  project discovery
  task review
  execution
  diff
  logs
  result

input batching
event loop behavior

persistence changes
migrations

security regressions
test matrix
Cargo gates

live Codex evidence
live Antigravity evidence

confirmation:
  no shell prompt
  no auto commit
  no auto push
  no GitHub ingress
  no MCP
  no M9 resolution workflow
  no M6 scheduler redesign
```

---

# 77. Acceptance Matrix

## Runtime Orchestration

```text
[ ] more than one real runtime can be registered
[ ] user can switch runtime easily
[ ] global default runtime/profile persists
[ ] project default may override global
[ ] one-run runtime selection works
[ ] running execution does not change provider mid-run
[ ] scheduler remains runtime-neutral
```

## Runtime Prerequisites

```text
[ ] installation status visible
[ ] version visible
[ ] authentication visible
[ ] account identity visible when safely provider-supported
[ ] incompatible CLI visible
[ ] profile visible
[ ] model visible
[ ] reasoning/effort visible
[ ] configured capabilities visible
[ ] effective capability state honest
```

## Codex

```text
[ ] M8 safety fixes preserved
[ ] provider-native configuration used safely
[ ] useful structured activity
[ ] no credential leakage
```

## Antigravity

```text
[ ] dedicated adapter
[ ] native headless execution
[ ] structured/machine event stream
[ ] model/effort mapping
[ ] provider-native mode/permissions mapping
[ ] cancellation
[ ] authentication probe
[ ] no credential leakage
```

## Terminal UX

```text
[ ] command-first
[ ] keybind-secondary
[ ] no implicit shell
[ ] activity-oriented Home
[ ] runtime dashboard
[ ] settings
[ ] doctor
[ ] discovery-first project onboarding
[ ] task execution envelope
[ ] useful execution activity
[ ] diff
[ ] logs
[ ] polished result
[ ] responsive paste
```

## Safety

```text
[ ] Project Registry remains path authority
[ ] M3 preflight remains authoritative
[ ] explicit Run remains mandatory
[ ] SQLite remains durable lifecycle authority
[ ] runtime profile is trusted local state
[ ] task cannot escalate permissions
[ ] auto commit off
[ ] auto push off
[ ] network off by default
[ ] extra writable roots absent by default
[ ] provider output remains untrusted
```

---

# 78. Final Product Contract

The implementation is complete only when Relay feels like an orchestration CLI rather than a provider wrapper.

Target mental model:

```text
Codex / Antigravity
    = coding executors

Relay
    = trusted local control plane
```

Normal user interaction should look like:

```text
> /tasks
> /projects
> /runtimes
> /runtime use antigravity/implement
> /run 81a7
> /diff 81a7
> /logs 81a7
> /settings
> /doctor
```

while Relay continuously presents:

```text
project
task
runtime
profile
model
reasoning/effort
permissions
execution activity
repository changes
verification
human attention
durable outcome
```

Permanent product principle:

> **Relay owns the execution envelope. Provider runtimes operate inside it.**

Permanent terminal principle:

> **Relay is a command-oriented terminal orchestration console for controlled coding-agent execution across projects and providers.**
