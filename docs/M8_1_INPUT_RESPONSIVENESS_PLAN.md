# Relay — M8.1 Input Correctness, Responsiveness & Terminal UX Completion Plan

**Status:** DEV remediation required  
**Date:** 2026-08-15  
**Current verified candidate:** `4962fe82f8e08b836a0072fd4c6c52a80cae22aa`  
**Parent corrective commit:** `f49c8fad1807dbeca303b39dbd79ecbd4cc0efea`  
**Scope:** Duplicate keyboard input, key-repeat policy, input batching, redraw coalescing, semantic-theme completion, remaining legacy-screen cleanup, and final UX acceptance.

---

# 1. Why This Remediation Exists

The current implementation is materially improved, but it should **not** be declared complete yet.

A live Windows dogfood test exposed a user-facing input correctness defect:

```text
physical keypress
    ↓
Relay receives more than one KeyEvent
    ↓
each event is treated as text input
    ↓
character appears twice / input feels hypersensitive
```

Observed symptom:

```text
user types:  codex
Relay may show: ccooddeexx
```

This is an input correctness blocker.

The implementation also still falls short of the approved terminal UX contract in several areas:

```text
1. KeyEventKind is not filtered before text mutation.
2. Rapid key events are not actually coalesced before redraw.
3. The frame clock does not own dirty-frame rendering.
4. Some normal screens still use giant Borders::ALL panels.
5. Result/Execution still contain old keybind-oriented UI text.
6. The semantic Theme exists, but most views are still plain strings.
7. Diff added/removed semantic colors are defined but not actually rendered line-by-line.
8. Runtime health/task state markers are largely unstyled strings.
9. The Theme struct is not the actual source used by style helpers.
10. The command-first shell is improved, but legacy screen behavior remains mixed into the product.
```

This plan corrects those remaining defects without changing M2–M8 Core semantics.

---

# 2. Verified Root Cause — Duplicate Keyboard Input

Current event flow:

```text
Crossterm EventStream
    ↓
Event::Key(key)
    ↓
handle_key(state, key)
    ↓
KeyCode::Char(c)
    ↓
push(c)
```

The current event pump does not filter:

```text
KeyEventKind::Press
KeyEventKind::Repeat
KeyEventKind::Release
```

before calling `handle_key`.

The current `handle_key` also does not inspect `key.kind`.

Therefore any terminal/backend that reports multiple key event kinds for one physical action can mutate Relay state multiple times.

This is especially important on Windows terminals.

---

# 3. Correct Keyboard Event Contract

Relay needs one centralized keyboard policy.

Create conceptually:

```rust
fn should_dispatch_key(key: &KeyEvent, context: InputContext) -> bool
```

or:

```rust
enum RelayKeyDisposition {
    Dispatch,
    Ignore,
}
```

The event pump, not every screen, should own the policy.

---

# 4. KeyEventKind Policy

## 4.1 Release

Always ignore:

```text
KeyEventKind::Release
```

for normal Relay interaction.

A release event must never:

```text
append text
delete text
submit a command
navigate
confirm
cancel
quit
```

## 4.2 Press

Always dispatch:

```text
KeyEventKind::Press
```

subject to normal command/screen handling.

## 4.3 Repeat

Repeat must be context-sensitive.

### Text entry

For:

```text
CommandPrompt
TextEditor
AddProject manual editor
ImportFile path editor
```

initial safe policy:

```text
printable character Repeat → IGNORE
```

This removes hypersensitive duplicate text and prevents OS repeat behavior from flooding text entry.

If intentional held-key text repetition is desired later, implement explicit repeat throttling rather than inheriting backend behavior blindly.

### Navigation

For:

```text
Up
Down
PageUp
PageDown
Left
Right
```

repeat may be accepted so users can hold navigation keys.

### Backspace/Delete

Allow controlled repeat.

Backspace should feel normal when held.

Do not process both Press and Release.

### Enter/Esc/Tab/commands

Ignore Repeat.

A held Enter must not submit twice.

A held Esc must not unwind multiple screens.

A held slash must not repeatedly enter command mode.

---

# 5. Add Explicit Input Context

Do not infer repeat behavior ad hoc from individual screens.

Introduce:

```rust
enum InputContext {
    CommandPrompt,
    TextEditor,
    Selection,
    Confirmation,
    PassiveView,
}
```

AppState should expose:

```rust
fn input_context(&self) -> InputContext
```

Examples:

```text
command_mode == true       → CommandPrompt
PasteTask                  → TextEditor
ImportFile                 → TextEditor
AddProject Advanced        → TextEditor
NewTask selector           → Selection
Tasks                      → Selection
Runtime list               → Selection
cancel confirmation        → Confirmation
Home                       → PassiveView
Execution normal view      → PassiveView
```

Then the event layer can make one consistent repeat decision.

---

# 6. Centralize Key Filtering in `events.rs`

Target:

```rust
match event {
    Event::Key(key) => {
        if !should_dispatch_key(&key, state.input_context()) {
            continue;
        }

        dispatch_key(state, key).await?;
        dirty = true;
    }
}
```

Do not scatter:

```rust
if key.kind == ...
```

through twenty screens.

Tests should call this policy directly.

---

# 7. Required Duplicate-Input Regression Tests

Add unit tests for:

```text
Press('a')    → "a"
Release('a')  → no change

Press('a')
Release('a')
              → "a", NOT "aa"

Press('a')
Repeat('a')
Release('a')
              → "a" in command/text entry

Press('/')
Release('/')
              → enters command mode once

Press(Enter)
Release(Enter)
              → command submitted once

Press(Backspace)
Repeat(Backspace)
Release(Backspace)
              → controlled deletion

Press(Down)
Repeat(Down)
Repeat(Down)
Release(Down)
              → navigation can move repeatedly
```

Add Windows-oriented regression coverage where practical.

---

# 8. Do Not Use Time-Based Debounce for Normal Typing

Do **not** solve duplicate input with:

```text
ignore same character if within N milliseconds
```

That would incorrectly break legitimate fast typing such as:

```text
book
cool
committee
```

and would make Relay feel laggy.

The fix must be based primarily on:

```text
KeyEventKind
+
input context
```

not character equality and wall-clock timing.

---

# 9. Current Redraw Logic Is Still Not Properly Coalesced

Current conceptual flow is:

```text
loop:
    state.tick()
    if dirty:
        draw immediately

    select:
        receive one key
        dirty = true

next loop:
    draw immediately
```

So even though `EventStream` replaced fixed `poll()`, rapid fallback key events can still result in:

```text
key
draw
key
draw
key
draw
```

This does not satisfy the approved batching goal.

---

# 10. Frame Clock Must Own Normal Redraw

Change the rendering model.

Target:

```text
input/runtime/health events
        ↓
mutate state
        ↓
dirty = true

frame clock
        ↓
if dirty
    draw once
    dirty = false
```

Do not draw immediately at the top of every loop after one event.

Recommended frame cap:

```text
30–60 FPS
```

30 FPS is sufficient for the first alpha.

Input processing must remain faster than the display refresh rate.

---

# 11. Correct Event Pump Shape

Conceptual target:

```rust
loop {
    tokio::select! {
        terminal = reader.next() => {
            handle_terminal_event(...);
            // no immediate draw
        }

        scheduler = execution_rx.recv() => {
            handle_execution_event(...);
            // no immediate draw
        }

        health = health_rx.recv() => {
            handle_health_event(...);
            // no immediate draw
        }

        _ = frame_tick.tick() => {
            if dirty {
                terminal.draw(...)?;
                dirty = false;
            }
        }

        _ = elapsed_tick.tick(), if state.has_active_execution() => {
            state.update_elapsed();
            dirty = true;
        }
    }
}
```

The exact implementation may vary.

The behavioral contract must not.

---

# 12. Drain Rapidly Queued Terminal Events

When one terminal event arrives, opportunistically drain immediately available events before the next frame.

Conceptually:

```text
first terminal event
    ↓
handle it
    ↓
drain ready events up to bounded batch size/time
    ↓
mutate state
    ↓
one frame
```

Use a bound such as:

```text
max N events per drain
or
max ~1–2 ms processing slice
```

to avoid starving runtime events.

Do not create an unbounded input loop.

---

# 13. True Paste Event

For:

```rust
Event::Paste(String)
```

continue to apply the whole paste as one state mutation.

Do not iterate through characters via fake `KeyEvent`s.

Expected:

```text
800-character paste
→ one input mutation
→ one/few frames
```

---

# 14. Fallback Paste as Rapid Key Events

Some terminals do not deliver `Event::Paste`.

For rapid printable-key bursts:

```text
Press events
```

process them quickly into the current text buffer while holding redraw until the frame tick.

Do not visually redraw each character.

This is the principal fallback batching requirement.

---

# 15. Input Latency Acceptance

Manual target:

```text
ordinary typing:
appears immediate

fast typing:
no missed/doubled characters

held printable key:
does not explode text

Backspace hold:
usable

arrow hold:
usable

large paste:
does not crawl character-by-character

runtime activity:
continues updating

cancel:
responsive during input burst
```

---

# 16. Remaining Theme Defect

A semantic `Theme` type exists, but the current style helper functions use hard-coded `Color::*` values directly.

This means:

```text
Theme
```

is currently more descriptive than authoritative.

Correct this.

---

# 17. Make Theme the Actual Style Source

Prefer:

```rust
impl RelayTheme {
    fn accent(&self) -> Style
    fn success(&self) -> Style
    fn warning(&self) -> Style
    ...
}
```

or equivalent.

App-level rendering should receive:

```text
&RelayTheme
```

Do not keep:

```rust
pub fn style_success() -> Style {
    Style::default().fg(Color::Green)
}
```

while separately defining:

```rust
Theme { success: ... }
```

The theme should be the source of truth.

---

# 18. Required Semantic Theme Roles

At minimum:

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

Default:

```text
Accent        Cyan
Success       Green
Warning       Yellow
Error         Red
Muted         DarkGray
Text          terminal default
Selection     Cyan + Bold
Diff Added    Green
Diff Removed  Red
```

No meaning may depend on color alone.

---

# 19. Theme Must Be Applied to Real Content

The following must use styled `Line` / `Span` content, not plain formatted Strings only:

```text
Home activity states
Tasks states
Project safety
Runtime readiness
Runtime capabilities
Doctor checks
Task Review capabilities
Execution activity
Result outcome
Diff lines
Warnings/errors
Command suggestions
Selection markers
```

Examples:

```text
✓ Ready         success
● Running       accent
◌ Waiting       muted/warning
! Needs action  warning
× Failed        error
? Unknown       warning/muted
```

---

# 20. Diff Styling Is a Required Acceptance Item

Current semantic diff styles are defined, but the Diff view must actually parse/display:

```text
+ added line
- removed line
@@ hunk
file header
context
```

with:

```text
+       diff_added
-       diff_removed
@@      accent/muted
header  muted/accent
```

Do not color the whole diff one color.

Do not treat `+++` / `---` file headers as ordinary added/removed content without special handling.

---

# 21. Runtime Dashboard Styling

`/runtimes` should use semantic spans:

```text
Codex
  ● Ready                 green
  Profile  Implement      muted/accent
  Model    Default        normal

Antigravity
  × Not authenticated     red
```

Selected runtime:

```text
> Antigravity
```

should use selection styling.

Do not rely only on ASCII markers.

---

# 22. Capability Styling

Runtime detail:

```text
Workspace read       ✓ Verified
Workspace write      ? Unknown
Commands             ✓ Verified
Network              × Disabled
```

Mapping:

```text
Verified     success
Configured   accent/muted
Unknown      warning
Mismatch     error
Unavailable  error/muted
Disabled     muted
```

---

# 23. Remaining Layout Defect — Giant Borders

Normal views should not use:

```rust
Block::default()
    .title(...)
    .borders(Borders::ALL)
```

for the whole main content area.

Review and remove from normal:

```text
Home
Tasks
Projects
New Task
Task Review
Execution
Result
Runtimes
Settings
Doctor
```

Borders may remain for focused modal/editor content where they help.

---

# 24. Specifically Remove Remaining Legacy Panels

The verified candidate still retains bordered normal views in places such as Home, Execution, and Result.

Correct them to:

```text
Header
blank/section hierarchy
content
footer
```

not:

```text
┌──────────────────────────────────────┐
│ everything                           │
└──────────────────────────────────────┘
```

---

# 25. Remove Legacy Result Shortcuts From Primary Content

Do not render:

```text
[T] Tasks
[D] Dashboard
[N] New Task
```

as primary Result UX.

Replace with:

```text
Try:
  /diff
  /logs
  /tasks
  /new
```

or interactive selectable actions consistent with the command-first shell.

Legacy T/D/N accelerators may still work invisibly during transition.

---

# 26. Remove Legacy Execution Shortcut Language

Do not make:

```text
C: cancel
B: background
```

the product vocabulary.

Normal execution view:

```text
/cancel <task>
/diff
/logs
/tasks
```

Focused selectors/confirmations can still use Enter/Esc.

---

# 27. Complete Activity Home Styling

Target:

```text
Relay                              Codex ● Implement

2 active · 1 waiting · 1 attention

● relay
  Runtime isolation remediation
  Codex / Implement · RUNNING · 02:14

◌ relay
  Runtime docs
  WAITING · project slot

! practicore
  Migration redesign
  NEEDS DECISION

────────────────────────────────────────
> /_
```

Style:

```text
RUNNING             accent
WAITING             muted/warning
NEEDS DECISION      warning
FAILED              error
DONE                 success
project names        normal/accent
secondary metadata   muted
```

---

# 28. Persistent Command Prompt Must Be Visually Active

When command mode is active:

```text
> /runt
```

style:

```text
prompt marker    accent
typed text       normal
suggestions      muted/selection
```

When inactive:

```text
> /_
```

keep it visible but muted.

Do not visually treat the entire footer as muted if the user is actively typing.

---

# 29. Command Suggestions

Show one clear active suggestion, with others muted.

Example:

```text
> /run

  /run <task>       ← selected
  /runtime
  /runtimes
```

Support:

```text
Up/Down
Tab
Enter
```

if command-completion interaction already exists or can be added without scope blowup.

At minimum, typing suggestions must not overwhelm the input line.

---

# 30. Sensitive Keyboard Behavior Acceptance

Manually test in the actual Windows terminal used for Relay.

Required sequences:

```text
abcdefg
committee
bookkeeper
// intentionally repeated chars

/runtime
/projects
/project add

hold a
hold Backspace
hold Down
tap Enter once
tap Esc once
```

Expected:

```text
no doubled characters
no accidental duplicate commands
no double screen transitions
no hypersensitive selection
```

Test in at least:

```text
Windows Terminal / PowerShell environment used for dogfood
```

If practical, also:

```text
cmd-hosted terminal
VS Code integrated terminal
```

---

# 31. Key Event Diagnostic Mode for DEV

Add an optional developer-only diagnostic, disabled by default.

Example environment flag:

```text
RELAY_TUI_INPUT_TRACE=1
```

It may log safely to a file or bounded diagnostic buffer:

```text
KeyEventKind
KeyCode
modifiers
timestamp
input context
disposition
```

Do not print diagnostics directly into the normal alternate-screen UI.

Do not log pasted task contents or secrets.

This is useful for backend-specific key anomalies.

Remove or keep gated after remediation.

---

# 32. Do Not Add Arbitrary Debounce Delays

Do not fix sensitivity by sleeping:

```text
20 ms after each key
50 ms debounce
```

That would regress responsiveness.

Correct architecture:

```text
event-kind filtering
+
context-aware repeat
+
state updates immediately
+
frame-coalesced rendering
```

---

# 33. Scheduler/Runtime Polling in Event Loop

The current event loop calls application `tick()` before each wait cycle.

Evaluate whether this performs unnecessary async work on every key.

Preferred long-term model:

```text
scheduler event channel
runtime event channel
health event channel
bounded periodic liveness tick
```

If changing this now risks M7 liveness semantics, retain `tick()` as a bounded safety tick but **do not bind it to every keyboard event**.

M7 scheduler-driver liveness must remain preserved.

---

# 34. Proposed Event Sources

Target event pump:

```text
Terminal(Event)
Scheduler/Core activity
Runtime activity
Health probe completion
Frame tick
Elapsed-time tick
Liveness tick
```

Use:

```rust
tokio::select!
```

with explicit dirty-state management.

---

# 35. Frame Scheduling Rules

Recommended:

```text
dirty = true
```

on:

```text
input mutation
screen transition
runtime event
scheduler event
health change
resize
feedback update
```

Draw:

```text
on next frame tick
```

Immediate draw may be used only for:

```text
initial frame
terminal restoration/safety-critical transition
```

not every key.

---

# 36. Low-CPU Acceptance

Idle Relay should not redraw at 30 FPS with no state change.

Frame tick may continue, but:

```text
dirty == false
→ no draw
```

Elapsed tick should only mark dirty when:

```text
active execution exists
```

Health probes should not poll every frame.

---

# 37. Tests — Event Filtering

Add:

```text
release_ignored
press_dispatched
repeat_printable_ignored_in_text
repeat_navigation_allowed
repeat_backspace_allowed
repeat_enter_ignored
repeat_escape_ignored
```

---

# 38. Tests — Command Input

Simulate:

```text
Press(c), Release(c)
Press(o), Release(o)
Press(d), Release(d)
Press(e), Release(e)
Press(x), Release(x)
```

Expected:

```text
codex
```

not:

```text
ccooddeexx
```

Also:

```text
committee
bookkeeper
```

must preserve intentional repeated characters when generated as separate Press events.

---

# 39. Tests — Render Coalescing

Use a fake event source if practical.

Inject:

```text
100 key Press events
```

within one frame period.

Assert:

```text
state contains all accepted characters
draw count << 100
```

Target:

```text
1–3 draws
```

depending on deterministic harness.

Do not write timing-flaky tests if avoidable; inject a fake frame clock.

---

# 40. Tests — Bulk Paste

Inject:

```text
Event::Paste(large_string)
```

Assert:

```text
one logical state mutation
correct final content
bounded draw count
```

---

# 41. Tests — Theme Application

Prefer view-model/span tests.

Verify:

```text
success state → success role
failure → error
waiting → warning/muted
selected → selection
diff + → added
diff - → removed
```

Do not screenshot-test raw terminal ANSI unless necessary.

---

# 42. Tests — Legacy UI Removal

Add simple rendering/view assertions that normal primary content no longer includes:

```text
N: new task
T: tasks
P: projects
A: add project
[T] Tasks
[D] Dashboard
[N] New Task
```

These may remain in hidden compatibility documentation only if deliberately retained.

---

# 43. Manual Acceptance Matrix

## Input

```text
[ ] one physical key → one character
[ ] key release never duplicates
[ ] held printable key does not flood
[ ] held backspace works
[ ] held arrows work
[ ] Enter executes once
[ ] Esc acts once
```

## Responsiveness

```text
[ ] fast typing smooth
[ ] large paste smooth
[ ] runtime output updates promptly
[ ] cancellation responsive
[ ] no per-character visible redraw crawl
[ ] idle CPU reasonable
```

## Theme

```text
[ ] Header styled
[ ] Home semantic state colors
[ ] Tasks semantic state colors
[ ] Runtime health colors
[ ] capability colors
[ ] Doctor colors
[ ] Execution colors
[ ] Result colors
[ ] Diff +/- colors
[ ] warnings/errors distinct
```

## Layout

```text
[ ] no giant full-screen border on Home
[ ] no giant full-screen border on Execution
[ ] no giant full-screen border on Result
[ ] command footer persistent
[ ] no old shortcut wall
```

---

# 44. Hard Release Gate

**DO NOT CALL M8.1 UX COMPLETE IF ANY ARE TRUE:**

```text
[ ] typing can duplicate characters
[ ] Release key events mutate UI state
[ ] Enter can submit twice
[ ] the UI redraws once per rapid key event
[ ] fallback key-event paste visibly crawls
[ ] Theme exists but most screens are plain unstyled Strings
[ ] Diff semantic colors are defined but unused
[ ] Home still has a giant border
[ ] Execution still has a giant border
[ ] Result still has a giant border
[ ] Result still promotes [T]/[D]/[N] as primary navigation
[ ] runtime states are unstyled ASCII only
[ ] old keybind vocabulary remains the main UX
[ ] Antigravity can take terminal control
```

---

# 45. Recommended Commit Sequence

Keep this remediation small and reviewable.

```text
1. fix[Jacinth]: filter terminal key kinds and stop duplicate input

2. perf[Jacinth]: coalesce terminal input and frame redraws

3. feat[Jacinth]: apply semantic theme across Relay views

4. refactor[Jacinth]: remove remaining legacy bordered/keybind UI
```

If changes are tightly coupled, commits 3–4 may be combined.

Do not rewrite M6/M7 scheduler semantics.

---

# 46. Quality Gates

Run:

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace

cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

Then perform manual Windows TUI acceptance.

Automated Cargo tests alone are **not sufficient** to prove keyboard correctness in the real terminal backend.

---

# 47. Required DEV Completion Report

Return:

```text
branch
base SHA
successor SHA
commit list

INPUT FIX
KeyEventKind policy:
...
Repeat policy:
...
Text-entry behavior:
...
Navigation repeat:
...

RESPONSIVENESS
Event-loop architecture:
...
Frame-coalescing:
...
Rapid-key batching:
...
Bulk Paste:
...
Idle behavior:
...

THEME
Theme source of truth:
...
Screens styled:
...
Diff styling:
...

LEGACY UI REMOVAL
Borders removed:
...
Old shortcut text removed:
...

MANUAL WINDOWS ACCEPTANCE
single typing:
PASS

repeated-letter words:
PASS

hold printable key:
PASS

hold Backspace:
PASS

hold arrows:
PASS

Enter once:
PASS

Esc once:
PASS

large paste:
PASS

Antigravity terminal isolation:
PASS

Cargo gates:
PASS

M6 scheduler contract changes:
NONE

M7 lifecycle changes:
NONE
```

---

# 48. Final UX Contract

Relay input must feel like a normal high-quality developer CLI:

```text
typed once
→ appears once

typed quickly
→ not lost

pasted
→ appears promptly

held navigation
→ repeats naturally

provider output arrives
→ UI remains responsive
```

The rendering contract is:

> **State changes immediately; frames are coalesced.**

The keyboard contract is:

> **Physical key semantics are normalized before Relay mutates state.**

The visual contract is:

> **Semantic theme roles are actually rendered throughout the product, not merely defined in a theme module.**

And the interaction contract remains:

> **Commands are primary. Keybinds are accelerators. Relay owns the terminal.**
