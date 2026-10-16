# Relay v2 Web + TUI + Herdr UX Handoff

# 1. Product UX Boundary

Relay UI explains **semantic work**. Herdr explains **physical runtime**.

Relay should answer:

- What did I approve?
- What is Relay doing now?
- Which role is responsible?
- Which provider/attempt is active?
- What candidate is being verified?
- What needs my attention?
- What is allowed vs merely advisory?
- What failed/retried and why?

Herdr should answer:

- What terminal/process is actually running?
- What pane/session is it in?
- What raw output is visible?

Do not recreate a full terminal multiplexer inside Relay Web.

# 2. Primary Interface

Primary v2 interface:

```text
local Web UI -> Relay daemon
```

Power-user client:

```text
Relay TUI -> Relay daemon
```

Raw runtime/debug interface:

```text
Herdr UI/session or direct local-process diagnostics
```

Closing Relay clients does not stop daemon workflows.

# 3. Connection / State Model

Clients load:

```text
snapshot at event sequence N
-> subscribe/request events > N
```

Every screen renders daemon state, not client-local workflow truth.

Connection states:

```text
CONNECTED
RECONNECTING
STALE_SNAPSHOT
DAEMON_UNAVAILABLE
PROTOCOL_INCOMPATIBLE
```

A stale/disconnected client must not imply an action succeeded.

# 4. Global Dashboard

Show:

- active projects;
- Ready tasks awaiting Run;
- running/suspended workflows;
- Attention Inbox count by clarification/decision/approval;
- provider health/capacity summaries;
- active attempts;
- failed/lost/recovering runtime bindings;
- global concurrency/budget usage;
- recent completed/failed/cancelled tasks.

Do not make raw provider terminal output the dashboard centerpiece.

# 5. Project View

Show:

- logical project identity;
- trusted checkouts;
- checkout trust/remote status;
- active tasks/workflows;
- current policy summary and enforcement strength;
- configured workflow/skill/context declarations;
- provider/runtime profile preferences;
- worktree/candidate/lease activity;
- project-scoped history.

Clearly distinguish repository-declared configuration from locally trusted configuration.

# 6. Task Detail

Task header:

- task state;
- project/checkout;
- current ApprovedIntent generation;
- source handoff/spec;
- ExecutionGrant status;
- workflow version/config snapshot;
- execution budget remaining;
- attention state.

Primary actions depend on state:

```text
READY -> Run
RUNNING -> Suspend / Cancel
SUSPENDED -> Resume / Cancel
active attempt -> Inspect / Interrupt Attempt
NEEDS_* -> Resolve corresponding attention item
```

Do not show one ambiguous `Pause` button.

# 7. Run Review Screen

Before Run, show what the execution grant will authorize:

- approved objective;
- constraints/non-goals;
- workflow;
- roles that may be activated;
- permitted delegation classes;
- project/checkout/worktree scope;
- budget;
- provider preference/pins;
- capabilities that still require separate approval;
- enforcement limitations.

Run creates a durable ExecutionGrant. It does not pre-authorize future push/deploy/destructive operations.

# 8. Approved Intent History

Show intent generations as immutable revisions.

A scope expansion flow should display:

```text
current approved scope
requested addition/change
reason
impact
Accept as new intent generation / Reject
```

Do not overwrite the old scope in place.

# 9. Workflow Graph

Initial v2 graph is an operational read-only visualization.

Nodes show:

- role name/generation;
- semantic state;
- active/previous attempts;
- selected provider/runtime profile;
- required/accepted result;
- candidate generation;
- blocking dependencies;
- delegation children.

Edges distinguish:

- static workflow transition;
- blocking delegation;
- consultation;
- remediation loop.

# 10. Workflow Builder — Deferred

Do not make the graphical builder part of the early v2 critical path.

When implemented later:

- drafts are user-local by default;
- schema/version validation is live;
- invalid/deadlocked graphs cannot activate;
- saving to `.relay/workflows/` is an explicit repository modification;
- UI warns that repository files will change;
- no arbitrary executable transition expressions.

# 11. Role / Attempt Inspector

Role panel:

- role definition/purpose;
- inputs;
- required result schema;
- independence rule;
- state;
- parent/children;
- accepted result artifact.

Attempt panel:

- attempt number;
- provider + runtime profile;
- capability/health snapshot;
- runtime state;
- start/end;
- retry reason;
- runtime binding;
- raw log link;
- published result validation state.

This UI makes provider replacement understandable without pretending a new semantic role was created.

# 12. Candidate / Verification View

Show:

- candidate generation;
- producer role/attempt;
- base/current HEAD;
- fingerprint status;
- verification lease/fencing generation;
- verifier independence properties;
- checks/evidence;
- PASS/FAIL/INVALIDATED.

If candidate changed during verification, display `INVALIDATED` rather than a stale PASS.

# 13. Provider Screen

Show provider family separately from runtime profiles.

For each provider:

- installed/discovered;
- auth state where observable;
- health;
- quota category where observable;
- supported capabilities;
- configured runtime profiles;
- active attempts/capacity;
- recent failures.

Do not imply Relay bypasses provider subscription/billing/quota rules.

# 14. Policy / Enforcement Screen

For sensitive capabilities, show both policy disposition and enforcement class.

Example:

```text
Git push
Disposition: REQUIRE_APPROVAL
Enforcement: RELAY_ENFORCED for Relay action executor
Provider shell bypass risk: not sandbox-enforced
```

This is deliberately more truthful than a simple green/red permission toggle.

# 15. Attention Inbox

One inbox, three semantic sections:

## Clarifications

Missing facts needed to continue.

## Decisions

Architecture/scope/design choices requiring human judgment.

## Approvals

Exact side effects requiring authorization.

Each item shows what is blocked and what happens after resolution.

# 16. Approval UX

Approval screen shows:

- exact operation kind;
- exact parameters;
- project/task;
- candidate generation if relevant;
- requesting role/attempt;
- policy reason;
- expiry/replay semantics;
- enforcement class;
- diff/plan artifact when relevant.

Material parameter changes invalidate the old approval and produce a new request.

# 17. Artifact Viewer

Show:

- artifact type/schema;
- producer role/attempt;
- trust class (`ProviderClaim`, `RelayObserved`, `Derived`);
- candidate linkage;
- checksum/size;
- sensitivity/retention;
- referenced artifacts.

Raw logs are secondary evidence, not the default semantic result.

# 18. Decision Log

Combine durable events into an explainable timeline:

```text
Task imported
Run granted
Workflow started
Implementer created
Attempt 1 Codex started
Attempt 1 quota exhausted
Attempt 2 Claude started
Candidate 1 published
Verifier created
Candidate verification PASS
Task completed
```

Include policy/approval/semantic-decision records without exposing secrets.

# 19. Runtime View

Show semantic-to-physical mapping:

```text
RoleAttempt
-> backplane
-> runtime handle
-> observed status
```

For Herdr, optionally display workspace/tab/pane metadata as diagnostics only.

Actions:

- open in Herdr;
- view bounded raw log;
- interrupt attempt;
- reconcile now where appropriate.

# 20. Suspend vs Interrupt vs Cancel UX

Use explicit wording:

- **Suspend workflow** — stop new work from being scheduled;
- **Interrupt attempt** — interrupt this running provider session;
- **Cancel workflow** — cancel remaining workflow work and clean up safely.

Never label these all as Pause/Stop interchangeably.

# 21. Budget UX

Show enforceable limits separately from estimates.

Examples:

```text
Role activations: 4 / 12
Delegations: 1 / 4
Concurrent attempts: 2 / 3
Wall-clock limit: enforceable
Provider token/currency estimate: informational only
```

# 22. Skills / Context UI

Display:

- source: user-local vs repository;
- trust level;
- content hash/version used by current run;
- roles using it;
- whether it is mandatory/optional context.

A skill cannot show or grant authority.

# 23. Semantic Decision UI

If enabled, show semantic decisions as advisory provenance:

- decision kind;
- legal choices supplied;
- engine/version;
- recommendation;
- confidence/probabilities where available;
- whether it was shadow-only;
- actual final action and why.

Do not present model confidence as authorization.

# 24. Local Web Security

Requirements:

- bind locally by default;
- authenticate browser session to local daemon;
- CSRF/origin protections appropriate to local Web architecture;
- never expose role-session capabilities to unrelated browser content;
- do not store provider secrets in frontend persistence;
- redact sensitive artifact/log data by policy;
- protocol version mismatch fails clearly.

Remote access is out of initial v2 scope unless separately designed.

# 25. TUI Redesign

Preserve efficient keyboard-driven views:

```text
Dashboard
Projects
Tasks
Attention
Providers
```

Task detail includes:

- ApprovedIntent summary;
- workflow/roles;
- candidate/result;
- attempts;
- actions.

The TUI is a semantic client, not the only engine process.

# 26. Client Independence

Required scenarios:

- start a task from Web, inspect it in TUI;
- close both, workflow continues;
- reopen after daemon events advanced;
- snapshot/event catch-up yields same state;
- start from CLI/TUI, inspect from Web;
- planner MCP submission creates Ready task visible everywhere.

# 27. UI Acceptance Scenarios

1. User sees exactly what Run authorizes before execution.
2. Scope expansion cannot be mistaken for ordinary clarification.
3. Provider retry is visible as another attempt of the same role.
4. Candidate invalidation cannot still display PASS.
5. Policy screen distinguishes advisory vs enforceable restrictions.
6. Approval binds to exact operation parameters.
7. Suspend/Interrupt/Cancel are visibly distinct.
8. A disconnected UI never claims current runtime state.
9. Herdr raw terminal remains accessible without becoming Relay's semantic UI.
10. Jev/semantic decisions are visibly optional/advisory.
