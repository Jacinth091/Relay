# Relay — M8.1 DEV Gate Remediation Plan for Candidate 45b8008

**Status:** RETURN TO DEV  
**Date:** 2026-08-15  
**Candidate reviewed:** `45b8008bea8b3639d3cbf33d9aa84895fd674cfd`  
**Branch:** `feat/m8-codex-runtime`  
**Direct parent:** `8e73af10459838510868c91e66dc3d44e9acf3f5`  
**Purpose:** Close the remaining correctness blockers before M8.1 is forwarded to the Review Branch.

---

# 1. DEV Verdict

```text
M8.1 — Multi-Runtime Orchestration + Terminal UX

Candidate:
45b8008bea8b3639d3cbf33d9aa84895fd674cfd

Search-style command palette:
SUBSTANTIAL PASS

Overall M8.1 architecture:
FAIL / REMEDIATION REQUIRED

DEV VERDICT:
RETURN TO DEV

READY FOR REVIEW:
NO
```

The command palette has landed and is much closer to the intended UX, but the branch still contains runtime-routing, completion-correctness, provider, and execution-envelope defects that should be fixed before independent Review.

---

# 2. What Passes

## Command palette structure

Implemented:

```text
structured CommandSpec catalog
command categories
argument kinds
provider completions
profile completions
task completions
search/ranking
vertical popup rendering
selection marker
Up / Down
Tab completion
Enter execution/advancement
Esc close
max-visible list
dynamic task/provider/profile completion paths
```

## Read-only command split

The previous `/diff` and `/logs` helper bug was partially corrected:

```text
/diff and /logs now select a task
instead of directly calling the Run helper
```

Keep that correction.

## ExecutionRunPolicy plumbing

Core now supports:

```text
submit_with_policy(...)
queued ExecutionRunPolicy
per-run runtime_profile_id
per-run max_duration
per-run repair limit
```

This is the correct additive direction.

## RuntimeRouter

`RuntimeRouter` now implements the `Runtime` trait and can route a `RuntimeRequest` by profile prefix.

Keep this implementation and wire it into production composition.

---

# 3. R-DEV-01 — BLOCKER: Runtime switching is still cosmetic

## Current problem

The TUI scheduler still uses:

```rust
Arc::new(AppRuntime::default())
```

in both initial construction and scheduler rebind.

Production `AppRuntime::default()` is Codex.

Therefore:

```text
/runtime use antigravity/implement
        ↓
TUI active_profile_id changes
        ↓
ExecutionRunPolicy says antigravity/implement
        ↓
scheduler Runtime object is still AppRuntime::Codex
        ↓
CodexRuntime.start(...) receives an Antigravity profile
```

The new `RuntimeRouter` exists but is not the runtime owned by the production scheduler.

## Required fix

Build one production router:

```text
RuntimeRouter
  codex -> CodexRuntime
  antigravity -> AntigravityRuntime
```

Then construct:

```rust
ExecutionScheduler<RuntimeRouter>
```

instead of:

```rust
ExecutionScheduler<AppRuntime>
```

Production scheduler lifetime:

```text
startup
  ↓
one router-backed scheduler
  ↓
future executions route by frozen profile ID
```

Do not rebuild the scheduler merely because the global default runtime changes.

## Required tests

```text
codex/implement → Codex fixture runtime
antigravity/implement → Antigravity fixture runtime

global default switch after Task A is queued
→ Task A retains original provider

Task B queued after switch
→ Task B uses new provider
```

No live providers in normal tests.

---

# 4. R-DEV-02 — BLOCKER: TUI preflight is still Codex-only

## Current problem

Current UI preflight still checks:

```text
Codex runtime health
```

regardless of selected profile.

It also treats only:

```text
chatgpt-codex
```

as a supported effective stack.

Therefore Antigravity cannot participate in the same trusted Run path correctly.

## Required fix

Resolve the effective local execution profile before Run:

```text
selected task
    ↓
trusted runtime/profile selection
    ↓
selected provider
    ↓
provider health
    ↓
provider profile resolution
    ↓
requested-stack compatibility
    ↓
repository eligibility
```

Do not hard-code Codex into `ui_preflight()`.

Recommended output:

```rust
RunPreflightView {
    eligible: bool,
    profile_id: String,
    provider_id: String,
    runtime_health: RuntimeProbeState,
    repository: ...,
    stack_compatibility: ...,
    blocked_reason: Option<String>,
}
```

---

# 5. R-DEV-03 — MAJOR: Runtime labels still use `AppRuntime::default()`

## Current problem

Task Review and execution/result presentation still derive runtime identity from the application default runtime in places.

That can display:

```text
Codex
```

for an execution whose frozen run policy is Antigravity, or display the new global default for an older queued/running task.

## Required fix

Runtime display must come from the execution/task's frozen policy.

For queued/running work preserve:

```text
runtime_profile_id
provider_id
```

in the execution view.

For persisted execution history, read:

```text
executions.runtime_profile
runtime_metadata_json
```

where available.

Never render active execution provider from the mutable global default.

---

# 6. R-DEV-04 — MAJOR: Runtime settings file exists but is not actually integrated

## Current problem

`runtime_settings.rs` exists, but production `main.rs` does not load and use it as the trusted runtime configuration source.

`/runtime use` currently mutates only:

```text
active_runtime_provider
active_profile_id
```

in memory.

It does not validate the profile through the runtime integration and does not persist the selected default.

## Required fix

At startup:

```text
load runtimes.toml
    ↓
validate version/schema
    ↓
resolve global default
    ↓
initialize active/default profile
```

For:

```text
/runtime use <profile>
```

perform:

```text
lookup provider
resolve profile
confirm provider/profile exists
persist trusted runtime settings
update global default
```

Unknown profile:

```text
fail closed
no AppState mutation
```

Do not silently map arbitrary strings to Codex.

---

# 7. R-DEV-05 — MAJOR: Palette task candidates use the wrong executable rule

## Current problem

All task suggestions use:

```rust
executable: t.eligible_for_run
```

regardless of command.

The same `TaskCompletion` is reused for:

```text
/run
/cancel
/diff
/logs
```

This produces incorrect behavior.

### `/cancel`

Cancellable tasks are normally:

```text
RUNNING
VERIFYING
QUEUED
```

and therefore:

```text
eligible_for_run == false
```

Their suggestions become non-executable.

Pressing Enter on the suggestion only rewrites the prompt and cannot execute cancellation.

### `/diff` and `/logs`

Completed/failed/running tasks may be inspectable but not READY.

Their suggestions likewise become disabled.

## Required fix

Do not use one Run-only boolean as universal command eligibility.

Preferred:

```rust
TaskCompletion {
    id,
    project,
    title,
    state,
}
```

The command-specific list itself determines whether the suggestion is executable.

Or introduce:

```rust
TaskCompletionCapabilities {
    runnable,
    cancellable,
    inspectable,
}
```

Then:

```text
/run     → executable if runnable
/cancel  → executable if cancellable
/diff    → executable if inspectable
/logs    → executable if inspectable
```

## Mandatory regression

```text
RUNNING task
/cancel 
→ selected suggestion Enter executes Cancel path

DONE task
/diff
→ selected suggestion Enter opens Diff

FAILED task
/logs
→ selected suggestion Enter opens Logs

none of these queue a new execution
```

---

# 8. R-DEV-06 — MAJOR: Runtime-profile suggestions are hard-coded as available

## Current problem

Completion context currently creates:

```text
codex/*
antigravity/*
```

profiles with:

```text
available = true
disabled_reason = None
```

regardless of:

```text
installation
authentication
compatibility
profile resolution
capability mismatch
```

This contradicts the command-palette design.

## Required fix

Populate profile candidates from:

```text
RuntimeRouter / RuntimeIntegration
        +
cached RuntimeProbe state
```

Example:

```text
antigravity/implement
× Not authenticated
```

or:

```text
codex/deep
! incompatible with installed runtime/model
```

Unavailable candidates may remain visible for discovery but:

```text
Enter must not activate them
```

---

# 9. R-DEV-07 — MAJOR: `/runtime use` still accepts arbitrary profiles

## Current problem

Manual input such as:

```text
/runtime use nonsense/profile
```

can mutate active profile state because the execution path checks only whether the string starts with `antigravity/`; otherwise it treats it as Codex.

## Required fix

`RuntimeUse` execution must:

```text
parse provider prefix
lookup registered provider
integration.resolve_profile(profile)
verify current provider health
persist only after validation
```

Unknown profile:

```text
Unknown runtime profile 'nonsense/profile'.
Try /runtime use ...
```

No mutation.

---

# 10. R-DEV-08 — MAJOR: `/runtime test <provider>` ignores the provider

## Current problem

The parser preserves the provider argument, but the execution branch discards it and triggers a full runtime health refresh.

## Required fix

Implement provider-specific probe:

```text
/runtime test codex
→ Codex only

/runtime test antigravity
→ Antigravity only
```

`/runtimes` and `/doctor` may trigger all-provider refresh.

---

# 11. R-DEV-09 — BLOCKER: Antigravity structured result path is incomplete

## Current problem

The new Antigravity launch command correctly moved toward:

```text
agy -p
--output-format stream-json
--sandbox
```

However the implementation still creates:

```text
relay-agy-<id>-result.json
```

and later requires that file to exist.

The current launch argv does not pass a result-file path to AGY.

Therefore successful execution can reach:

```text
read_result()
```

and fail because Relay created a path but AGY was never instructed to write that file.

## Required fix

Use one coherent current provider-native result contract.

Preferred:

```text
stream-json terminal result event
+
--json-schema <Relay structured result schema>
```

Parse the final structured result directly from the provider's machine stream.

Do not maintain a temp result file unless the current provider CLI explicitly supports writing one and Relay actually supplies that option.

---

# 12. R-DEV-10 — BLOCKER: Antigravity event parser still expects Codex-shaped events

## Current problem

The launch path now requests Antigravity:

```text
stream-json
```

but the reader still recognizes events such as:

```text
thread.started
turn.started
turn.completed
turn.failed
```

Those were inherited from the earlier Codex-shaped design.

The provider-native machine stream must be parsed according to the current Antigravity schema.

## Required fix

Implement current provider-native event parsing.

Conceptual mapping:

```text
init
→ RuntimeActivity::SessionStarted

step_update
→ normalized Command / AgentMessage / FileChanged / progress activity

result
→ terminal structured Relay outcome
```

Unknown well-formed event:

```text
tolerate / diagnostic
```

Malformed event:

```text
InvalidOutput
```

Do not require `turn.completed` unless the actual installed/provider contract emits it.

---

# 13. R-DEV-11 — MAJOR: Antigravity capability verification is still false-positive

## Current problem

Workspace write is still reported `Verified` when the working directory merely exists.

This is not evidence of effective provider write capability.

## Required fix

Use:

```text
Configured
Verified
Unknown
Mismatch
Unavailable
```

honestly.

Until provider-supported proof exists:

```text
Workspace write
Configured
Effective: Unknown
```

is acceptable.

False `Verified` is not.

---

# 14. R-DEV-12 — MAJOR: Antigravity model catalog remains hard-coded

## Current problem

The adapter still advertises static model IDs.

Provider catalogs are time-varying.

## Required fix

Use:

```text
provider-native discovery
```

when there is a stable machine-readable interface.

Otherwise expose:

```text
Default
```

only.

A trusted custom override may be accepted later through runtime settings after validation.

Do not claim guessed model IDs are available.

---

# 15. R-DEV-13 — MAJOR: Command palette root works, but descriptions/availability are not yet fully contextual

The visual/search mechanics are good enough to keep.

Do not rewrite the palette.

Correct only its source data:

```text
providers
profiles
tasks
availability
disabled reasons
```

The palette should remain a pure read-only view of already-loaded Relay state.

No provider probes, SQLite calls, Git commands, or execution should occur merely because the user types.

---

# 16. R-DEV-14 — MAJOR: Project discovery still falls directly into the manual form

## Current problem

On both successful discovery and discovery failure, the current implementation calls the AddProject form.

The intended UX is:

```text
Detected repository
→ confirmation selector

Detection failure
→ choose directory / Advanced / Cancel
```

not:

```text
open six-field form immediately
```

## Required fix

Introduce explicit project-add modes:

```rust
Detecting
Candidate
NoRepository
Advanced
```

Only `Advanced` opens the manual form.

---

# 17. R-DEV-15 — MAJOR: Input/event-loop remediation is still pending

The palette commit did not modify `events.rs`.

Therefore the prior source-aligned requirements still need explicit closure:

```text
bounded rapid input drain
liveness tick decoupled from every key
dirty-state draw only when needed
safe input diagnostics
```

Do not regress the already-fixed:

```text
KeyEventKind Press/Repeat/Release behavior
bulk Event::Paste
frame-clock redraw
```

---

# 18. Required Implementation Order

Do not start with more visual polish.

## Phase 1 — Execution truth

```text
1. wire RuntimeRouter into production scheduler
2. make preflight provider-neutral
3. persist/validate runtime selection
4. fix runtime labels to frozen execution policy
```

## Phase 2 — Palette correctness

```text
5. command-specific task executability
6. runtime profile availability from real health/profile data
7. validate RuntimeUse
8. provider-specific RuntimeTest
```

## Phase 3 — Antigravity correctness

```text
9. correct stream-json event parser
10. correct structured terminal result path
11. truthful capability verification
12. model discovery/default behavior
```

## Phase 4 — UX remainder

```text
13. discovery-first project candidate view
14. finish event-loop batching/liveness separation
15. semantic runtime/task review/execution presentation
```

Then run dogfood.

---

# 19. Required Tests

Add regressions for:

```text
/runtime use antigravity/implement
→ next queued task dispatches Antigravity fixture

/runtime use codex/implement
→ next queued task dispatches Codex fixture

queued task provider immutable after default switch

/cancel running-task suggestion
→ executable

/diff done-task suggestion
→ executable
→ no queue

/logs failed-task suggestion
→ executable
→ no queue

unavailable runtime profile
→ palette visible but disabled

/runtime use invalid/profile
→ no state mutation

/runtime test codex
→ does not probe Antigravity

Antigravity stream:
init
step_update
result
→ normalized valid execution

Antigravity missing result
→ InvalidOutput

workspace directory exists
→ does NOT automatically mean write Verified
```

---

# 20. Manual Acceptance

Before DEV PASS:

## Palette

```text
/
→ vertical list

/ru
→ filtered list

Up/Down
→ selection

Tab
→ completion

Enter
→ action
```

## Runtime routing

```text
/runtime use antigravity/implement
/run <task>
→ actual execution record/provider is Antigravity
```

not merely label.

## Cancel

```text
/cancel 
→ running task candidate
→ Enter invokes cancellation
```

## Diff

```text
/diff 
→ completed task candidate
→ Enter opens diff
→ no execution queued
```

## Antigravity

```text
Relay remains terminal owner
provider machine stream parses
terminal result accepted
no provider TUI
```

---

# 21. Quality Gates

Run:

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
git status --short
```

Reported green tests are necessary but not sufficient.

Live provider dogfood remains separate from normal Cargo tests.

---

# 22. Required DEV Completion Report

```text
M8.1

Base:
45b8008bea8b3639d3cbf33d9aa84895fd674cfd

Successor:
<SHA>

R-DEV-01 RuntimeRouter production wiring:
RESOLVED

R-DEV-02 provider-neutral preflight:
RESOLVED

R-DEV-03 frozen runtime labels:
RESOLVED

R-DEV-04 runtime settings integration:
RESOLVED

R-DEV-05 command-specific task palette eligibility:
RESOLVED

R-DEV-06 runtime-profile availability:
RESOLVED

R-DEV-07 RuntimeUse validation:
RESOLVED

R-DEV-08 provider-specific RuntimeTest:
RESOLVED

R-DEV-09 Antigravity structured result:
RESOLVED

R-DEV-10 Antigravity stream parser:
RESOLVED

R-DEV-11 truthful capabilities:
RESOLVED

R-DEV-12 model catalog:
RESOLVED

R-DEV-14 project discovery:
RESOLVED

R-DEV-15 event-loop remainder:
RESOLVED

cargo fmt:
PASS

cargo clippy:
PASS

cargo test:
PASS

Windows command palette acceptance:
PASS

Codex dogfood:
PASS

Antigravity dogfood:
PASS

DEV VERDICT:
PASS — READY FOR REVIEW
```

---

# 23. Final Gate

Do not forward M8.1 to Review while any of these are true:

```text
/runtime use antigravity changes labels but scheduler owns Codex

/cancel suggestions are disabled because the task is not READY

/diff or /logs suggestions require READY state

Antigravity waits for a result file it never asked AGY to create

Antigravity parses Codex-style turn events from a provider-native stream

workspace write is called Verified because a directory exists

runtime profiles are always shown available

invalid runtime profile strings mutate the active runtime

project discovery still defaults to the manual six-field form
```

The palette UX is worth keeping.

The remaining work is to make the **state behind the palette truthful and executable**.
