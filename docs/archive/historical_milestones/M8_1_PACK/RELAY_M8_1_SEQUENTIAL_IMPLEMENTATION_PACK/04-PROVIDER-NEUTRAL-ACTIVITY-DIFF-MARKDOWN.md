# Relay — Provider-Neutral Activity Abstraction, Structured Diff & Rich Terminal Rendering Plan

**Status:** DEV implementation plan  
**Date:** 2026-08-15  
**Scope:** Provider-neutral execution abstraction, normalized commands, compact cwd display, structured local Git diff presentation, Markdown/rich-text rendering, formatted vs raw logs, and provider-compliance guardrails.

---

# 1. Product Goal

Relay should present coding-agent execution like a polished orchestration CLI:

```text
● Read apps/relay-tui/src/main.rs
  Read 21 lines

● Edit apps/relay-tui/src/main.rs
  +6 -1

  2035   active_profile_id: ...
+ 2036   active_model: ProviderDefault
+ 2037   active_reasoning_effort: ProfileDefault
+ 2038   runtime_settings: ...

● Run
  cargo test --workspace
  cwd  .

Provider message

  Core flow

  Human + Planner
    → explicit approval
    → Relay Ingress
    → Relay Core
    → Coding Runtime
```

The user should not normally see:

```text
C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe
cmd.exe /D /C
/bin/bash -lc
raw JSONL
literal ```markdown fences
full absolute user-home paths
```

Those belong in explicit diagnostics.

---

# 2. Provider Compliance Rule

Relay may normalize only data that the provider intentionally exposes through a documented/programmatic interface.

Allowed sources:

```text
documented CLI JSON / JSONL events
documented SDK/API events
documented tool-call metadata
provider-visible assistant messages
provider-visible command/output metadata
local trusted Git state
Relay's own scheduler/runtime state
```

Do not use:

```text
provider TUI scraping
screen scraping
PTY scraping of interactive UI
undocumented internal session files
credential files
hidden model state
hidden chain-of-thought
reverse-engineered proprietary protocol fields
rate-limit or safety bypasses
```

Provider adapters must have a `machine_interface_verified` compatibility check.

If a provider has no documented machine interface, Relay should mark that adapter:

```text
Unsupported for automated Relay execution
```

rather than automate its interactive UI.

---

# 3. OpenAI / Codex Boundary

Relay should integrate Codex through:

```text
codex exec --json
```

or another current documented machine interface.

Do not automate the consumer ChatGPT web/TUI experience as a substitute for Codex.

The adapter should consume only the JSONL events exposed by `codex exec`.

Current Codex event concepts already include:

```text
thread started
turn started/completed
agent message
command execution
file change
reasoning status
error
```

Relay should map those into its own provider-neutral activity model.

Important:

```text
Reasoning event
→ "Analyzing"
```

Do not display hidden reasoning text.

---

# 4. Antigravity Boundary

Relay should use Antigravity's documented non-interactive print mode:

```text
agy --print
--output-format stream-json
```

and consume documented typed events such as:

```text
init
step_update
result
```

including documented `tool_info` payloads where present.

Do not embed the Antigravity TUI inside Relay.

---

# 5. Future Provider Adapter Contract

Every new provider must declare:

```rust
pub struct ProviderMachineContract {
    pub provider_id: String,
    pub machine_mode: MachineModeKind,
    pub structured_activity: bool,
    pub structured_final_result: bool,
    pub documented_interface: bool,
}
```

Relay only enables normal automated execution when:

```text
documented_interface == true
machine mode compatible
```

Provider-specific Terms/licensing remain an adapter acceptance requirement.

---

# 6. Separate Four Data Channels

Do not flatten everything into stdout.

Relay should carry four independent channels:

## Activity

```text
Read
Edit
Run command
Verify
Analyze
Warning
```

## Rich provider message

```text
Markdown or plain assistant-facing message
```

## Raw diagnostics

```text
stdout
stderr
provider diagnostics
raw wrapper command
```

## Repository truth

```text
local Git status/diff
```

These must remain distinct all the way to the TUI.

---

# 7. Extend RuntimeActivity

Current foundation:

```rust
RuntimeActivity::CommandStarted {
    logical_command,
    raw_command,
}
```

Keep it.

Extend the model:

```rust
pub enum RuntimeActivity {
    SessionStarted,

    Analyzing {
        description: String,
    },

    FileRead {
        path: PathBuf,
        lines_read: Option<usize>,
    },

    CommandStarted {
        logical_command: String,
        raw_command: Option<String>,
        working_directory: Option<PathBuf>,
    },

    CommandProgress {
        logical_command: String,
        progress_line: String,
    },

    CommandCompleted {
        logical_command: String,
        success: bool,
        exit_code: Option<i32>,
        working_directory: Option<PathBuf>,
    },

    FileChanged {
        path: PathBuf,
        change_kind: FileChangeKind,
    },

    VerificationStarted {
        description: String,
    },

    VerificationProgress {
        description: String,
        detail: String,
    },

    VerificationCompleted {
        description: String,
        passed: bool,
    },

    AgentMessage {
        message: RuntimeText,
    },

    Warning {
        message: String,
    },

    ProviderDiagnostic {
        message: String,
    },
}
```

---

# 8. Add RuntimeText

```rust
pub struct RuntimeText {
    pub content: String,
    pub format: RuntimeTextFormat,
}
```

```rust
pub enum RuntimeTextFormat {
    Plain,
    Markdown,
}
```

Only provider-native agent/message events may opt into Markdown.

Command stdout/stderr remains Plain by default.

Never infer Markdown merely because a line starts with:

```text
#
*
-
```

---

# 9. RuntimeEvent

Target:

```rust
pub enum RuntimeEvent {
    Activity(RuntimeActivity),

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

Normal TUI uses:

```text
Activity
```

Raw Logs uses:

```text
Output
```

---

# 10. Core Forwarding

Add:

```rust
CoreExecutionEvent::RuntimeActivity {
    task_id: TaskId,
    execution_id: ExecutionId,
    activity: RuntimeActivity,
}
```

Core must not:

```text
render Markdown
strip PowerShell wrappers
parse Git diff
choose colors
```

Core only associates and forwards the typed event.

---

# 11. Codex Adapter Normalization

The current Codex adapter already sees structured:

```text
agent_message
command_execution
file_change
reasoning
```

Stop converting them into strings such as:

```text
› Running ...
✓ command
› Modified file.rs
```

Instead map to `RuntimeActivity`.

Example:

```text
item.started
command_execution
```

→

```rust
RuntimeActivity::CommandStarted { ... }
```

`item.completed command_execution` →

```rust
RuntimeActivity::CommandCompleted { ... }
```

`file_change` →

```rust
RuntimeActivity::FileChanged { ... }
```

`agent_message` →

```rust
RuntimeActivity::AgentMessage {
    format: Markdown
}
```

`reasoning` →

```rust
RuntimeActivity::Analyzing {
    description: "Codex is analyzing the task"
}
```

Do not expose reasoning contents.

---

# 12. Antigravity Adapter Normalization

Translate provider-native stream events.

Examples:

```text
step_update tool=read_file
→ FileRead

step_update tool=edit/apply_patch
→ FileChanged

step_update tool=terminal
→ CommandStarted / CommandCompleted

assistant/final message
→ AgentMessage(Markdown)

result
→ RuntimeOutcome
```

Do not depend on Antigravity's visual TUI strings.

---

# 13. Logical Command Extraction

Preferred order:

```text
1. provider native logical command field
2. provider structured argv/program
3. documented shell-script field
4. known exact wrapper abstraction
5. safe raw fallback
```

Never derive commands from model prose.

---

# 14. Known Wrapper Abstraction

Display-only normalization may recognize:

```text
powershell.exe -Command <script>
pwsh -Command <script>
cmd.exe /D /C <script>
bash -lc <script>
sh -lc <script>
```

Example:

```text
RAW
C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe
-Command "cargo test --workspace"

NORMALIZED
cargo test --workspace
```

The raw execution stays unchanged.

---

# 15. Do Not Build a Generic Shell Parser

Only unwrap known exact shapes.

If ambiguous:

```text
powershell.exe <command>
```

is acceptable.

The complete raw form remains available in diagnostics.

---

# 16. Working Directory Presentation

Extend command activity with:

```text
working_directory
```

Normal TUI displays project-relative location.

Given:

```text
trusted root:
C:\Users\PCC\Desktop\My Projects\Relay

cwd:
C:\Users\PCC\Desktop\My Projects\Relay\crates\relay-core
```

display:

```text
cwd  crates/relay-core
```

At root:

```text
cwd  .
```

Do not normally show:

```text
C:\Users\PCC\Desktop\My Projects\Relay
```

---

# 17. Path Safety

Provider-reported cwd/path must be validated against:

```text
RuntimeRequest.trusted_working_dir
```

If path is within root:

```text
show project-relative path
```

If outside:

```text
<outside trusted workspace>
```

and surface a warning.

Do not trust provider display metadata to redefine repository authority.

---

# 18. ExecutionViewState

Replace:

```rust
output: VecDeque<String>
```

with structured state.

Recommended:

```rust
pub struct ExecutionViewState {
    ...
    activities: VecDeque<RuntimeActivity>,
    raw_logs: ExecutionLogs,
    current_command: Option<CommandPresentation>,
    changed_files: Vec<FileChangePresentation>,
    verification: Vec<VerificationPresentation>,
}
```

Bounds remain mandatory.

---

# 19. ExecutionLogEntry

```rust
pub struct ExecutionLogEntry {
    pub source: ExecutionLogSource,
    pub stream: Option<RuntimeOutputStream>,
    pub content: String,
}
```

```rust
pub enum ExecutionLogSource {
    Provider,
    Command,
    Relay,
    Runtime,
}
```

Keep raw diagnostics separate from primary activity.

---

# 20. Normal Execution UX

Target:

```text
relay · Runtime model control

Codex / Implement
GPT-5.x · High · Write · Offline

● RUNNING                                  02:14

✓ Read apps/relay-tui/src/main.rs
  21 lines

● Edit apps/relay-tui/src/main.rs
  +6 -1

  2035   active_profile_id: ...
+ 2036   active_model: ProviderDefault
+ 2037   active_reasoning_effort: ProfileDefault

● Run
  cargo test --workspace
  cwd  .

Provider

  Core flow

  Human + Planner
    → explicit approval
    → Relay Ingress
    → Relay Core
```

---

# 21. Command Progress

Default inline command output should be concise.

Example:

```text
● cargo test --workspace
  compiling relay-core...
```

Completed:

```text
✓ cargo test --workspace
  163 tests passed
```

Failure:

```text
× cargo test --workspace
  exit 101
  error: ...
```

Full command output remains in Logs.

---

# 22. Logs Modes

Default:

```text
/logs <task>
```

renders formatted logs.

Add:

```text
/logs raw <task>
```

for diagnostics.

Formatted Logs:

```text
Command
  cargo test --workspace
  cwd .

Output
  running 163 tests
  ...
```

Raw Logs:

```text
[provider/stdout]
...

[provider/stderr]
...

[raw-command]
"C:\Windows\...\powershell.exe" -Command ...
```

---

# 23. Structured Diff — Source of Truth

The normal diff view must come from local trusted Git state.

Use direct Git invocation:

```text
git diff --no-ext-diff --no-color --unified=3
git diff --cached --no-ext-diff --no-color --unified=3
git status --porcelain=v1 -z
```

No shell.

Provider-generated patch text is not authoritative.

---

# 24. Diff Model

```rust
pub struct DiffDocument {
    pub files: Vec<DiffFile>,
}
```

```rust
pub struct DiffFile {
    pub old_path: Option<PathBuf>,
    pub new_path: Option<PathBuf>,
    pub status: DiffFileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<DiffHunk>,
}
```

```rust
pub struct DiffHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}
```

```rust
pub struct DiffLine {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub kind: DiffLineKind,
    pub text: String,
}
```

---

# 25. Full Diff UX

Target:

```text
Diff · relay

M apps/relay-tui/src/main.rs                 +6 -1

@@ 2033–2041
  2033 │ antigravity_health: ...
  2034 │ active_runtime_provider: ...
  2035 │ active_profile_id: ...
+ 2036 │ active_model: ProviderDefault
+ 2037 │ active_reasoning_effort: ProfileDefault
+ 2038 │ runtime_settings: ...
  2042 │ runtime_detail_provider: None,

M crates/relay-core/src/execution.rs          +7 -1
```

Do not show Git transport metadata by default:

```text
diff --git
index
--- a/
+++ b/
```

---

# 26. Inline Edit Preview

When `FileChanged` arrives:

```text
refresh local Git diff for that path
```

Then display a small preview:

```text
● Edit apps/relay-tui/src/main.rs
  +6 -1

  2035   ...
+ 2036   ...
+ 2037   ...
```

Bound:

```text
8–12 changed lines
```

Full patch:

```text
/diff
```

---

# 27. Diff Styling

Use semantic Theme:

```text
Added line      diff_added
Removed line    diff_removed
Context         text/muted
Line number     muted
Hunk heading    accent
Added count     success
Removed count   error
```

Keep the terminal background neutral.

---

# 28. Binary / Rename

Binary:

```text
M assets/logo.png
  Binary file changed
```

Rename:

```text
R old/path.rs → new/path.rs
```

No binary rendering.

---

# 29. Markdown Renderer

Add:

```text
apps/relay-tui/src/markdown.rs
```

Use a real CommonMark parser such as:

```text
pulldown-cmark
```

and convert parser events into Ratatui:

```text
Line
Span
```

Do not generate ANSI externally.

---

# 30. Markdown v0.1 Features

Support:

```text
headings
paragraphs
unordered lists
ordered lists
bold
italic
inline code
fenced code blocks
blockquotes
links
horizontal rules
```

Tables/task lists can follow later.

---

# 31. Markdown Examples

Input:

````markdown
## Core flow

```text
Human + Planner
  -> explicit approval
  -> Relay
```
````

Display:

```text
Core flow

  Human + Planner
    -> explicit approval
    -> Relay
```

No literal:

```text
##
```text
```

---

# 32. Markdown Security

Before parsing/rendering:

```text
sanitize terminal control sequences
```

Raw HTML:

```text
do not execute
```

OSC/ANSI:

```text
do not pass through
```

Markdown is presentation text only.

---

# 33. Do Not Render Command stdout as Markdown Automatically

Command output remains Plain.

Reason:

```text
compiler output
Git output
test output
shell output
```

may contain Markdown-like syntax accidentally.

Only typed provider events explicitly marked Markdown use the Markdown renderer.

---

# 34. Agent Messages

`AgentMessage` should normally use:

```text
RuntimeTextFormat::Markdown
```

when the provider's documented structured event represents assistant-authored text.

If a provider cannot guarantee Markdown:

```text
Plain
```

is valid.

---

# 35. Hidden Reasoning Guardrail

Do not add:

```text
RuntimeActivity::ChainOfThought
RuntimeActivity::ReasoningText
```

Provider reasoning events may map to:

```text
Analyzing
Planning
Working
```

but not hidden internal reasoning content.

This is both a product and provider-compliance invariant.

---

# 36. Provider Compliance Checklist

Every adapter must document:

```text
machine interface used
official documentation/source reference
which event types Relay consumes
which event types Relay ignores
whether assistant text is Markdown
how commands are represented
how paths are represented
how final result is represented
```

Review must reject adapters that depend on:

```text
screen scraping
private undocumented session files
hidden reasoning extraction
credential inspection
safety bypass
```

---

# 37. ChatGPT-Specific Boundary

Do not use the ChatGPT consumer web/app UI as a Relay runtime by scraping its displayed responses.

If OpenAI is used as an executor, use:

```text
Codex CLI documented machine interface
OpenAI API / SDK where explicitly designed for programmatic use
```

A future ChatGPT integration, if any, needs its own officially supported programmatic interface and terms review.

---

# 38. Provider-Agnostic Compliance Trait

Optional but useful:

```rust
pub struct RuntimeInterfaceMetadata {
    pub provider_id: String,
    pub interface_name: String,
    pub documented_machine_interface: bool,
    pub structured_events: bool,
    pub interactive_ui_required: bool,
}
```

Run preflight:

```text
interactive_ui_required == true
→ not supported by Relay background execution
```

---

# 39. Source Attribution In Developer Documentation

Add:

```text
docs/runtime-provider-contracts.md
```

For each adapter:

```text
Codex
  Interface: codex exec --json
  Activity source: JSONL event stream
  Raw output: stderr + selected fields

Antigravity
  Interface: agy --print --output-format stream-json
  Activity source: typed NDJSON
```

Do not hard-code terms excerpts.

Document the interface and link/reference the current official docs for maintainers.

---

# 40. Suggested TUI Modules

```text
apps/relay-tui/src/

execution_view.rs
markdown.rs
diff.rs
path_display.rs
logs.rs
```

or fit into the existing planned `ui/` / `view_model/` structure.

Do not keep growing `main.rs`.

---

# 41. Suggested Runtime Changes

```text
crates/relay-runtime/src/activity.rs
crates/relay-runtime/src/lib.rs

crates/relay-core/src/execution.rs

runtimes/relay-runtime-codex/src/lib.rs
runtimes/relay-runtime-antigravity/src/lib.rs
```

---

# 42. Test — Codex Structured Activity

Fixture JSONL:

```text
thread.started
item.started command_execution
item.completed command_execution
item.completed file_change
item.completed agent_message
turn.completed
```

Expected:

```text
SessionStarted
CommandStarted
CommandCompleted
FileChanged
AgentMessage(Markdown)
Outcome
```

No raw UI strings such as:

```text
› Running
```

inside the adapter.

---

# 43. Test — Command Normalization

Input:

```text
powershell.exe -Command "cargo test --workspace"
```

Expected presentation:

```text
cargo test --workspace
```

Raw diagnostic preserved.

Test:

```text
bash -lc "cargo test --workspace"
```

same result.

Ambiguous input:

```text
fallback safely
```

---

# 44. Test — Path Normalization

Trusted root:

```text
C:\Users\PCC\Desktop\My Projects\Relay
```

Paths:

```text
root
→ .

root\crates\relay-core
→ crates/relay-core

C:\Windows
→ <outside trusted workspace>
```

---

# 45. Test — Diff Parser

Fixture:

```diff
diff --git a/src/lib.rs b/src/lib.rs
@@ -10,2 +10,3 @@
 context
-old
+new
+another
```

Expected:

```text
old/new line numbers correct
additions = 2
deletions = 1
```

---

# 46. Test — Markdown

Fixture:

````markdown
# Heading

- item
- **bold**

Use `cargo test`.

```rust
fn main() {}
```
````

Expected:

```text
no literal Markdown control syntax
semantic lines/spans
code block retained
```

---

# 47. Test — Markdown Safety

Input includes:

```text
ANSI escape
OSC title sequence
raw HTML
```

Expected:

```text
no terminal control execution
safe visible content only
```

---

# 48. Test — Plain Command Output

Input:

```text
# compiler diagnostic
* warning
```

when source is command stdout.

Expected:

```text
rendered literally as Plain
```

not Markdown.

---

# 49. Test — Read-Only Diff

Opening:

```text
/diff
```

must not:

```text
queue task
stage files
reset files
commit
push
```

---

# 50. Manual Acceptance

Use a low-risk Relay dogfood run.

Expected execution view:

```text
Read path
Edit path +n -n
Run logical command
cwd project-relative
Markdown provider summary formatted
```

Open:

```text
/logs raw
```

Expected:

```text
full wrapper paths and raw diagnostics
```

Open:

```text
/diff
```

Expected:

```text
structured hunk view
```

---

# 51. Hard Completion Gate

Do not call this complete if:

```text
[ ] normal execution view still shows powershell.exe full path
[ ] commands have no cwd context
[ ] user-home absolute paths dominate normal activity
[ ] provider adapter emits preformatted "› Running" UI strings
[ ] RuntimeActivity is still unused
[ ] provider messages show literal Markdown fences
[ ] command stdout is incorrectly treated as Markdown
[ ] diff view is still raw unified Git text only
[ ] provider-generated diff is treated as canonical
[ ] hidden reasoning text is displayed
[ ] provider TUI scraping is required
[ ] raw diagnostics disappear entirely
```

---

# 52. Recommended Commit Sequence

```text
1. feat[Jacinth]: carry structured runtime activity through Core

2. feat[Jacinth]: normalize provider commands and workspace paths

3. feat[Jacinth]: render structured Git diffs

4. feat[Jacinth]: render provider Markdown in Relay TUI

5. refactor[Jacinth]: separate formatted and raw execution logs

6. docs[Jacinth]: document provider machine-interface contract
```

---

# 53. Final Architecture

```text
Codex JSONL            Antigravity stream-json
      │                         │
      ↓                         ↓
 provider adapter        provider adapter
      │                         │
      └────── RuntimeActivity ──┘
                   │
                   ↓
            Relay Core events
                   │
         ┌─────────┼──────────┐
         ↓         ↓          ↓
      Activity   Raw Logs   Outcome
         │
         ↓
 local Git truth
         │
         ↓
 Relay presentation models
         │
         ├── normalized command
         ├── project-relative cwd
         ├── structured diff
         └── Markdown/Rich Text
                   ↓
                 TUI
```

The product rule is:

> **Relay abstracts execution, not provider internals.**

The compliance rule is:

> **Consume only documented machine interfaces and user-visible/provider-exposed output; never scrape hidden reasoning or interactive provider UI.**

The UX rule is:

> **Normal mode shows what happened. Raw mode shows how the provider/process represented it.**
