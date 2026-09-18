# Relay — Runtime Orchestration + Execution UX Implementation Context

**Status:** Draft implementation context for Dev expansion  
**Purpose:** Seed context for a bounded post-M8 orchestration / execution-UX pass  
**Origin:** M8 dogfood observations on native Windows Codex execution  
**Intended owner:** Dev Branch  
**Review expectation:** Dev should expand this into a concrete implementation plan before coding

---

# 1. Why this work exists

M8 proved that Relay can:

- discover the native Codex CLI;
- authenticate and validate CLI compatibility;
- launch Codex;
- stream Codex JSONL;
- execute commands through Codex;
- receive structured Relay outcomes;
- preserve the existing repository-safety / scheduler / persistence boundaries.

The dogfood run also exposed a different class of problem:

> Relay currently behaves more like a thin Codex launcher / debug console than a mature orchestration surface.

The runtime works, but the user experience does not yet clearly express:

- which AI model is being used;
- which reasoning effort is being used;
- which capabilities Relay granted to the runtime;
- whether those capabilities are actually effective;
- what the coding agent is doing right now;
- what files changed;
- what commands are running;
- what verification has passed or failed;
- what diff the agent produced;
- whether raw executor output is informational or actionable.

This pass should make Relay feel like a first-class terminal orchestrator rather than a wrapper around raw executor logs.

---

# 2. Product direction

Relay should own **execution orchestration**.

The user should approve:

```text
what task is being executed
which runtime profile is used
what capabilities that profile grants
what repository is trusted
what safety limits apply
```

The coding runtime should receive only the already-approved execution envelope.

The intended relationship is:

```text
User
  ↓
Relay Task Review
  ↓
Relay Runtime Profile
  ↓
Relay capability / safety policy
  ↓
Executor (Codex)
  ↓
Relay-normalized execution events
  ↓
Relay execution UI / result / diff / logs
```

Relay should not expose Codex-specific implementation details as the primary UX.

Codex is one execution engine.

Relay is the orchestrator.

---

# 3. Scope classification

This should be treated as a **bounded orchestration / execution UX pass**, not a redesign of the accepted M6 scheduler or M7 lifecycle.

Possible naming:

```text
M8.1 — Runtime Orchestration + Execution UX Polish
```

or:

```text
Post-M8 Orchestration Polish
```

The exact milestone label is a Master / project-governance decision.

This work should not become a reason to reopen already-accepted M6 persistence / scheduler semantics.

---

# 4. Dogfood observations that motivated this work

## 4.1 Runtime launch works

Native Windows Codex was confirmed available and executable.

Observed:

```text
Codex CLI:
codex-cli 0.147.0

Relay Doctor:
Codex available
authentication ready
exec compatibility ready
```

A direct Codex execution with the same general Relay sandbox intent succeeded.

This means the primary remaining product issue is not basic Codex connectivity.

---

## 4.2 Relay execution reached real Codex activity

Relay successfully surfaced:

```text
Codex session started
Codex started working
command execution events
structured result events
```

This confirms the runtime adapter is functional enough to support richer UI.

---

## 4.3 Effective write capability was unclear / failed

A dogfood task returned:

```text
implementation_failure

Could not apply the approved README-only change because the repository
was reported as read-only / write approval unavailable.
```

Relay was configured to request:

```text
sandbox: workspace-write
extra writable roots: none
network: off
Codex approval prompts: disabled
```

The key issue is not necessarily that Relay selected the wrong configured policy.

The issue is:

> Relay currently does not clearly distinguish **configured capability** from **effective runtime capability**.

The final product should not discover a required write-capability mismatch only after multiple agent attempts.

---

## 4.4 Execution UI feels like a debug console

Current execution output is primarily a large text stream.

Examples include full Windows shell-wrapper paths such as:

```text
C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe -Command ...
```

This is technically useful for diagnostics but poor as the default user-facing representation.

The user should normally see the logical action:

```text
Running git diff --check
```

not the entire provider-specific shell wrapper.

---

## 4.5 Live execution feedback is too sparse

Relay already consumes Codex JSONL, including:

```text
thread.started
turn.started
item.started
item.updated
item.completed
turn.completed
turn.failed
error
```

but the current UI does not yet turn those into a polished activity stream.

The result is long silent periods compared with Codex CLI / Codex desktop.

---

## 4.6 Paste responsiveness is poor in some terminals

Relay enables bracketed paste and supports a bulk `Paste` event.

However, some terminal environments appear to deliver pasted content as rapid individual key events.

The current loop handles one terminal event at a time, redraws, then sleeps/polls again.

Observed effect:

```text
large paste
→ appears character-by-character
→ visibly slow
```

This is a TUI event-loop / redraw problem, not a task-protocol problem.

---

# 5. Runtime Profile as a first-class Relay concept

The current `codex/default` profile should evolve from a mostly descriptive label into an actual trusted local execution policy.

A Runtime Profile should describe:

```text
runtime
model selection
reasoning effort
filesystem capability
network capability
extra writable roots
approval ownership
execution duration
repair budget
runtime-specific safe options
```

Example conceptual profile:

```text
Profile: Codex / Implement

Runtime
  Provider           Codex
  Profile ID         codex/implement

AI
  Model              Default
  Reasoning effort   High

Capabilities
  Filesystem         Workspace write
  Shell              Allowed inside sandbox
  Network            Off
  Extra write roots  None

Control
  Approval owner     Relay
  Maximum duration   30 minutes
  Repair attempts    2

Repository Safety
  Auto commit        Off
  Auto push          Off
  Remote mutation    Off
```

---

# 6. Suggested built-in profiles

Initial built-in profiles should stay small and understandable.

## Codex / Review

Purpose:

```text
inspection
analysis
review
explanation
```

Suggested policy:

```text
filesystem: read-only
network: off
reasoning: medium/default
```

## Codex / Implement

Purpose:

```text
normal coding tasks
```

Suggested policy:

```text
filesystem: workspace-write
network: off
reasoning: high/default configurable
```

## Codex / Deep

Purpose:

```text
difficult debugging
complex implementation
longer-running investigation
```

Suggested policy:

```text
filesystem: workspace-write
network: configurable only by trusted local setting
reasoning: high / xhigh
longer timeout
```

Do not add a "full machine access" profile by default.

Relay should continue to prefer bounded execution.

---

# 7. Model selection

Relay already has an internal Codex `model_override` concept.

This should become a user-visible trusted runtime setting.

Recommended behavior:

```text
Model:
  Default
  <known selectable models if discoverable>
  Custom...
```

`Default` should mean:

```text
do not pass an explicit model override;
allow the installed Codex CLI / account configuration to choose its default
```

Do not hardcode the product around one permanent model name.

Model catalogs can change.

If Relay later supports multiple runtimes, model selection should remain runtime-profile-specific.

---

# 8. Reasoning effort

Reasoning effort should also be part of the Runtime Profile.

Conceptual UI:

```text
Reasoning effort:
  Default
  Low
  Medium
  High
  XHigh
```

The exact supported values should be validated against the selected runtime/model.

Do not allow a remote / pasted task to silently escalate reasoning effort.

Reasoning effort is trusted local execution configuration.

---

# 9. Approval ownership

Relay should remain the approval owner.

The intended semantic meaning of:

```text
Codex interactive approvals disabled
```

is:

> Codex should not independently prompt the human because Relay already approved the execution envelope.

It should **not** mean:

> Codex has no capability to write.

Desired flow:

```text
User approves Relay Runtime Profile
        ↓
Relay grants exact capabilities
        ↓
Codex runs non-interactively within those capabilities
```

If Codex requires something outside the approved envelope, that should eventually become a Relay:

```text
NEEDS_DECISION
```

rather than an uncontrolled runtime prompt.

This aligns naturally with M9.

---

# 10. Configured capability vs effective capability

Relay should distinguish:

```text
Configured
```

from:

```text
Effective
```

Example:

```text
Filesystem

Configured:
  workspace-write

Effective:
  workspace-write ✓
```

If Relay cannot establish the expected capability:

```text
Configured:
  workspace-write

Effective:
  read-only ×
```

then an implementation task should be blocked or clearly degraded before spending multiple repair attempts.

The exact implementation may use:

- runtime self-check;
- controlled capability probe;
- provider-reported capability state;
- deterministic test operation;
- another safe runtime-specific verification mechanism.

Dev should propose the safest implementation.

Do not weaken the sandbox merely to make the check pass.

---

# 11. Task Review should show the execution envelope

The Task Review screen should become the place where the user understands what will actually happen.

Target information architecture:

```text
Task Review

Task
  Verify Codex connection

Project
  relay
  feat/m8-codex-runtime
  repository eligible
  worktree clean

Runtime
  Codex CLI
  Profile: Codex / Implement

AI
  Model: Default
  Reasoning: High

Permissions
  Read workspace        ✓
  Edit workspace        ✓
  Run commands          ✓
  Network               ×
  Outside workspace     ×
  Commit                ×
  Push                  ×
  Remote mutation       ×

Limits
  Maximum duration      30 min
  Repair attempts       2

[R] Run
[P] Change Profile
[B] Back
```

Do not make the user reason from raw Codex CLI flags.

Relay should translate profile intent into runtime-specific configuration.

---

# 12. Execution UI contract

Relay should feel like a first-class terminal application, not a raw stdout viewer.

Primary execution UX should prioritize:

```text
current phase
current activity
recent completed activities
files changed
verification
duration
runtime/model/profile
clear cancel/background controls
```

Raw executor logs should remain available but secondary.

---

# 13. Suggested Execution screen

Conceptual target:

```text
Relay / relay / feat/m8-codex-runtime

TASK
Verify Codex connection

Codex / Implement
Default model · High · Write · Offline

● Working  00:18

Current
  ● Running verification

Activity
  ✓ Inspected repository
  ✓ Read README.md
  ✓ Modified README.md
  ● Running git diff --check

Files
  M README.md

Verification
  ● git diff --check

[C] Cancel   [D] Diff   [L] Logs   [B] Background
```

The exact visual layout is open for Dev design.

---

# 14. Runtime event normalization

The UI should not be directly coupled to Codex JSON strings.

Recommended conceptual pipeline:

```text
Codex JSONL
    ↓
Codex runtime adapter
    ↓
Relay-normalized execution event
    ↓
Execution view model
    ↓
TUI
```

Possible normalized events:

```text
SessionStarted
ExecutionStarted
Analyzing
CommandStarted
CommandUpdated
CommandCompleted
FileChanged
VerificationStarted
VerificationCompleted
AgentMessage
ExecutionCompleted
ExecutionFailed
```

Do not expose hidden chain-of-thought.

Reasoning events should remain summarized into safe activity states such as:

```text
Analyzing task
Inspecting repository
Preparing change
```

---

# 15. `item.updated` / live command progress

Codex emits intermediate activity.

Relay should use intermediate events where available instead of waiting only for command completion.

Desired behavior:

```text
● Running cargo test

  compiling relay-core
  compiling relay-runtime-codex
  running tests
  84 / 143 passed
```

Then:

```text
✓ cargo test
```

All live output must remain bounded.

Do not allow unbounded terminal retention.

---

# 16. Clean logical command rendering

Default UX should show:

```text
git diff --check
cargo test --workspace
rg "RuntimeProfile"
```

not:

```text
C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -Command ...
```

The full raw executor command may remain available in:

```text
[L] Logs
```

or a debug/diagnostic view.

This is presentation cleanup.

Do not falsify the actual executed command.

---

# 17. Diff viewer

A coding orchestrator should make repository changes directly inspectable.

Add a first-class diff view:

```text
[D] Diff
```

Target capabilities:

```text
list changed files
show additions/removals
syntax / semantic highlighting where practical
scroll large diffs
show total + / - counts
bounded memory use
```

Example:

```diff
README.md

 Relay is a local handoff engine.

+ This line was added during the Relay dogfood validation.
```

The diff must be generated from trusted local repository state.

Do not accept executor-provided "diff summaries" as canonical repository truth.

---

# 18. Result screen improvements

The result screen should prioritize outcome and repository effects.

Suggested shape:

```text
RESULT — SUCCESS

Task
  Verify Codex connection

Runtime
  Codex / Implement
  Default model · High

Files changed
  M README.md

Changes
  +1  -0

Verification
  ✓ git diff --check

Repairs
  0

Repository
  clean before execution
  modified after execution
  no commit
  no push

Summary
  Added a small dogfood validation note.

[D] Diff   [L] Logs   [T] Tasks   [N] New Task
```

For failure:

```text
RESULT — IMPLEMENTATION FAILURE

Blocking capability
  Edit workspace

Configured
  workspace-write

Effective
  read-only

No repository changes were applied.
```

---

# 19. Semantic color system

Relay should use color as information, not decoration.

Suggested semantic roles:

```text
Accent
  active selection
  active task
  current phase

Success
  completed
  verified
  repository safe

Warning
  pending
  degraded
  needs decision
  capability mismatch

Error
  failed
  blocked
  durability uncertainty

Muted
  IDs
  timestamps
  paths
  secondary metadata

Diff Added
  additions

Diff Removed
  removals
```

Do not hard-code a large arbitrary theme surface.

Keep the palette small and semantic.

---

# 20. Input / paste responsiveness

Relay currently supports bracketed paste, but some terminals may fall back to rapid key events.

The event loop should not redraw after every queued character.

Recommended behavior:

```text
wait for first event
    ↓
consume / drain all immediately queued input events
    ↓
update state
    ↓
draw once
```

For an 800-character paste:

```text
desired:
one visible update or a very small number of redraws

not:
800 visible redraws
```

The exact batching strategy should avoid starving runtime output or cancellation input.

---

# 21. Event-driven TUI responsiveness

The current TUI uses a polling loop with draw/poll/sleep behavior.

Dev should evaluate moving toward a more event-driven model where the screen redraws when one of these changes:

```text
terminal input
runtime execution event
scheduler state
health state
timer tick for elapsed time / spinner
```

Goal:

```text
runtime event arrives
→ state updates promptly
→ UI redraws promptly
```

Avoid unnecessary fixed sleep latency.

Maintain bounded CPU usage.

---

# 22. Raw Logs view

Raw logs still matter for diagnosis.

Add or preserve a secondary view:

```text
[L] Logs
```

It may contain:

```text
stdout
stderr
raw logical command
provider-specific diagnostic messages
bounded runtime events
```

The normal execution view should not be the raw log viewer.

---

# 23. Runtime/settings screen

`/settings` should evolve from informational text into an actual trusted local control surface.

Conceptual target:

```text
Settings

Runtime
  Profile               Codex / Implement
  Model                 Default
  Reasoning effort      High

Permissions
  Workspace             Read + Write
  Network               Off
  Extra writable roots  None

Execution
  Max duration           30 min
  Repair attempts        2
  Global concurrency     2
  Per-project            1

Repository
  Auto commit            Off
  Auto push              Off

[Enter] Change
[R] Reset
[Esc] Back
```

Dev should decide which settings are persisted and where.

Do not allow remote task payloads to mutate these trusted settings.

---

# 24. Runtime profile persistence

Dev should propose the minimal persistence model.

Possible direction:

```text
Relay local config
```

with runtime profiles such as:

```toml
[runtimes.codex.profiles.implement]
model = "default"
reasoning_effort = "high"
filesystem = "workspace-write"
network = false
max_duration_secs = 1800
repair_attempts = 2
```

This is illustrative only.

The final schema must stay compatible with the current Relay config ownership model.

Do not introduce another database solely for runtime settings.

---

# 25. Profile selection precedence

Recommended precedence:

```text
explicit trusted local selection for this run
    ↓
project default runtime profile
    ↓
Relay global default runtime profile
    ↓
fail closed if no safe profile can be resolved
```

A HandoffTask may request a logical stack.

It should not directly control privileged runtime capabilities.

---

# 26. Security invariants that must remain frozen

Do not weaken these while improving UX:

```text
trusted project path comes from Project Registry
remote task does not choose cwd
explicit Run approval remains required
auto-commit remains off
auto-push remains off
remote mutation remains disallowed
network remains off unless explicitly enabled by trusted local policy
extra writable roots remain empty unless explicitly trusted
task text remains stdin / structured runtime input
no shell concatenation of untrusted task text
same-project scheduler isolation remains authoritative
different-project execution remains bounded
durability invariant remains unchanged
```

---

# 27. Relationship with M9

This pass should make M9 cleaner.

If an executor needs capability outside the approved Runtime Profile:

```text
needs network
needs additional write root
needs architecture change
needs destructive operation
needs privilege escalation
```

Relay should eventually convert that into:

```text
NEEDS_DECISION
```

M9 then owns:

```text
request packet
planner handoff
resolution packet
resume / restart / cancel disposition
```

Do not implement the full M9 decision loop inside this polish pass.

Only expose the execution/capability information M9 will need.

---

# 28. Non-goals

Do not add in this pass:

```text
GitHub Issue ingress
GitHub task polling
External Binding behavior
Relay Direct / MCP
auto-commit
auto-push
generic shell console
generic /exec
generic /git
arbitrary full-machine write access
runtime marketplace
provider-specific UI clone
chain-of-thought display
M6 scheduler redesign
M5 persistence redesign
```

---

# 29. Suggested implementation surfaces

Likely affected areas:

```text
apps/relay-tui/
    execution rendering
    result rendering
    settings rendering
    input event loop
    diff/log views
    semantic theme helpers

runtimes/relay-runtime-codex/
    runtime profile config
    model override
    reasoning effort mapping
    capability configuration
    provider event → normalized Relay event mapping

crates/relay-runtime/
    only if a small provider-neutral execution-event/profile abstraction
    is genuinely required

crates/relay-core/
    only if trusted runtime profile resolution belongs in Core
```

Prefer keeping provider-specific Codex options inside the Codex adapter.

Avoid leaking Codex CLI flags into generic Core types.

---

# 30. Recommended design questions for Dev

Before coding, Dev should explicitly answer:

1. Where do Runtime Profiles live?
2. Which settings are global vs per-project?
3. How is a one-run profile override represented?
4. How is reasoning effort validated per runtime/model?
5. How does Relay determine configured vs effective capabilities?
6. Which Codex events should become provider-neutral runtime events?
7. How are live command updates bounded?
8. How is raw log retention bounded?
9. How is the diff generated and bounded?
10. How should TUI input batching work without delaying cancellation?
11. What existing M7/M8 types can be reused?
12. What is the minimum shared interface change, if any?

---

# 31. Suggested acceptance criteria

## Runtime orchestration

```text
[ ] user can inspect active Runtime Profile before Run
[ ] model selection is visible
[ ] reasoning effort is visible
[ ] filesystem capability is visible
[ ] network capability is visible
[ ] runtime profile is trusted local state
[ ] HandoffTask cannot silently escalate permissions
```

## Capability handling

```text
[ ] configured capability is distinguishable from effective capability
[ ] required workspace-write mismatch fails clearly
[ ] capability failure does not burn unnecessary repair attempts
[ ] no sandbox weakening is introduced to mask failures
```

## Execution UX

```text
[ ] current activity is visible
[ ] command start is visible
[ ] useful command updates are visible
[ ] command completion is visible
[ ] file changes are visible
[ ] verification progress is visible
[ ] raw PowerShell wrapper noise is hidden by default
[ ] raw logs remain available
```

## Diff

```text
[ ] changed files can be inspected
[ ] additions/removals are readable
[ ] diff comes from local trusted repository state
[ ] large diffs remain bounded / scrollable
```

## Responsiveness

```text
[ ] normal paste does not visibly render one character per frame
[ ] key-event fallback paste is batched
[ ] runtime events update the screen promptly
[ ] cancellation input remains responsive
[ ] no high idle CPU regression
```

## Visual hierarchy

```text
[ ] semantic colors distinguish success/warning/error/current/muted
[ ] execution screen is scannable without reading raw logs
[ ] result screen clearly distinguishes outcome, changes, verification, and repository state
```

## Regression

```text
[ ] M6 scheduler isolation remains intact
[ ] M6 durability semantics remain intact
[ ] M7 explicit Run remains intact
[ ] M8 Codex process isolation remains intact
[ ] auto-commit remains off
[ ] auto-push remains off
[ ] network remains off by default
```

---

# 32. Testing expectations

Dev should add deterministic tests for:

```text
runtime profile resolution
trusted profile precedence
model override command mapping
reasoning effort command/config mapping
capability mapping
effective capability failure state
provider event normalization
item.updated handling
logical command rendering
raw log preservation
diff generation
diff bounds
paste batching
event-loop responsiveness where testable
semantic result-state mapping
```

Windows-specific runtime tests should remain platform-gated where necessary.

Do not remove existing M8 process-tree / launcher regressions.

---

# 33. Quality gates

Use the normal workspace gates:

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
git status --short
```

---

# 34. Dev deliverable expected

Dev should expand this context into a concrete implementation plan before coding.

The expanded plan should include:

```text
final scope
files/types to change
runtime-profile schema
TUI state changes
normalized event model
settings persistence
capability verification approach
diff implementation
input-loop design
migration/backward-compatibility impact
test matrix
commit strategy
explicit non-goals
```

Then implement as one bounded batch.

---

# 35. Product UX contract

The target product principle for this pass is:

> Relay should feel like a first-class terminal orchestrator. Runtime configuration, execution state, repository changes, verification, and required human action should be understandable without reading raw provider logs.

And:

> Relay owns the execution envelope. The coding runtime operates inside it.

This should remain true even when Relay later supports executors other than Codex.
