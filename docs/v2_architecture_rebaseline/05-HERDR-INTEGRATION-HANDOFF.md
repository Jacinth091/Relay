# Relay v2 Herdr Integration Handoff

# 1. Positioning

Herdr is Relay v2's **default/reference interactive runtime backplane**.

It is not:

- the task owner;
- the workflow engine;
- the provider resolver;
- the project trust authority;
- the worktree/candidate authority;
- the artifact protocol;
- the policy engine.

Relay must remain semantically correct when Herdr is absent or temporarily unavailable.

# 2. Why a Real Backplane Seam Exists

v2 deliberately maintains at least:

```text
FakeBackplane
LocalProcessBackplane
HerdrBackplane
```

This makes the seam concrete rather than hypothetical and prevents core semantics from becoming Herdr topology semantics.

# 3. RuntimeBackplane Interface

External conceptual interface:

```rust
trait RuntimeBackplane {
    async fn provision(req: RuntimeProvisionRequest) -> RuntimeHandle;
    async fn observe(handle: RuntimeHandle) -> RuntimeObservation;
    async fn send(handle: RuntimeHandle, input: RuntimeInput) -> SendReceipt;
    async fn interrupt(handle: RuntimeHandle) -> InterruptReceipt;
    async fn dispose(handle: RuntimeHandle) -> DisposeReceipt;
    async fn snapshot() -> BackplaneSnapshot;
    async fn reconcile(binding: RuntimeBinding) -> ReconcileOutcome;
}
```

Do not expose `create_workspace`, `create_tab`, `create_pane` to Relay Core.

# 4. Herdr Internal Topology Planner

Inside the Herdr adapter, map Relay runtime requests to Herdr physical resources.

Default policy may be:

```text
Checkout -> Workspace
Task -> Tab
RoleAttempt -> Pane/process
```

But this is an adapter policy and may evolve without changing domain IDs.

# 5. Logical vs Physical Identity

Never equate:

```text
TaskId == HerdrTabId
RoleInstanceId == HerdrPaneId
RoleAttemptId == PID
```

Persist a `RuntimeBinding` that maps Relay attempt identity to adapter/runtime handle metadata.

# 6. Worktree Ownership

Relay's WorktreeManager owns creation/selection/validation/cleanup.

Herdr receives trusted CWD in `RuntimeProvisionRequest`.

If Herdr exposes useful worktree primitives, they may be used behind a dedicated repository/worktree adapter only if the semantic ownership remains Relay's. Do not make the process backplane decide repository state.

# 7. Provider Launch Separation

Flow:

```text
RoleAttempt
-> ExecutionTarget
-> ProviderAdapter.build_launch(...)
-> LaunchSpecification
-> RuntimeBackplane.provision(...)
```

Herdr does not need to understand Codex/Claude/AGY role semantics.

# 8. Provision Idempotency

Provisioning is an external effect.

Relay creates a stable effect key before calling Herdr.

Herdr adapter should attach Relay metadata when supported:

- task ID;
- workflow run ID;
- role attempt ID;
- effect key;
- checkout ID.

On timeout/response loss, reconcile by metadata/snapshot before creating a second pane/process.

# 9. Transport / Capability Negotiation

At daemon/backplane startup:

- discover/connect Herdr;
- verify compatible protocol/features;
- record capability snapshot;
- refuse unsupported operations explicitly;
- do not silently downgrade critical guarantees.

Keep transport-specific details inside the adapter.

# 10. Runtime Observations

Normalize physical observations into provider-neutral facts:

```text
PROVISIONED
STARTING
RUNNING
IDLE_OR_WAITING_INPUT
EXITED
INTERRUPTED
MISSING
ORPHANED
MISMATCHED
UNKNOWN
```

ProviderAdapter may further interpret output/readiness into provider-specific states such as auth/quota/result readiness.

# 11. Input / Output

Herdr handles transport of terminal input/output.

Relay requirements:

- bounded capture;
- raw output is diagnostic evidence;
- secrets/redaction policy applies;
- structured result must arrive through Exchange/result publication path;
- conversation-log parsing is fallback only.

# 12. Manual User Intervention

Users may open Herdr and interact manually.

Relay should distinguish observations such as:

- user attached;
- process terminated externally;
- unexpected input/activity where detectable;
- pane renamed/moved if topology changes;
- process replaced/missing.

Manual intervention does not automatically invalidate work, but it may reduce provenance certainty. Policy can require fresh verification or explicit acknowledgment.

# 13. Relay Restart / Herdr Survives

Recovery:

```text
Relay loads active RuntimeBindings
-> Herdr snapshot
-> match by stable metadata/handle
-> reconcile each binding
-> ATTACHED / EXITED / MISSING / ORPHANED / MISMATCHED / AMBIGUOUS
```

Never create duplicate runtime resources merely because the first RPC reply was lost.

# 14. Herdr Restart / Relay Survives

Relay remains canonical.

If Herdr loses sessions:

- mark bindings lost/missing;
- do not mark role complete;
- provider/result artifacts already durably published remain;
- workflow decides bounded retry/new attempt/escalation;
- leases/candidates are reconciled independently of Herdr.

# 15. Daemon and Herdr Lifetime

Relay daemon and Herdr server may have independent lifetimes.

Do not make closing Relay Web/TUI kill Herdr resources unless an explicit task/runtime action requests cleanup.

# 16. LocalProcessBackplane Comparison

LocalProcess path may support only:

- process spawn;
- stdio capture/input;
- interrupt/kill;
- process observation.

It does not need workspaces/tabs/panes. This validates why those concepts cannot be in the common interface.

# 17. Provider Readiness

Provider-specific readiness belongs to ProviderAdapter.

Examples of normalized provider facts:

```text
READY_FOR_ASSIGNMENT
AUTH_REQUIRED
QUOTA_EXHAUSTED
WAITING_FOR_USER
RESULT_PUBLISHED
PROVIDER_ERROR
```

Herdr reports physical process observations; adapters interpret provider semantics.

# 18. Role Result Contract

Even if Herdr reports clean process exit:

```text
no valid typed result artifact
=> RoleInstance cannot be semantically COMPLETED
```

A result may already have been published before the process later crashes; Relay uses the durable result validation state, not a simplistic exit-code rule.

# 19. Cancellation / Interrupt

`InterruptAttempt` maps to Herdr interrupt/termination behavior according to capabilities.

`CancelWorkflow` is higher-level Relay intent and may issue interrupts/dispose effects after durable cancellation state is recorded.

If interruption success is ambiguous, record/reconcile it rather than claiming stopped.

# 20. Security / Sandbox Truth

Herdr process hosting does not automatically imply sandboxing.

Record active enforcement capabilities separately:

- filesystem sandbox;
- network restrictions;
- command restrictions;
- environment/secret exposure;
- OS user isolation.

Relay policy UI/docs must not claim Herdr enforces restrictions it does not actually enforce.

# 21. Runtime Metadata

Where Herdr permits metadata/tags, attach enough for reconciliation without embedding secrets:

```text
relay_state_root_id
project_id
checkout_id
task_id
workflow_run_id
role_instance_id
role_attempt_id
effect_key
```

Do not store role-session tokens/provider secrets in Herdr labels.

# 22. Raw Logs

Capture bounded stdout/stderr/terminal transcript as optional RawLog artifacts.

Requirements:

- size/rotation bounds;
- secret redaction policy where possible;
- provenance;
- retention policy;
- never use raw logs as the canonical typed protocol.

# 23. Cleanup Ownership

Relay records whether a runtime resource was created/owned by Relay.

Cleanup rules:

- never delete user-created unrelated Herdr resources;
- do not clean resources still referenced by active attempts;
- cleanup external resource only after durable semantic decision/effect intent;
- ambiguous cleanup outcome is reconciled.

# 24. Herdr Integration Tests

## Pure adapter tests

- topology planner mapping;
- metadata encoding/decoding;
- observation normalization;
- capability negotiation;
- reconciliation matching;
- idempotency lookup behavior.

## Protocol tests

Using fake/test Herdr transport:

- provision;
- timeout after server-side success;
- retry/reconcile without duplicate;
- send input;
- observe exit;
- interrupt;
- missing resource;
- restart snapshot.

## Installed-Herdr smoke tests

Optional/environment-gated:

- launch one harmless provider/shell fixture;
- attach/open resource;
- restart Relay and reattach;
- terminate externally and reconcile;
- cleanup owned resources.

Do not make local developer CI require a paid provider session merely to test the adapter.

# 25. Acceptance Criteria

1. Relay Core has no Herdr topology types.
2. HerdrBackplane satisfies the same runtime interface as Fake/LocalProcess.
3. Worktree ownership remains Relay-side.
4. Provider semantics remain ProviderAdapter-side.
5. provision operations are idempotent/reconcilable.
6. Relay restart can reattach/reconcile surviving Herdr resources.
7. Herdr loss does not fabricate semantic role completion.
8. raw logs are secondary diagnostics.
9. manual terminal access remains possible.
10. Herdr absence does not corrupt or invalidate the v2 domain model.
