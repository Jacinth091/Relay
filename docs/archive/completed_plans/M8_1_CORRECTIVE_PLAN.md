# Relay — M8.1 Corrective Runtime Isolation + Terminal UX Overhaul Implementation Plan

**Status:** DEV corrective implementation plan  
**Date:** 2026-08-15  
**Current implementation under correction:** `015e614`  
**Scope:** Runtime isolation, multi-runtime orchestration UX, command-first TUI, responsiveness, semantic color system, project onboarding, runtime prerequisites, execution activity, diff/logs/result presentation  
**Primary design authorities:**
- `10-DEVELOPMENT/reference/TERMINAL-UX-GUIDELINES.md`
- `RELAY_RUNTIME_ORCHESTRATION_EXECUTION_UX_IMPLEMENTATION_CONTEXT.md`
- `RELAY_MULTI_RUNTIME_ORCHESTRATION_TUI_IMPLEMENTATION_PLAN.md`
- accepted M2–M7 contracts
- M8 Codex Runtime review/remediation requirements

---

# 1. Why This Corrective Pass Is Required

The current implementation contains several of the requested feature names, but it does **not yet implement the intended product behavior**.

The current implementation should not be considered complete merely because it contains:

```text
/runtimes
/runtime use ...
AntigravityRuntime
RuntimeProfile
RuntimeRouter
Diff
Logs
Doctor
```

The implementation is complete only when those features behave according to the Relay product and UX contracts.

Current observed gaps include:

```text
1. Antigravity can take over / overwrite Relay's terminal UI.
2. The TUI still behaves primarily like the old M7 full-screen/keybind application.
3. Slash commands are layered over legacy screens instead of defining the primary interaction model.
4. The persistent command-shell concept is incomplete.
5. The Activity Home is still too close to a debug/status screen.
6. Legacy shortcut legends remain prominent.
7. Semantic colors/theme roles were not implemented.
8. The fixed poll/sleep event loop does not implement the proposed responsive/event-driven model.
9. Rapid key-event paste can still cause excessive per-character work/redraw behavior.
10. Project onboarding still retains too much manual-form behavior.
11. Runtime prerequisites/account/profile information is not yet a polished orchestration dashboard.
12. Configured-vs-effective capabilities are not honestly verified in all adapters.
13. Antigravity provider behavior is modeled too similarly to Codex instead of through its own verified machine interface.
14. Provider activity is not yet consistently normalized into Relay-owned execution presentation.
```

This pass is therefore an **architecture-preserving corrective implementation**, not cosmetic polish.

---

# 2. Permanent Product Rules

Freeze these rules before coding.

## 2.1 Relay owns the terminal

> **A provider runtime must never render its own interactive TUI over Relay.**

Codex, Antigravity, and future provider executors are worker processes behind Relay.

The only user-facing orchestration TUI is Relay.

## 2.2 Relay Core is provider-neutral; integrations are provider-native

```text
Relay Scheduler / Core
    provider-neutral

Codex adapter
    Codex-native

Antigravity adapter
    Antigravity-native

Relay TUI
    provider-aware
```

Do not force provider adapters into fake identical CLI semantics.

## 2.3 Relay is command-first and keybind-secondary

Primary interaction:

```text
> /tasks
> /projects
> /runtimes
> /runtime use antigravity/implement
> /run 81a7
```

Secondary interaction:

```text
Up / Down
Enter
Esc
Tab
```

Legacy single-letter shortcuts may remain temporarily as compatibility accelerators, but must not be the primary visible product language.

## 2.4 Relay owns lifecycle truth

The TUI never invents:

```text
RUNNING
DONE
FAILED
CANCELLED
NEEDS_DECISION
```

Durable Core/SQLite state remains authoritative.

## 2.5 Provider output is untrusted

Provider output:

```text
does not execute commands
does not change Relay state directly
does not select paths
does not select runtime permissions
does not render terminal control sequences directly
```

---

# 3. Frozen Architecture Boundaries

Do not reopen:

```text
M2 Project Registry
M3 Repository Safety
M4 HandoffTask semantics
M5 SQLite canonical state
M6 scheduler/recovery/cancellation
M7 explicit Run approval
```

Specifically:

```text
IMPORT != EXECUTE
Project Registry owns trusted path
M3 preflight remains mandatory
M6 scheduler remains authoritative
SQLite remains lifecycle authority
auto-commit remains OFF
auto-push remains OFF
```

---

# 4. Correct Target Architecture

```text
                         Relay TUI
                            │
      ┌─────────────────────┼─────────────────────┐
      │                     │                     │
 Command Shell        Orchestration UI      Runtime UI
      │                     │                     │
      └─────────────────────┴──────────┬──────────┘
                                       ↓
                                  Relay Core
                                       ↓
                              ExecutionScheduler
                                       ↓
                                  RuntimeRouter
                         ┌─────────────┴─────────────┐
                         ↓                           ↓
                    CodexRuntime             AntigravityRuntime
                         │                           │
                machine/headless mode       machine/headless mode
                         │                           │
                    piped events                piped events
                         └─────────────┬─────────────┘
                                       ↓
                             Relay-normalized activity
                                       ↓
                                  Relay TUI only
```

Never:

```text
Relay alternate screen
        ↓
provider interactive TUI
        ↓
provider takes terminal ownership
```

---

# 5. Stage 0 — Freeze the Baseline

Before modifications:

```text
git status --short
git rev-parse HEAD
cargo test --workspace
```

Record:

```text
current SHA
current test count
current runtime behavior
current terminal takeover reproduction
current project-add behavior
current paste behavior
```

Capture a small manual reproduction checklist.

Do not stage/commit until the first corrective checkpoint is green.

---

# 6. Stage 1 — Fix Antigravity Terminal Isolation First

This is the first blocking correction.

Do not work around the problem by repainting Relay after Antigravity exits.

Relay must prevent the provider from owning the terminal in the first place.

## 6.1 Machine-interface-only rule

The Antigravity adapter must use a **documented, non-interactive, machine-readable provider mode**.

Before coding:

```text
1. inspect installed `agy --help`;
2. inspect the current official Antigravity CLI documentation;
3. identify the supported non-interactive / print / structured-stream interface;
4. build a compatibility probe for those exact capabilities;
5. fail closed when the required machine mode is unavailable.
```

Do not assume that Codex-like:

```text
exec
--json
--output-last-message
```

semantics apply to Antigravity.

Provider adapters must use provider-native contracts.

## 6.2 No provider interactive fallback

If the required headless/machine mode is missing:

```text
Runtime health = Incompatible
Run disabled for that provider
```

Do not silently fall back into the Antigravity interactive terminal UI.

Do not allow an authentication prompt, settings TUI, alternate screen, or model picker to appear over Relay during execution.

## 6.3 Child stdio isolation

For execution workers:

```text
stdin  = piped
stdout = piped
stderr = piped
```

Never inherit the Relay terminal for normal execution workers.

## 6.4 Windows console isolation

Add an explicit Windows process-isolation helper.

Conceptually:

```text
ProviderProcessIsolation::BackgroundWorker
```

When launching provider workers on Windows:

```text
- use a no-console/no-window creation mode suitable for piped worker execution;
- keep stdout/stderr pipes;
- ensure `.cmd` launchers are also isolated when wrapped through cmd.exe;
- ensure cancellation targets only that worker process tree;
- do not launch a provider in a new interactive console.
```

Use a small platform-specific helper rather than scattering creation flags through adapters.

## 6.5 POSIX terminal isolation

For Unix-like systems:

```text
- create an isolated process group/session as appropriate;
- do not allow the provider worker to acquire Relay's controlling terminal;
- retain process-tree cancellation.
```

Do not use terminal inheritance.

## 6.6 Terminal-control output sanitization

Provider output rendered by Relay must sanitize control bytes.

At minimum detect/remove or visibly escape:

```text
alternate-screen enter/leave
cursor positioning
terminal reset
OSC sequences
arbitrary ANSI control sequences
```

Do not feed provider bytes directly to Crossterm/Ratatui as terminal commands.

Raw logs may preserve a safe escaped representation.

## 6.7 Auth interaction rule

Runtime probe may determine:

```text
NotAuthenticated
```

but a Relay Run must not trigger an interactive provider login flow over Relay.

For this pass:

```text
NotAuthenticated → Run disabled + actionable message
```

Relay does not manage provider credentials.

## 6.8 Terminal isolation acceptance

Required manual acceptance:

```text
1. Start Relay.
2. Select Antigravity runtime.
3. Run a low-risk task.
4. Antigravity process starts.
5. Relay remains visible for the entire execution.
6. Antigravity never switches the terminal buffer.
7. Relay receives structured activity.
8. Cancel the execution.
9. Relay remains usable.
10. Cursor/raw mode/bracketed paste remain correct.
```

Also verify provider success, failure, cancellation, crash, and invalid-output cases all return cleanly to Relay.

---

# 7. Stage 2 — Correct Antigravity Provider Semantics

## 7.1 Remove Codex-shaped assumptions

Do not model Antigravity by copying Codex flags and event names unless verified.

Create provider-native modules conceptually:

```text
command.rs
probe.rs
stream.rs
outcome.rs
capabilities.rs
```

inside `relay-runtime-antigravity`.

## 7.2 Compatibility probe

Probe the exact required machine capabilities:

```text
installed
version
authentication
machine/headless execution
structured stream
structured terminal result
model selection
effort/reasoning control
permission/mode controls
```

Do not report `Ready` unless all capabilities required by the selected Relay profile are compatible.

## 7.3 Model catalog

Do not permanently hard-code guessed provider model IDs.

Preferred precedence:

```text
provider-native model discovery
    ↓
validated local static compatibility list when provider discovery unavailable
    ↓
Default only
```

If Relay cannot safely discover models:

```text
Model: Default
```

is acceptable.

Do not invent unsupported models.

## 7.4 Effective capability verification

Current logic must not treat:

```text
working directory exists
```

as proof of:

```text
filesystem write capability verified
```

For every capability:

```text
Configured
Verified
Unknown
Mismatch
Unavailable
```

must be honest.

A provider-specific safe probe may establish effective write when appropriate.

Never weaken the sandbox to make the probe succeed.

---

# 8. Stage 3 — Refactor TUI Structure Before Further Visual Work

The current TUI should not continue growing primarily inside `main.rs`.

Recommended target:

```text
apps/relay-tui/src/

main.rs
app.rs
events.rs
commands.rs
project_discovery.rs
theme.rs

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
    home.rs
    runtime.rs
    execution.rs
```

Required separation:

```text
application state
terminal event pump
command parsing
theme
view models
rendering
provider execution
```

must no longer be entangled.

---

# 9. Stage 4 — Implement the Semantic Theme System

The proposed semantic colors are a requirement, not optional decoration.

Create a theme model conceptually containing:

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

Recommended first default:

```text
Accent        Cyan
Success       Green
Warning       Yellow
Error         Red
Muted         DarkGray
Text          terminal default foreground
Selection     Cyan + Bold or Reverse
Diff Added    Green
Diff Removed  Red
```

## 9.1 Semantic styling helpers

Create helpers such as:

```text
style_status(...)
style_runtime_health(...)
style_task_state(...)
style_wait_reason(...)
style_attention(...)
style_diff_line(...)
style_muted(...)
```

Do not scatter arbitrary color literals across render functions.

## 9.2 Color semantics

Use:

```text
● active / ready       accent/success
◌ waiting              muted/warning
! attention            warning
✓ success              success
× failure              error
? unknown              muted/warning
```

Text state remains visible without color.

## 9.3 Theme acceptance

The following must visibly use the semantic theme:

```text
Home
Tasks
Projects
Runtimes
Runtime Detail
Task Review
Execution
Result
Doctor
Diff
Logs warnings/errors
command suggestions
feedback/errors
```

If only one or two screens are colored, the theme is not complete.

---

# 10. Stage 5 — Replace the Legacy Full-Screen Layout Philosophy

Stop treating every view as one giant full-terminal bordered block.

Target normal layout:

```text
Header
────────────────────────────────────────

Main content

────────────────────────────────────────
context / suggestions
> /command
```

Borders are allowed for focused editors, modal confirmations, and bounded diff/log viewports when useful, but should not wrap the entire terminal by default.

## 10.1 Persistent header

Example:

```text
Relay                                  Codex ● Implement
```

or:

```text
Relay                         Antigravity ● Implement
```

## 10.2 Persistent command footer

Normal views end with:

```text
────────────────────────────────────────
> /_
```

Optional muted hint:

```text
/help for commands
```

Do not primarily render legacy shortcut legends.

---

# 11. Stage 6 — Make the Command Shell the Actual Primary Interaction Model

Slash commands must not merely redirect into unchanged M7 legacy forms.

Required command family:

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
/runtime use <profile>
/runtime test <provider>
/settings
/doctor
/diff [task]
/logs [task]
/help
/quit
```

## 11.1 Explicit input modes

Implement:

```text
CommandPrompt
Selection
TextEditor
Confirmation
```

Normal typing goes to the command prompt.

Inside selectors:

```text
Up/Down
Enter
Esc
```

remain valid.

## 11.2 Command suggestions

Typing:

```text
/pro
```

shows:

```text
/projects
/project add
```

## 11.3 No implicit shell

These remain inert:

```text
git status
cargo test
rm -rf .
powershell -Command ...
cmd /C ...
```

Response:

```text
Unknown Relay command. Try /help.
```

---

# 12. Stage 7 — Replace Dashboard With the Designed Activity Home

Do not show the raw internal counter wall.

## 12.1 Idle

```text
 Relay                                  Codex ● Implement

 No active work.

 Projects  2
 Tasks     12

 Recent
 ✓ relay        M8 Codex Runtime
 ✓ practicore   Placement reassignment

────────────────────────────────────────
> /_
```

## 12.2 Active

```text
 Relay                                  Codex ● Implement

 2 active · 1 waiting · 1 attention

 ● relay
   Runtime isolation remediation
   Codex / Implement · RUNNING · 02:14

 ● practicore
   Placement update
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

## 12.3 First run

```text
 Relay                                  No runtime selected

 Welcome to Relay.

 No trusted projects are configured.

 Try:
   /project add
   /runtimes
   /help

────────────────────────────────────────
> /_
```

Do not show zero-value categories.

---

# 13. Stage 8 — Runtime Dashboard Must Become a Real Orchestration Surface

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
   Account        user@example.com / unavailable
   Profile        Antigravity / Implement
   Model          Default
   Effort         High

────────────────────────────────────────

Default: Codex / Implement

> /_
```

## 13.1 Runtime health

Show:

```text
Installed
Version
Authenticated
Account identity when safely available
Compatibility
Last probe
```

Do not scrape credential stores.

## 13.2 Runtime detail

Show:

```text
profile
model
reasoning/effort
configured capabilities
effective capabilities
duration
repair budget
```

Use semantic colors for Verified, Configured, Unknown, Mismatch, and Unavailable.

## 13.3 Runtime switching

Easy runtime switching:

```text
/runtime use codex/implement
/runtime use antigravity/implement
```

must update the trusted default selection and must not alter active executions.

---

# 14. Stage 9 — Fix Discovery-First Project Onboarding

`/project add` with no path inspects the current working directory.

`/project add <path>` inspects the supplied local directory.

Use direct read-only Git process arguments.

Detect:

```text
Git root
remote
repository name
suggested ProjectKey
GitHub owner/repository when derivable
suggested runtime/profile
```

Normal confirmation:

```text
 Add Project

 Detected repository

 Name       Relay
 Key        relay
 Path       C:\Users\PCC\Desktop\My Projects\Relay
 Remote     github.com/Jacinth091/Relay
 Runtime    Codex / Implement

 > Add this repository
   Edit details
   Choose another directory
   Advanced setup
   Cancel

────────────────────────────────────────
> /_
```

The six-field manual form belongs only under `Advanced setup`.

If `/project add` still opens the blank form first, this requirement is not implemented.

---

# 15. Stage 10 — Redesign New Task Flow

`/new`:

```text
 New Task

 > Paste task
   Import task file
   Back

────────────────────────────────────────
> /_
```

Do not make `F` or `V` the required discovery mechanism.

Multi-line paste/editor is an explicit `TextEditor` focus mode.

---

# 16. Stage 11 — Task Review Must Show the Execution Envelope

Before Run:

```text
 Task

 ID           81a7...
 Project      relay
 State        READY
 Repository   ✓ eligible

 Runtime
   Antigravity / Implement

 AI
   Model        Default
   Effort       High

 Capabilities
   Read workspace       ✓
   Edit workspace       ✓ / ? / ×
   Run commands         ✓
   Network              ×
   Extra write roots    none
   Commit               ×
   Push                 ×

 Limits
   Maximum duration     30m
   Repairs              2

 > Run
   Change runtime/profile
   Details
   Back

────────────────────────────────────────
> /_
```

Configured/effective distinctions must be visible where relevant.

Run remains explicit.

---

# 17. Stage 12 — Normalize Provider Activity Before Rendering

Provider JSON/NDJSON must not directly define TUI text.

Pipeline:

```text
Provider stream
    ↓
provider parser
    ↓
RuntimeActivity
    ↓
ExecutionViewModel
    ↓
Relay rendering
```

Provider-neutral activities may include:

```text
SessionStarted
Analyzing
CommandStarted
CommandProgress
CommandCompleted
FileChanged
VerificationStarted
VerificationProgress
VerificationCompleted
AgentMessage
Warning
ProviderDiagnostic
```

Do not expose chain-of-thought.

Do not infer durable state from activity.

---

# 18. Stage 13 — Redesign Execution Screen

Target:

```text
 relay · Runtime isolation remediation

 Antigravity / Implement
 Default model · High · Write · Offline

 ● RUNNING                                    02:14
   Attempt 1/2

 Current
   ● Running cargo test --workspace

 Activity
   ✓ Inspected repository
   ✓ Read Antigravity adapter
   ✓ Modified process isolation
   ● Running cargo test --workspace

 Files
   M runtimes/relay-runtime-antigravity/src/lib.rs
   M apps/relay-tui/src/events.rs

 Verification
   ● cargo test --workspace

────────────────────────────────────────
> /_

/cancel  /diff  /logs
```

Normal execution view must hide provider shell-wrapper noise.

Raw provider diagnostics belong in Logs.

---

# 19. Stage 14 — Diff, Logs, and Result

## Diff

Source of truth:

```text
trusted local Git repository
```

Features:

```text
changed file list
+/- counts
scrollable bounded patch
semantic added/removed colors
large-diff truncation notice
```

Read-only.

## Logs

Bounded diagnostic view:

```text
stdout
stderr
escaped provider diagnostics
actual launcher details when useful
```

No terminal control execution.

## Result

Target:

```text
 RESULT — SUCCESS

 Runtime
   Antigravity / Implement

 Files changed
   M ...

 Changes
   +42  -8

 Verification
   ✓ cargo fmt --check
   ✓ cargo test --workspace

 Repository
   modified
   no commit
   no push

 Summary
   ...

 > Diff
   Logs
   Tasks
```

Failure should make blockers obvious.

---

# 20. Stage 15 — Replace the Fixed Poll/Sleep Loop With a Responsive Event Pump

The proposed responsiveness logic is a requirement.

Do not retain correctness around:

```text
poll(...)
handle one event
draw
sleep(...)
repeat
```

as the primary architecture.

Preferred direction:

```text
Crossterm EventStream
        +
scheduler/runtime events
        +
health/config events
        +
bounded frame clock
        ↓
tokio::select!
```

Conceptual event loop:

```rust
loop {
    tokio::select! {
        terminal_event = terminal_events.next() => {
            apply_terminal_event(...);
            dirty = true;
        }

        execution_event = execution_events.recv() => {
            apply_execution_event(...);
            dirty = true;
        }

        health_event = health_events.recv() => {
            apply_health_event(...);
            dirty = true;
        }

        _ = frame_tick.tick(), if dirty => {
            draw();
            dirty = false;
        }

        _ = elapsed_tick.tick(), if active_execution_exists => {
            update_elapsed();
            dirty = true;
        }
    }
}
```

Exact implementation may differ.

Required properties:

```text
no fixed sleep after every key
input can drain rapidly
runtime events update promptly
redraw is coalesced
idle CPU remains low
```

---

# 21. Stage 16 — Paste Batching and Redraw Coalescing

Bracketed `Event::Paste`:

```text
append as one state update
```

Fallback rapid key events:

```text
process rapidly
do not redraw after every character
```

Target:

```text
800-character paste
→ state updated rapidly
→ a small number of frames
```

Not:

```text
800 characters
→ ~800 visible redraws
```

Use frame-rate/coalescing logic rather than arbitrary sleeps.

Cancellation and Esc must remain responsive during large input bursts.

---

# 22. Stage 17 — Runtime Health Must Be Asynchronous and Lazy

Do not block startup probing every provider.

Startup:

```text
probe active/default runtime
```

Lazy:

```text
/runtimes
/doctor
/runtime test <provider>
```

probe others asynchronously.

TUI should show:

```text
○ Checking...
```

rather than freezing.

Do not probe every frame.

---

# 23. Stage 18 — Runtime Switching Must Rebind Safely

Changing the default runtime/profile:

```text
does not affect active execution
does not destroy queued state
does not bypass M7 scheduler rebind protections
```

If scheduler composition must be rebuilt, do so only when safe.

Prefer a true RuntimeRouter that avoids TUI scheduler recreation for every provider switch.

---

# 24. Stage 19 — Tests for Terminal Isolation

Add deterministic fixture processes.

Test child that attempts:

```text
alternate screen sequence
cursor reset
ANSI clear-screen
large stdout
large stderr
exit failure
hang until cancellation
```

Verify:

```text
Relay sanitizes output
Relay terminal state survives
worker cancellation isolated
output remains bounded
```

Windows-specific integration verifies worker no-console isolation.

POSIX-specific integration verifies worker cannot acquire Relay controlling terminal.

---

# 25. Stage 20 — Theme Tests

Test semantic mapping functions:

```text
Ready            success
Running          accent
Queued           muted/warning
NeedsDecision    warning
Failed           error
PersistenceFail  error
Unknown          warning/muted
DiffAdd          success
DiffRemove       error
```

Prefer semantic-style tests over brittle full terminal escape-code snapshots.

---

# 26. Stage 21 — Event Loop / Responsiveness Tests

Add tests or deterministic harnesses for:

```text
rapid 1,000-key burst
single bulk Paste
runtime event during input burst
cancel during input burst
health update during execution
resize event
idle loop
```

Acceptance:

```text
no starvation
no key-per-frame requirement
no excessive idle CPU
no provider output delay tied to keyboard input
```

---

# 27. Stage 22 — UX Acceptance Tests

Explicitly verify:

```text
[ ] command prompt visible on normal views
[ ] /help discoverable
[ ] old keybind legend no longer primary
[ ] Home uses activity design
[ ] no giant full-screen border by default
[ ] semantic colors visible
[ ] /runtimes is a real dashboard
[ ] runtime detail shows prerequisites
[ ] /project add auto-detects repo
[ ] manual project form is Advanced
[ ] /new uses an interactive selector
[ ] Task Review shows runtime envelope
[ ] Execution shows activities/files/verification
[ ] Diff uses +/- semantic color
[ ] Logs secondary
[ ] Result scannable
[ ] paste responsive
```

---

# 28. Hard "Do Not Call This Implemented If..." Criteria

The corrective pass is **NOT COMPLETE** if any of these remain true:

```text
[ ] Antigravity can replace Relay's terminal UI.
[ ] Provider execution can enter its interactive TUI.
[ ] Provider auth can unexpectedly open over Relay during Run.
[ ] capability verification treats directory existence as write verification.
[ ] Antigravity model IDs are guessed/hard-coded without verified support.
[ ] help text still presents N/T/P/A/R/C as the primary UI.
[ ] normal views are still giant full-screen bordered panels.
[ ] semantic theme/colors are absent from primary views.
[ ] Activity Home still behaves like a counter/debug page.
[ ] command prompt disappears across normal navigation.
[ ] /project add opens the manual form first.
[ ] event loop still relies on per-event fixed sleep/poll behavior.
[ ] large paste visibly renders character-by-character.
[ ] provider machine output directly controls terminal presentation.
[ ] Execution screen is primarily raw stdout/stderr.
[ ] raw shell-wrapper commands dominate normal activity display.
[ ] runtime dashboard cannot distinguish installed/auth/compatible/profile/capability state.
```

This checklist is a release gate.

---

# 29. Recommended Commit Sequence

Keep corrections reviewable.

```text
1. fix[Jacinth]: isolate provider workers from Relay terminal
2. fix[Jacinth]: correct Antigravity machine runtime integration
3. refactor[Jacinth]: split Relay terminal application surfaces
4. feat[Jacinth]: add semantic terminal theme
5. feat[Jacinth]: make Relay command shell primary
6. feat[Jacinth]: implement orchestration activity home
7. feat[Jacinth]: complete runtime dashboard and prerequisites
8. feat[Jacinth]: make project onboarding discovery-first
9. feat[Jacinth]: improve task review execution envelope
10. feat[Jacinth]: normalize runtime activity and execution views
11. feat[Jacinth]: add styled diff logs and results
12. perf[Jacinth]: make terminal event loop responsive
```

Adjust if implementation dependencies require fewer commits.

Do not combine all work into one opaque commit.

---

# 30. Quality Gates

After each logical checkpoint and at the end:

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

Normal tests must not consume live Codex or Antigravity usage.

Run provider live tests only through explicit manual/integration gates.

---

# 31. Manual End-to-End Acceptance

## 31.1 Relay Startup

Expected:

```text
Relay opens promptly
active runtime health visible
command prompt visible
no giant debug dashboard
```

## 31.2 Project Add

```text
/project add
```

Expected:

```text
current Git repository detected
suggestions shown
manual fields not required
explicit confirmation
```

## 31.3 Runtime Dashboard

```text
/runtimes
```

Expected:

```text
Codex status
Antigravity status
authentication
version
profile
model
reasoning/effort
capabilities
```

## 31.4 Runtime Switch

```text
/runtime use antigravity/implement
```

Expected:

```text
future Run uses Antigravity
active work unaffected
```

## 31.5 Antigravity Run

Expected:

```text
Relay remains visible
AGY never takes over terminal
activity appears inside Relay
files/verification update
cancel works
```

## 31.6 Codex Run

Expected:

```text
same Relay orchestration UX
provider-specific adapter details remain behind Relay
```

## 31.7 Large Paste

Expected:

```text
bulk input feels immediate
no character-by-character repaint crawl
```

## 31.8 Result

Expected:

```text
semantic success/failure
files changed
diff
verification
runtime/profile
no commit
no push
```

---

# 32. Completion Report Required From Dev

Return:

```text
branch
base SHA
successor SHA
commit list
changed files

ANTIGRAVITY TERMINAL ISOLATION
- machine mode used
- Windows isolation
- POSIX isolation
- TTY ownership evidence
- cancellation evidence

ANTIGRAVITY PROVIDER CORRECTNESS
- compatibility probe
- model discovery strategy
- capability verification
- structured stream mapping

TUI ARCHITECTURE
- files/modules
- focus model
- command shell
- activity home

SEMANTIC THEME
- roles
- mappings
- screens using theme

RUNTIME UX
- dashboard
- details
- account handling
- profiles
- switching

PROJECT UX
- auto detection
- advanced fallback

EXECUTION UX
- normalized activities
- files
- verification
- diff
- logs
- result

RESPONSIVENESS
- event-pump architecture
- frame coalescing
- paste batching
- idle behavior

TESTS
- terminal isolation
- theme
- commands
- runtime dashboard
- project discovery
- execution
- paste/input
- existing M2-M8 regressions

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

NO LIVE PROVIDER USE DURING NORMAL TESTS:
PASS

NO AUTO COMMIT:
PASS

NO AUTO PUSH:
PASS

M6 SCHEDULER CONTRACT CHANGES:
NONE / explicitly list deviation
```

---

# 33. Final Acceptance Principle

The work is accepted only when this mental model is true:

```text
Antigravity
Codex
future providers
      ↓
background/machine workers
      ↓
Relay-normalized activity
      ↓
Relay orchestration UI
```

and never:

```text
Relay
 ↓
nested provider TUI
 ↓
provider owns terminal
```

The target terminal product must feel like:

> **a responsive, command-oriented orchestration CLI with a coherent semantic visual language — not a debug dashboard with slash-command aliases.**

The permanent runtime rule is:

> **Provider workers never own Relay's terminal.**

The permanent UX rule is:

> **Commands are primary, keybinds are accelerators, activity is concise, colors are semantic, and the UI redraws because state changed — not because a fixed polling loop happened to wake up.**
