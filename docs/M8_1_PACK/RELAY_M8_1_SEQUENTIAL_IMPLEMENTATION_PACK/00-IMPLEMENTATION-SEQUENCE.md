# Relay M8.1 — Sequential Implementation Pack

**Status:** DEV implementation sequence  
**Starting point:** Candidate `45b8008bea8b3639d3cbf33d9aa84895fd674cfd`  
**Purpose:** Apply the post-command-palette DEV corrections in dependency order without collapsing all work into one oversized implementation document.

## Recommended implementation order

### 01 — DEV Gate Remediation

File: `01-DEV-GATE-REMEDIATION-45B8008.md`

Start here. Resolve the candidate-specific correctness defects first, especially:

- production `RuntimeRouter` wiring;
- provider-neutral Run preflight;
- frozen runtime identity in execution/result views;
- runtime settings integration;
- command-specific task autocomplete eligibility;
- validated `/runtime use` and provider-specific `/runtime test`;
- Antigravity result/event/capability correctness;
- discovery-first project add flow;
- remaining input/event-loop correctness.

Do not rewrite the contextual command palette. Keep it and correct the state behind it.

### 02 — Codex App-Server Architecture Amendment

File: `02-CODEX-APP-SERVER-ARCHITECTURE-AMENDMENT.md`

This is the architectural foundation for the later Codex-specific work.

Implement the **additive M6 runtime-control amendment** while preserving accepted M6 scheduler semantics. Migrate the preferred Codex rich-client path toward local `codex app-server` over stdio JSONL.

This file supersedes earlier assumptions that Codex must remain primarily a `codex exec` batch wrapper for Relay's interactive runtime.

Key foundation:

```text
M6 scheduler semantics remain frozen
        +
RuntimeSession gains a control channel
        +
Codex adapter uses documented app-server primitives
```

Do not enable unrelated experimental app-server APIs merely because they exist.

### 03 — Runtime Model & Reasoning Control

File: `03-RUNTIME-MODEL-REASONING-CONTROL.md`

Implement model/reasoning tuning after the runtime selection and app-server foundations are stable.

Important update from the app-server reference:

- prefer Codex `model/list` for the actual available model catalog;
- use each model's `supportedReasoningEfforts` rather than a permanently hard-coded reasoning menu;
- freeze provider/profile/model/reasoning into the execution policy before queueing;
- preserve global, project, and one-run override layers;
- do not silently change already queued/running executions.

The safety profile and AI tuning remain separate concepts.

### 04 — Provider-Neutral Activity, Diff & Markdown Rendering

File: `04-PROVIDER-NEUTRAL-ACTIVITY-DIFF-MARKDOWN.md`

Once provider sessions produce structured events, normalize them into Relay activity.

Implement:

- `RuntimeActivity` end-to-end wiring;
- logical command presentation instead of raw launcher paths;
- compact project-relative cwd display;
- provider-authored Markdown rendering;
- plain command stdout by default;
- structured local-Git diff rendering;
- provider-side live diff/activity only as hints/previews;
- local Git remains final repository truth;
- raw diagnostic mode remains available.

For Codex app-server, use structured item/turn notifications rather than parsing the consumer TUI.

### 05 — Permission Broker + Scrollable Execution/Result UX

File: `05-PERMISSION-BROKER-SCROLLABLE-EXECUTION-RESULT.md`

Apply this after the M6 control-channel amendment exists.

The provider-neutral permission broker remains authoritative:

```text
provider request
    ↓
RuntimeEvent::PermissionRequested
    ↓
M6 scheduler / frozen permission ceiling
    ↓
Relay TUI human decision
    ↓
RuntimeControl::PermissionDecision
    ↓
provider continues same execution
```

For Codex, implement the provider-specific side using documented app-server approval requests rather than inventing a TUI-to-Codex side channel.

Also finish:

- `WAITING FOR PERMISSION` execution UX;
- no repair-attempt consumption for permission blockers;
- permission ceiling enforcement;
- scrollable Result;
- scrollable live Execution with pause/follow-tail semantics;
- page/home/end navigation for Diff/Logs;
- exact frozen execution metadata in Results.

## Supersedence rules

Later documents refine earlier implementation assumptions. Use these rules when they conflict:

1. `02-CODEX-APP-SERVER-ARCHITECTURE-AMENDMENT.md` supersedes older Codex-specific assumptions in 01/03/04/05 where those assumptions rely on `codex exec` as the only rich integration path.
2. The accepted M2/M3/M4/M5/M6/M7 invariants still take precedence unless the amendment explicitly calls out an additive evolution.
3. The app-server migration must **not** bypass the M6 scheduler, M5 SQLite authority, M3 repository boundary, or M7 explicit Run approval.
4. Local Git remains authoritative for final repository state even when a provider exposes live diff events.
5. Provider-native model/auth/approval information should be used when documented rather than duplicated with stale hard-coded catalogs.
6. Do not silently fall back from ChatGPT/subscription authentication to API-key billing.

## Recommended commit phases

A practical sequence is:

```text
A. candidate correctness / read-only command safety / real RuntimeRouter
B. additive M6 runtime-control channel
C. Codex app-server client + auth/model/probe basics
D. model/reasoning execution snapshot
E. provider-neutral activity + rich presentation
F. live permission broker + waiting state
G. scrolling/follow-tail polish
H. dogfood + Review evidence
```

Keep commits reviewable. Do not make one giant M8.1 commit.

## Required final DEV gate

Before forwarding to the Review Branch, confirm at minimum:

```text
Runtime switching changes the actual provider execution path.
Queued executions retain their frozen provider/profile/model/reasoning.
/diff and /logs never execute tasks.
/cancel suggestions operate on cancellable tasks.
Codex model/reasoning choices come from actual provider capability data where available.
Codex authentication mode is explicit and does not silently change billing mode.
Permission requests do not become implementation failures or consume repair attempts.
Relay remains the terminal owner.
Result/Execution/Diff/Logs are scrollable as specified.
Provider Markdown and structured activity are rendered through Relay's own presentation layer.
Local Git remains final diff/repository truth.
All accepted scheduler/persistence/cancellation/concurrency invariants still pass.
```

Then return the exact successor SHA and verification evidence to the DEV gate before Review.
