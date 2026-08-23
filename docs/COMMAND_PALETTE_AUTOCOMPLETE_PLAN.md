# Relay — Search-Style Command Palette & Contextual Autocomplete Implementation Plan

**Status:** DEV implementation plan  
**Date:** 2026-08-15  
**Target:** `apps/relay-tui`  
**Current source baseline reviewed:** `459a438839c64d8863045270cab9e337cebb8068`  
**Primary UX authority:** `relay-knowledge/10-DEVELOPMENT/reference/TERMINAL-UX-GUIDELINES.md`

---

# 1. Goal

Relay's command prompt should behave like a modern search box / command palette.

The user should not need to memorize or fully retype commands.

Target interaction:

```text
> /
```

immediately shows:

```text
  /new                  Create or import a task
  /tasks                Browse tasks
  /projects             Browse trusted projects
  /project add          Add a local repository
  /runtimes             Browse coding runtimes
  /settings             Open Relay settings
```

Typing:

```text
> /ru
```

filters live:

```text
> /ru

  /run <task>           Run a READY task
  /runtime <provider>   Open runtime details
  /runtime use          Select default runtime/profile
  /runtime test         Recheck runtime prerequisites
  /runtimes             Browse runtimes
```

Typing:

```text
> /runtime use 
```

changes from **command completion** to **argument completion**:

```text
> /runtime use 

  codex/implement       Codex · Write · High
  codex/review          Codex · Read only · Medium
  codex/deep            Codex · Write · XHigh
  antigravity/implement Antigravity · Write · High
  antigravity/review    Antigravity · Review
```

Typing:

```text
> /run 
```

shows only runnable tasks:

```text
> /run 

  81a7f2  relay        Runtime router correction
  2c019e  practicore   Fix placement validation
```

The command prompt should feel like:

```text
Google search suggestions
+
IDE command palette
+
Relay's trusted orchestration state
```

not like:

```text
a line of static help text
```

---

# 2. Why the Current Implementation Looks Wrong

Current `command.rs` contains:

```rust
pub const COMMANDS: &[&str] = &[ ... ];

pub fn command_suggestions(input: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .filter(|command| command.starts_with(input.trim()))
        .collect()
}
```

This has several limitations:

```text
1. Suggestions are strings only.
2. There is no description.
3. There is no selected suggestion.
4. There is no completion text separate from display syntax.
5. There is no command category.
6. There is no dynamic argument provider.
7. There is no disabled/unavailable reason.
8. There is no ranking beyond starts_with().
```

Current rendering then effectively does:

```rust
command_suggestions(&state.command_input).join("  ")
```

Therefore:

```text
input "/"
    ↓
every COMMANDS entry starts with "/"
    ↓
all commands become one giant footer string
    ↓
terminal wraps that string across the screen
```

That is the behavior visible in the current dogfood screenshot.

This is not the intended command-palette UX.

---

# 3. Current Keyboard Limitation

Current command-mode handling supports roughly:

```text
Enter
Backspace
Esc
printable characters
```

It does not yet provide command-palette navigation semantics for:

```text
Up
Down
Tab
BackTab
Right
```

The command palette therefore cannot currently:

```text
select a suggestion
autocomplete a suggestion
scroll a suggestion list
accept a dynamic task/profile candidate
```

This plan adds those behaviors while preserving the existing KeyEventKind correctness work.

---

# 4. Product Interaction Contract

Freeze these interaction rules.

## Primary

```text
typing /      → open command palette
typing text   → filter immediately
Up / Down     → move selected suggestion
Tab           → autocomplete selected suggestion
Enter         → execute selected complete action OR advance completion
Esc           → close palette / clear command mode
```

## Secondary

```text
PageUp / PageDown
    optional for long dynamic lists

Right
    optional alias for accepting completion
```

## Never

The command palette must never become a shell.

These remain non-executable:

```text
git status
cargo test
rm -rf .
powershell ...
cmd /C ...
```

Only registered `RelayCommand` actions may execute.

---

# 5. Do Not Implement This as a Giant Footer String

Remove this presentation model:

```text
prompt + every suggestion joined on one line
```

Instead render two independent surfaces:

```text
main content

suggestion overlay/list
────────────────────────────────────
> command prompt
```

The prompt remains fixed.

The suggestion list expands upward above it.

---

# 6. Recommended Visual Design

## `/`

```text
  New task
  /new
  Create or import a Relay handoff

  Tasks
  /tasks
  Browse durable tasks

  Projects
  /projects
  Browse trusted repositories

  Runtimes
  /runtimes
  Manage coding runtimes

──────────────────────────────────────────────
> /
```

A more compact default is acceptable:

```text
  /new                  Create/import task
  /tasks                Browse tasks
  /projects             Browse projects
  /project add          Add repository
  /runtimes             Browse runtimes
  /settings             Relay settings

──────────────────────────────────────────────
> /
```

Use the compact variant initially.

---

# 7. Selected Suggestion

One item should always be selected when suggestions exist.

Example:

```text
  /run <task>           Run a READY task
> /runtime <provider>   Open runtime details
  /runtime use          Switch runtime profile
  /runtime test         Recheck runtime
  /runtimes             Browse runtimes

──────────────────────────────────────────────
> /ru
```

Use:

```text
selection semantic style
+
> marker
```

Do not rely on color alone.

---

# 8. Semantic Styling

Use the existing Relay semantic Theme.

Suggested styling:

```text
selected row         theme.selection()
command token        theme.accent()
description          theme.muted()
disabled item        theme.muted()
unavailable reason   theme.warning()
error                theme.error()
```

The prompt:

```text
>
```

uses accent when active.

Typed command remains normal text.

---

# 9. Adaptive Height

Do not reserve the entire terminal for suggestions.

Recommended:

```text
MAX_VISIBLE_SUGGESTIONS = 6
```

or:

```text
min(matches, 7)
```

On very small terminals:

```text
show 3
```

On larger terminals:

```text
show 6–8
```

The main content should remain visible.

---

# 10. New Command Catalog Model

Replace `&[&str]` as the primary catalog with structured command metadata.

Conceptual model:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    Home,
    New,
    Tasks,
    Projects,
    ProjectAdd,
    Runtimes,
    RuntimeDetail,
    RuntimeUse,
    RuntimeTest,
    Settings,
    Doctor,
    Help,
    Run,
    Cancel,
    Diff,
    Logs,
    Quit,
}

pub struct CommandSpec {
    pub id: CommandId,
    pub syntax: &'static str,
    pub completion: &'static str,
    pub description: &'static str,
    pub category: CommandCategory,
    pub argument_kind: CommandArgumentKind,
}
```

Examples:

```rust
CommandSpec {
    id: CommandId::Tasks,
    syntax: "/tasks",
    completion: "/tasks",
    description: "Browse durable tasks",
    category: CommandCategory::Tasks,
    argument_kind: CommandArgumentKind::None,
}
```

```rust
CommandSpec {
    id: CommandId::RuntimeUse,
    syntax: "/runtime use <profile>",
    completion: "/runtime use ",
    description: "Switch the default runtime profile",
    category: CommandCategory::Runtime,
    argument_kind: CommandArgumentKind::RuntimeProfile,
}
```

Important distinction:

```text
syntax
    human-readable help

completion
    text inserted into the prompt
```

Do not insert literal:

```text
<profile>
<provider>
<task>
[path]
```

into the user's input.

---

# 11. Command Categories

Suggested:

```rust
enum CommandCategory {
    Navigation,
    Tasks,
    Projects,
    Runtime,
    System,
}
```

This may later support grouped `/help`.

The initial palette does not need section headers.

Keep the first UI compact.

---

# 12. Argument Kinds

Create a provider-neutral completion type.

```rust
enum CommandArgumentKind {
    None,
    Provider,
    RuntimeProfile,
    ReadyTask,
    CancellableTask,
    InspectableTask,
    ProjectPath,
}
```

Meaning:

```text
Provider
    codex
    antigravity

RuntimeProfile
    codex/implement
    codex/review
    ...

ReadyTask
    only tasks currently eligible for /run selection

CancellableTask
    queued/running tasks

InspectableTask
    durable task IDs usable by /diff and /logs

ProjectPath
    explicit path editing/completion context
```

Do not make command.rs directly depend on all of AppState.

---

# 13. Completion Context

Build a small read-only context from AppState.

Conceptual:

```rust
pub struct CompletionContext<'a> {
    pub providers: &'a [ProviderCompletion],
    pub profiles: &'a [ProfileCompletion],
    pub ready_tasks: &'a [TaskCompletion],
    pub cancellable_tasks: &'a [TaskCompletion],
    pub inspectable_tasks: &'a [TaskCompletion],
}
```

The command layer consumes this data but does not own Relay domain state.

---

# 14. Suggestion Model

Use a single model for static commands and dynamic arguments.

```rust
pub struct CommandSuggestion {
    pub display_primary: String,
    pub display_secondary: Option<String>,
    pub completion: String,
    pub description: String,
    pub kind: SuggestionKind,
    pub executable: bool,
    pub disabled_reason: Option<String>,
    pub score: u16,
}
```

Kinds:

```rust
enum SuggestionKind {
    Command,
    Provider,
    RuntimeProfile,
    Task,
    Path,
}
```

---

# 15. Palette State

Replace ad-hoc suggestion rendering with explicit state.

Recommended:

```rust
pub struct CommandPaletteState {
    pub input: String,
    pub selected_index: usize,
    pub scroll_offset: usize,
}
```

Current:

```text
command_mode
command_input
```

may remain temporarily, but the preferred migration is:

```rust
command_palette: Option<CommandPaletteState>
```

This makes palette ownership explicit.

---

# 16. Selection Reset Rules

When typed text changes:

```text
selected_index = 0
scroll_offset = 0
```

unless the previously selected suggestion still exists and preserving it is trivial.

Keep first implementation deterministic.

When no suggestions exist:

```text
selected_index = 0
```

and show:

```text
No Relay commands match "/xyz"
```

Do not display the entire `/help` list.

---

# 17. Search / Ranking

Relay currently has fewer than ~20 commands.

Do not introduce a heavy fuzzy-search dependency yet.

Implement a small deterministic score.

Recommended ranking:

```text
1. exact command match
2. completion starts with query
3. token starts with query
4. substring match
```

Example:

```text
query: /run

/run <task>             highest
/runtime <provider>     lower
/runtimes               lower
```

For `/pro`:

```text
/projects
/project add
```

rank above unrelated matches.

Case-insensitive matching is fine.

---

# 18. Query Tokenization

Autocomplete should understand command structure.

Examples:

```text
"/"
    command-name completion

"/runtime"
    command-name / runtime subcommand completion

"/runtime "
    provider or runtime-subcommand completion

"/runtime u"
    "use" completion

"/runtime use "
    profile completion

"/run "
    READY task completion
```

Do not treat the whole string as one raw prefix forever.

---

# 19. Command Tree

A lightweight command tree is preferable to increasingly complex string checks.

Conceptual:

```text
/
├── home
├── new
├── tasks
├── projects
├── project
│   └── add [path]
├── runtimes
├── runtime
│   ├── <provider>
│   ├── use <profile>
│   └── test <provider>
├── settings
├── doctor
├── help
├── run <task>
├── cancel <task>
├── diff [task]
├── logs [task]
└── quit
```

Do not build a general parser framework.

A small Relay-specific tree/catalog is enough.

---

# 20. Reconsider `/runtime profile`

Current source exposes:

```text
/runtime profile <profile>
```

If it does not have a unique product semantic distinct from:

```text
/runtime use <profile>
/runtime <provider>
```

remove or hide it from the primary palette.

Do not present redundant commands just because the parser currently has them.

If retained, document exactly what it does before exposing it.

---

# 21. `/runtime` Contextual Suggestions

Typing:

```text
/runtime 
```

should produce:

```text
  codex                 Open Codex runtime
  antigravity           Open Antigravity runtime
  use                   Change default runtime profile
  test                  Recheck runtime prerequisites
```

Then:

```text
/runtime c
```

shows:

```text
> codex
```

Tab:

```text
/runtime codex
```

Enter:

```text
opens Runtime Detail
```

---

# 22. `/runtime use` Contextual Suggestions

Typing:

```text
/runtime use 
```

shows actual resolved profiles.

Example:

```text
> codex/implement
  codex/review
  codex/deep
  antigravity/implement
  antigravity/review
  antigravity/deep
```

Display useful metadata:

```text
codex/implement        Write · High
codex/review           Read only · Medium
antigravity/implement  Write · High
```

Unavailable runtime/profile:

```text
antigravity/implement  × Runtime not authenticated
```

Either:

```text
omit unavailable profiles
```

or preferably show them disabled with a reason so the user can discover the feature.

Disabled Enter:

```text
does not mutate state
```

and shows the reason.

---

# 23. `/run` Contextual Task Suggestions

Typing:

```text
/run 
```

should not ask the user to remember UUIDs.

Show:

```text
> 81a7f2  relay        Runtime routing correction
  29f10c  practicore   Placement validation
```

Completion:

```text
/run 81a7f2
```

Task suggestion must contain:

```text
short ID
project
title
state/runtime eligibility
```

Use only tasks that satisfy the same **selection** predicate required before Run.

Actual Core/TUI preflight remains authoritative on Enter.

Autocomplete does not bypass execution approval.

---

# 24. `/cancel` Task Suggestions

Typing:

```text
/cancel 
```

shows:

```text
running
queued
waiting
```

tasks that can meaningfully receive a cancellation request.

Do not show DONE tasks.

Do not display CANCELLED before Core confirms it.

---

# 25. `/diff` and `/logs` Suggestions

Typing:

```text
/diff 
/logs 
```

shows inspectable tasks.

These suggestions are read-only navigation.

They must never submit a task.

Maintain the M8.1 read-only command correction:

```text
/diff != /run
/logs != /run
```

---

# 26. `/project add`

Typing:

```text
/project 
```

shows:

```text
> add
```

After:

```text
/project add 
```

initial suggestion:

```text
.                      Current directory
```

Optional later enhancements:

```text
recent Relay project paths
parent directory
```

Do not recursively scan the filesystem.

Do not execute shell completion commands.

Normal `/project add` with no path should still use discovery-first current-directory behavior.

---

# 27. Enter Semantics

Enter should be context-aware.

## Complete executable suggestion

Example:

```text
input: /ta
selected: /tasks
```

Enter may execute `/tasks` immediately.

This gives the search-box experience:

```text
type part
↓
select
↓
Enter
↓
done
```

No need to first Tab and then Enter.

## Incomplete command needing argument

Example:

```text
input: /runtime u
selected: /runtime use
```

Enter should:

```text
fill "/runtime use "
keep palette open
show runtime profiles
```

It should not execute an invalid command.

## Dynamic complete argument

Example:

```text
input: /runtime use ant
selected: antigravity/implement
```

Enter may construct and execute:

```text
/runtime use antigravity/implement
```

after normal parser/validation.

---

# 28. Tab Semantics

Tab always means:

```text
accept completion
do not execute
```

Examples:

```text
/ta + Tab
→ /tasks

/runtime u + Tab
→ /runtime use 

/runtime use ant + Tab
→ /runtime use antigravity/implement
```

Tab is useful when the user wants to edit before execution.

---

# 29. Up / Down

When palette is visible:

```text
Up      previous suggestion
Down    next suggestion
```

Use controlled Repeat behavior already defined in `events.rs`.

Holding Down should scroll naturally.

Do not append key characters while navigating.

---

# 30. Esc

When command palette active:

```text
Esc
→ close command mode
→ clear palette
```

Do not exit Relay.

A second Esc in normal application mode may follow existing screen navigation policy.

---

# 31. Mouse

Mouse support is not required.

Do not expand scope.

Keyboard-first is correct for this product.

---

# 32. Rendering Architecture

Current footer is fixed:

```text
Constraint::Length(2)
```

Do not expand it to 8 permanent lines.

Keep prompt/footer fixed at ~2 lines.

Render the suggestion list as a conditional overlay above it.

Conceptual:

```rust
if let Some(palette) = state.command_palette.as_ref() {
    let popup = command_palette_rect(frame.area(), visible_count);
    frame.render_widget(Clear, popup);
    render_command_palette(frame, palette, popup);
}
```

If avoiding boxed popup style:

```text
render a background-cleared list without Borders::ALL
```

matching the Relay UX guideline.

---

# 33. Palette Placement

Anchor to bottom, directly above prompt.

Example 40-row terminal:

```text
rows 1–32    current screen
rows 33–38   suggestions
row 39       separator
row 40       prompt
```

Do not place suggestions at the top of the terminal away from the input.

The user should visually associate them with typing.

---

# 34. Narrow Terminal Behavior

When width is narrow:

Priority:

```text
completion text
selection marker
```

Then:

```text
truncate description
truncate project/title metadata
```

Never truncate the input prompt itself unless terminal width is extremely small.

Example:

```text
> /run
```

should always remain visible.

---

# 35. Wide Terminal Behavior

Use two columns:

```text
command / candidate          description
```

Example:

```text
> /runtime use               Switch default runtime
  /runtime test              Recheck prerequisites
  /runtime codex             Open Codex runtime
```

Do not stretch across the full width unnecessarily.

Cap palette width if needed:

```text
80–100 columns
```

---

# 36. Highlight Matched Text

Optional but recommended.

For query:

```text
/ru
```

render matched prefix with accent:

```text
/ru n <task>
/ru ntime
/ru ntimes
```

Do not spend excessive complexity on arbitrary fuzzy-highlight ranges in first implementation.

Prefix/token highlighting is sufficient.

---

# 37. Dynamic Completion Must Use Trusted Local State

Argument suggestions come from Relay state.

Examples:

```text
runtime profiles
    RuntimeManager / RuntimeIntegration

tasks
    durable task + scheduler state

providers
    RuntimeRouter registry / probe state

projects
    Project Registry
```

Never source command completions from provider stdout or HandoffTask extensions.

---

# 38. Avoid Circular Dependencies

`command.rs` should remain presentation/parser-focused.

Do not import:

```text
ExecutionScheduler
SqliteStore
CodexRuntime
AntigravityRuntime
```

directly into the command parser.

Instead:

```text
AppState
  builds CompletionContext

command module
  transforms CompletionContext + input
  into CommandSuggestion[]
```

---

# 39. Recommended Modules

Current:

```text
apps/relay-tui/src/command.rs
```

can be split safely:

```text
command/
  mod.rs
  parser.rs
  catalog.rs
  completion.rs
```

or remain:

```text
command.rs
command_completion.rs
```

Recommended minimal change:

```text
command.rs
    RelayCommand
    parser
    CommandSpec catalog

command_completion.rs
    CompletionContext
    CommandSuggestion
    scoring
    contextual argument suggestions
```

Do not over-fragment the TUI.

---

# 40. AppState Changes

Add:

```rust
command_selected_index: usize
command_scroll_offset: usize
```

or preferably:

```rust
command_palette: Option<CommandPaletteState>
```

If current migration risk is high, preserve:

```text
command_mode
command_input
```

and add selection fields first.

The long-term model should converge to one `CommandPromptState`.

---

# 41. Suggested CommandPromptState

```rust
pub struct CommandPromptState {
    pub input: String,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub open: bool,
}
```

Methods:

```rust
fn open()
fn close()
fn clear()
fn push_char()
fn backspace()
fn move_next()
fn move_previous()
fn reset_selection()
```

Keep rendering state out of Core.

---

# 42. Suggestion Recalculation

Do not store full suggestions permanently in AppState unless profiling proves necessary.

With fewer than a few hundred candidates, compute:

```text
render / key event
```

from:

```text
input + CompletionContext
```

or cache only during command mode.

Dynamic task lists remain small enough for local alpha.

---

# 43. Performance

The palette must remain compatible with the new frame-coalesced event loop.

Typing:

```text
/
→ suggestions update state
→ dirty = true
→ next frame renders list
```

Do not:

```text
block on runtime health probes
query SQLite asynchronously per character
spawn Git commands per character
```

Build completion context from already-loaded/cached state.

---

# 44. Runtime Health Suggestions

Do not probe providers because the user typed:

```text
/runtime
```

Use cached runtime health.

If stale:

```text
Antigravity           ? not checked
```

The user can run:

```text
/runtime test antigravity
```

to refresh.

Autocomplete must feel instant.

---

# 45. Task Completion Data

Build from current in-memory:

```text
durable_tasks
scheduler_snapshot
```

No SQLite call on every keypress.

Suggested task completion struct:

```rust
pub struct TaskCompletion {
    pub id: TaskId,
    pub short_id: String,
    pub project: String,
    pub title: String,
    pub state: TaskState,
    pub scheduler_status: Option<SchedulerTaskStatus>,
}
```

---

# 46. Matching Task Selectors

When the user types:

```text
/run rel
```

optional enhancement:

match not only TaskId prefix, but also:

```text
project
title
```

Display:

```text
81a7f2  relay  Runtime router correction
```

Completion should still insert the **TaskId**, not arbitrary title text.

This makes Relay much more usable.

---

# 47. Ranking Tasks

Suggested task ranking:

```text
TaskId prefix             highest
project prefix
title word prefix
title substring
```

Then:

```text
recent / current state
```

may break ties.

Do not hide a direct ID prefix match behind fuzzy title matches.

---

# 48. Command History — Optional Later

Do not add full shell history in this implementation unless trivial.

Possible future:

```text
Up / Down with no suggestions
→ previous Relay commands
```

But currently Up / Down is more valuable for suggestion selection.

Keep scope focused.

---

# 49. Help Becomes Secondary

`/help` remains useful for full command documentation.

But normal discovery becomes:

```text
type /
```

Therefore Home no longer needs to print a giant command list.

Good empty-state hint:

```text
Type / for commands
```

That is enough.

---

# 50. Remove Giant Help Dump From Normal Workflow

The screenshot currently shows the complete command list occupying the main content after help/error state.

Keep `/help` as a dedicated Help view, but command discovery should no longer require opening it.

When palette is active:

```text
show only matching suggestions
```

not the entire registry in main content.

---

# 51. Error Recovery

If command execution returns:

```text
Unknown Relay command
```

keep the prompt usable.

Do not switch to a large failed Help screen.

Example:

```text
Unknown command "/runtiem".
Did you mean:

  /runtime
  /runtimes

────────────────────────────────────────
> /runtiem
```

Optional correction assistance is useful.

---

# 52. "Did You Mean" Matching

Reuse suggestion scoring after parse error.

If:

```text
/runtiem
```

fails, show top 1–3 matches.

Do not auto-execute the correction.

---

# 53. Disabled Suggestion Behavior

Example:

```text
/runtime use antigravity/implement
```

when Antigravity is not authenticated.

Display:

```text
  antigravity/implement  × Not authenticated
```

Selection may open runtime detail or show feedback, but must not silently activate an unusable profile.

Validation at actual command execution remains authoritative.

---

# 54. `/run` Disabled Reasons

A READY-looking task may not actually be runnable because of:

```text
repository ineligible
runtime unavailable
unsupported extension
scheduler rebind
runtime/profile mismatch
```

Autocomplete should avoid promising more than it knows.

Options:

```text
A. list only tasks passing current cheap UI eligibility
B. list READY tasks but display disabled reason
```

Preferred:

```text
show disabled candidate + reason
```

where cached information is available.

The final Run path still performs full Core/TUI preflight.

---

# 55. Preserve Explicit Run

Selecting a task suggestion and pressing Enter on:

```text
/run 81a7
```

is the explicit local Run action.

It must use the same accepted:

```text
task selection
repository preflight
runtime/profile resolution
scheduler submission
```

path.

Autocomplete itself does not queue anything.

---

# 56. Tests — Static Catalog

Test every catalog entry:

```text
unique CommandId
unique completion prefix where appropriate
non-empty description
valid parser relationship
```

Ensure exposed commands actually parse.

---

# 57. Tests — Root Slash

```text
suggest("/")
```

returns a bounded ranked command set.

Do not assert every command appears onscreen simultaneously.

The full catalog may be scrollable.

---

# 58. Tests — Prefix Filtering

Required:

```text
/pro
→ /projects
→ /project add

/ru
→ /run
→ /runtime ...
→ /runtimes

/sett
→ /settings
```

---

# 59. Tests — Command Completion

```text
input /ta
select /tasks
Tab
→ /tasks

input /runtime u
select /runtime use
Tab
→ /runtime use 
```

---

# 60. Tests — Enter Behavior

```text
/ta + selected /tasks + Enter
→ RelayCommand::Tasks

/runtime u + selected /runtime use + Enter
→ prompt becomes "/runtime use "
→ no command executed
```

---

# 61. Tests — Runtime Profiles

Completion context:

```text
codex/implement
codex/review
antigravity/implement
```

Input:

```text
/runtime use ant
```

Expected:

```text
antigravity/implement
```

Completion inserts profile ID.

---

# 62. Tests — Providers

Input:

```text
/runtime c
```

Expected:

```text
codex
```

Enter executes:

```text
RelayCommand::RuntimeDetail { provider: "codex" }
```

---

# 63. Tests — Task Suggestions

Given:

```text
READY 81a7 relay "Runtime routing"
DONE  91ff relay "Old task"
```

`/run `:

```text
contains 81a7
does not offer DONE as runnable
```

`/diff `:

```text
may contain both if both are inspectable
```

---

# 64. Tests — No Side Effects

Generating suggestions must not:

```text
run task
cancel task
mutate registry
probe provider
run Git
write config
write SQLite
```

Autocomplete is pure/read-only.

---

# 65. Tests — Keyboard Navigation

Test:

```text
Down → selected_index + 1
Up → selected_index - 1
wrap or clamp consistently
Tab → completion
Esc → close
```

Choose either wrap or clamp.

Recommended:

```text
wrap
```

for a small palette.

---

# 66. Tests — Resize / Visible Range

With:

```text
10 suggestions
max visible = 6
```

selection movement updates:

```text
scroll_offset
```

so selected item remains visible.

---

# 67. Tests — Small Terminal

Render/view-model logic should gracefully reduce:

```text
description hidden/truncated
candidate primary text preserved
prompt preserved
```

No panic from tiny Rects.

---

# 68. Tests — KeyEventKind Regression

Retain M8.1 input rules.

A physical key event stream:

```text
Press('/')
Release('/')
```

must open one command palette and insert one slash.

Repeat printable:

```text
ignored in CommandPrompt
```

Navigation Repeat:

```text
allowed
```

---

# 69. Tests — Paste Into Command Prompt

Pasting:

```text
/runtime use antigravity/implement
```

into command mode should:

```text
update input once
refresh matching palette
not auto-execute
```

The user still presses Enter.

---

# 70. Rendering Test Strategy

Do not snapshot entire terminal ANSI output unless necessary.

Prefer:

```text
CommandPaletteViewModel
```

tests:

```rust
struct PaletteRow {
    primary,
    secondary,
    selected,
    disabled,
    semantic_role,
}
```

Then Ratatui renderer becomes thin.

---

# 71. View Model

Recommended:

```rust
pub struct CommandPaletteViewModel {
    pub rows: Vec<CommandPaletteRow>,
    pub total_matches: usize,
    pub selected_visible_index: Option<usize>,
}

pub struct CommandPaletteRow {
    pub primary: String,
    pub description: String,
    pub metadata: Option<String>,
    pub selected: bool,
    pub disabled_reason: Option<String>,
}
```

Keeps completion/search separate from rendering.

---

# 72. Accessibility / Monochrome

Selected row must include:

```text
>
```

not only background color.

Disabled must include:

```text
×
!
```

or clear text reason.

Everything remains understandable without ANSI color.

---

# 73. Manual Acceptance

Run Relay in Windows Terminal.

## Root discovery

Type:

```text
/
```

Expected immediately:

```text
a vertical suggestion list
one row selected
no huge horizontal wrapped command string
```

## Filter

Type:

```text
ru
```

Expected:

```text
list narrows live
```

## Selection

Press:

```text
Down
Down
```

Expected:

```text
selection moves
input text does not change
```

## Completion

Press:

```text
Tab
```

Expected:

```text
selected command fills the prompt
```

## Execute

Select `/tasks`, Enter.

Expected:

```text
Tasks opens
```

No second Enter required.

---

# 74. Manual Runtime Completion

Type:

```text
/runtime use 
```

Expected:

```text
actual runtime profiles appear
```

Select:

```text
antigravity/implement
```

Press Enter.

Expected:

```text
normal validated RuntimeUse command path runs
```

Do not merely mutate display text if broader M8.1 runtime-routing correction has not landed yet.

---

# 75. Manual Task Completion

With READY tasks:

```text
/run 
```

Expected:

```text
short ID + project + title candidates
```

User should not need to copy a UUID from `/tasks`.

---

# 76. Manual Read-Only Completion

Type:

```text
/diff 
```

Select READY task.

Enter.

Expected:

```text
Diff view opens
task does NOT execute
```

Likewise `/logs`.

---

# 77. Manual Typo Assistance

Type:

```text
/runtiem
```

Expected:

```text
no execution
helpful nearest command suggestions
```

---

# 78. UX Completion Gate

Do **not** call command completion implemented if:

```text
[ ] typing "/" prints all commands as one wrapped line
[ ] user cannot move selection with Up/Down
[ ] Tab cannot autocomplete
[ ] Enter always requires the user to fully type the command first
[ ] /runtime use cannot suggest profiles
[ ] /runtime cannot suggest providers
[ ] /run still requires memorizing/copying a full TaskId
[ ] task autocomplete performs I/O or executes work
[ ] palette blocks the main UI with a giant panel
[ ] active command text is styled as muted/disabled
[ ] small terminal sizes hide the prompt
[ ] Release/Repeat events reintroduce duplicate typing
```

---

# 79. Suggested File Changes

Minimum likely files:

```text
apps/relay-tui/src/command.rs
apps/relay-tui/src/command_completion.rs    new
apps/relay-tui/src/main.rs
apps/relay-tui/src/events.rs
apps/relay-tui/src/theme.rs
```

If the TUI refactor from the broader M8.1 plan has already landed, place rendering in:

```text
apps/relay-tui/src/ui/shell.rs
apps/relay-tui/src/view_model/command_palette.rs
```

instead of adding more code to `main.rs`.

---

# 80. Suggested Commit

One cohesive commit is sufficient:

```text
feat[Jacinth]: add contextual Relay command palette
```

If dynamic task/runtime completion becomes large:

```text
1. feat[Jacinth]: add selectable command palette
2. feat[Jacinth]: add contextual runtime and task completion
```

---

# 81. Quality Gates

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

Then manually verify in the real Windows terminal.

No live Codex/Antigravity execution is necessary solely for command-palette testing.

---

# 82. Final Product Contract

The expected interaction is:

```text
User:
> /

Relay:
  /new
  /tasks
  /projects
  /runtimes
  ...

User:
> /ru

Relay:
  /run <task>
  /runtime <provider>
  /runtime use
  /runtime test
  /runtimes

User:
↓
Tab

Relay:
> /runtime use 

  codex/implement
  codex/review
  antigravity/implement
  ...
```

The user should be able to operate Relay through **recognition instead of memorization**.

The permanent UX rule is:

> **Typing `/` opens Relay's command discovery surface. Every additional character narrows the choices. Arrow keys select, Tab completes, and Enter acts.**

That is the command-first experience the Relay terminal guidelines intended.
