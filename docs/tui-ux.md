# TUI UX

This document describes the TUI **as implemented** in `apps/relay-tui`. It is not a design wishlist; anything not in the code today is omitted or labeled as such.

---

## 1. Design principles in force

These are the principles the current implementation actually follows:

- **Single workspace with panes, not page stacks.** One frame hosts every screen; screens swap into the content area while the top bar, prompt line, and key bar stay fixed. There is no nested navigation chrome.
- **Clean and minimal.** No decoration beyond panel borders, no color except semantic state, no static help text inside screen bodies — key hints live in one place (the key bar).
- **Data-relay focus.** The dashboard answers two questions at a glance: *what is running right now* (ACTIVE, DISTRIBUTION) and *what happens next* (queue projection, ESCALATIONS). It is a projection of scheduler and durable state, never an editor of intent.
- **Handoff language, not orchestrator language** ([ADR-0017](adr/0017-relay-identity-deterministic-handoff-engine.md)). User-facing copy says "handoff", "escalation", "distribution"; internal identifiers keep scheduler/runtime names. See [architecture.md](architecture.md) §"What Relay Is / Is Not".
- **Presentation owns nothing.** The TUI gathers intent, calls Relay Core (`plan_execution`, scheduler), and renders results. It does not own execution policy, ingress validation, routing, repository checks, or persistence decisions.

---

## 2. Frame anatomy

Every frame is split vertically into four regions (`ui/mod.rs`):

```text
+--------------------------------------------------------------+
| relay · dashboard                        codex ●             |  top bar (1 row)
+--------------------------------------------------------------+
|                                                              |
|  content area (screen-specific; Min(3) rows)                 |
|                                                              |
+--------------------------------------------------------------+
| > /_  (type / for commands)                                  |  prompt line (1 row)
+--------------------------------------------------------------+
| n new handoff  t tasks  p projects  / commands  q quit       |  key bar (1 row)
+--------------------------------------------------------------+
```

- **Top bar.** `relay · <screen title>` on the left; active provider name plus a health marker on the right (`●` green when ready, `…` muted while checking, `◌` red otherwise).
- **Content area.** Dispatches to the renderer for the current screen (see §6).
- **Prompt line.** Idle state shows the muted hint `> /_  (type / for commands)`; in command mode it becomes a live input `> <typed text>`.
- **Key bar.** Dynamically derived hints (§4).

When command mode is active, a bordered **command palette overlay** floats over the lower frame: up to 6 suggestions with completion, description, and disabled reasons where applicable. Enter prefers a command that was typed exactly over the highlighted suggestion; Tab completes.

---

## 3. Dashboard

The dashboard (`ui/dashboard.rs`) is three bordered panels: **ESCALATIONS**, **DISTRIBUTION**, **ACTIVE**.

### Layout rules

- Content width greater than 100 columns: three equal horizontal thirds.
- Otherwise: vertical stack in priority order — ESCALATIONS (45%), DISTRIBUTION (25%), ACTIVE (remaining fill). Escalations come first because they block on a human.
- If `state.feedback` is non-empty, one bottom row is reserved for it (muted text).
- With zero registered projects, the dashboard shows a welcome message instead of panels.

### ESCALATIONS panel

Lists durable tasks in `NeedsClarification` or `NeedsDecision` (the escalation inbox):

```text
! DECISION Approve migration strategy
   practicare · 3f2a91c0
! CLARIFICATION Ambiguous verification step
   relay · 8be41d77
```

Kind label style: `CLARIFICATION` warning-colored, `DECISION` error-colored. Each entry shows the truncated title and `project · short-id`. Empty state: "No escalations waiting."

### DISTRIBUTION panel

Per-provider work counts derived from the scheduler snapshot (running) plus durable `Done` tasks, keyed by each task's runtime profile:

```text
Codex (codex/implement): 1 running · 4 completed
```

Rows with zero running and zero completed are omitted. Empty state: "No approved work yet."

### ACTIVE panel and queue projection

Renders, in order:

1. Running executions: `● <title> <elapsed>s`.
2. Queued count (`N queued`), or `◌ No active tasks.` when nothing runs and nothing queues.
3. **Queue projection lines** — a deterministic derivation from the scheduler snapshot, durable tasks, and execution limits:
   - Up to three pending handoffs in scheduler order as `next: <title> -> <Provider>`.
   - If global concurrency slots are full: `waiting for execution slot (<active>/<max> active)`.
   - Else if the head of the queue's own project cap is full: `waiting for project slot on <project> (<active>/<cap> active)`.
4. Short-form escalation titles, so blocked work stays visible even when the ESCALATIONS panel is off-screen on narrow terminals.

---

## 4. The dynamic key bar

The key bar is never hand-written into a screen body. `ui/mod.rs::key_hints` returns `(key, label)` pairs whose match arms deliberately mirror `input.rs` — including state-dependent branches such as Settings edit mode, TaskReview submenu state, Execution permission prompts, and cancel confirmation. The bar re-renders every frame, so hints always describe what the current keys actually do:

```text
normal:            c cancel  b tasks  / commands
permission popup:  y allow once  s allow this run  n deny
cancel confirm:    y confirm cancel  n keep running
```

Where command mode is available (`command_mode_available`), a `/ commands` hint is appended. In command mode the bar switches to `tab complete  enter run  esc close`.

Rule for future work: adding a key binding means updating both `input.rs` and the matching `key_hints` arm in the same change. Static key text inside screen bodies is a defect.

---

## 5. Settings screen

Settings is an interactive, validated form (`ui/runtime.rs`; form logic in `state.rs`). It is not free-text configuration editing: only execution limits are editable.

### Sections shown

- **Execution** — the four editable fields: `Max parallel`, `Per-project max parallel`, `Max repair attempts`, `Max duration (minutes)`.
- **Safety** — locked invariants: explicit Run approval On, automatic commit Off, automatic push Off. These rows render a `[locked]` tag (warning color) and use `#` as their cursor marker; they are browsable and explained via inline hints but never enter edit mode. This is deliberate: explicit local Run approval and never-auto-push are product invariants 13/14, not user preferences.
- **Storage** — config file path and SQLite database path.
- **Runtime** — active provider, profile, and health status.
- A reserved status line at the panel bottom shows an inline validation error (error color), a muted `Saved.`, or nothing.

### Browse vs edit modes

- **Browse:** Up/Down select a field; Enter or Left/Right starts editing (buffer seeded with the current value); typing a digit starts editing with that digit as seed; `r` re-checks health; Esc returns to dashboard.
- **Edit:** digits append to the buffer, Backspace deletes, Enter commits, Esc cancels. The edited value renders inline as `<buffer>_`.

### Validation rules (commit time)

1. The value must parse as a positive integer.
2. Repair attempts must fit `u32`.
3. Duration must be between 1 and the Codex maximum (240 minutes).
4. The full candidate limit set must pass relay-core's own `ExecutionLimits` validation, so the form surfaces exactly the engine's rejection reasons.
5. Per-project parallelism may not exceed global parallelism.

### Live vs persisted

On successful save the value is written to the runtime settings TOML file **and** adopted into in-memory execution limits immediately — changes apply to subsequently queued executions without restart and persist across launches. A failed write surfaces `Save failed: …` and leaves memory unchanged.

---

## 6. Screen inventory

| Screen | Purpose | Entry points |
|---|---|---|
| Dashboard | Escalation inbox, distribution, active work + queue projection | default screen; `n` new handoff, `t` tasks, `p` projects, `a` add project; `/home` |
| Tasks | Durable task list with selection | `t` from dashboard; `/tasks` |
| Projects | Trusted project registry listing, repo re-check | `p`; `/projects` |
| Add Project | Register a local Git repository (multi-field form) | `a`; `/project add [path]` |
| New Task | Choose paste-task or import-file ingress | `n`; `/new` |
| Import File | Enter a canonical JSON / Markdown-wrapped task file path | `f` from New Task |
| Paste Task | Multi-line paste editor (Ctrl+Enter inserts a newline) | `v` from New Task |
| Task Preview | Validate-before-import preview; confirm or cancel | automatic after Import/Paste submit |
| Task Review | Cached preflight, per-run model/reasoning overrides, Run approval | Enter on a task in Tasks; `/run <id>` |
| Execution | Live output, permission decisions, bounded cancel confirmation | Run approval; `/cancel <id>` |
| Result | Done/Failed outcome summary | automatic on run completion |
| Diff | Read-only git diff of the executed repository | `/diff <id>` |
| Logs | Execution log viewer | `/logs <id>` |
| Runtimes | Provider list with health markers and profiles | `/runtimes` |
| Runtime Detail | Provider detail, model/reasoning selectors, test runtime | `/runtime <provider>` plus `use`, `model`, `reasoning`, `test` subcommands |
| Settings | Interactive execution-limit form (§5) | `/settings` |
| Doctor | System diagnostics: git health, config writability, SQLite, Codex CLI detail (executable, version, auth), Antigravity status; also reports Relay Direct / MCP (`relay serve-mcp`) as implemented. Enter on a provider row opens that runtime's detail screen; Enter on a git/config/sqlite row surfaces remediation guidance in the feedback line | `/doctor` |
| Updates | Installed component versions (Relay itself, Codex CLI, Antigravity CLI, Git) from local probes only — Relay never phones home — plus update guidance per component; `r` re-runs the probes | `/updates` |

Global keys: Ctrl+C quits from anywhere; Esc navigates back per screen; `/` opens command mode on most screens.

---

## 7. Theme usage rules

The theme (`theme.rs`) defines semantic roles: `accent`, `success`, `warning`, `error`, `muted`, `text`, `selection`, `diff_added`, `diff_removed`. Usage rules observed across screens:

- **Muted labels, bright values.** Field labels, secondary text, counts, and empty states render in `muted`; primary values and content render in `text`. Panel borders are muted; only titles use `header` (accent + bold).
- **State-only color.** Green/red/yellow appear only for state markers (`●`/`◌`/`!`/`✓`/`×`), escalation kinds, validation errors, and diff semantics — never decoration.
- **Selection is accent + bold**, not a background highlight.
- **Status markers are fixed glyphs**: `●` active, `◌` waiting/unavailable, `!` attention, `✓` success, `×` failure, `?` unknown.
- Diff rendering maps patch lines to `diff_added` / `diff_removed`, headers to accent/muted roles.

---

## 8. Design history

The original TUI rendered each screen as a paragraph of plain text lines with static help text baked into the body. During the M8.1–M9 implementation window (August 2026, landed on `feat/m9-relay-direct-mcp` in commits `eb473e2` "redesign TUI frame with dynamic key bar and panel dashboard", `008f141` interactive Settings form, and `d0551ce` queue projection), that layout was replaced by:

- the fixed four-region frame with the dynamic key bar;
- the three-panel responsive dashboard;
- the monolith split into `state`, `input`, `commands`, `util`, and `ui/` modules.

Earlier documents describing per-screen paragraph mockups (including the previous version of this file) describe superseded designs.

---

## Core boundary

The TUI receives domain state, domain events, and command results from Relay Core. It does not directly spawn Codex, call GitHub, inspect Git remotes, or write SQLite. See [architecture.md](architecture.md).

## Related documents

- [local-ingress.md](local-ingress.md) — task entry formats
- [clarification-protocol.md](clarification-protocol.md) — escalation packets surfaced in the inbox
- [adr/0017-relay-identity-deterministic-handoff-engine.md](adr/0017-relay-identity-deterministic-handoff-engine.md) — language rules
- [cli.md](cli.md) — CLI and TUI command reference
