# M10 Phase 02 — Fan-Out Execution Model

## Goal

One locally approved handoff can run on N chosen providers simultaneously as sibling executions grouped under one parent task identity, with deterministic slot accounting, cancellation propagation, and disagreement handling.

## Current state

Exists today:

- `submit_with_policy` (`crates/relay-core/src/execution.rs`) accepts exactly one `ExecutionRunPolicy` per task; a task produces at most one execution on one runtime (ADR-0018, item 3).
- The scheduler keys active work by `TaskId` (`SchedulerInner.active: HashMap<TaskId, ActiveExecution>`), enforces global `max_parallel` and per-project `per_project_max_parallel`, and supports per-task cancellation via `cancel(task_id)`.
- `plan_execution` turns intent into one validated, clamped plan; the TUI gathers intent at Run approval and calls it before submission.
- Durable state is a flat per-task state machine (`TaskState`: Ready, InProgress, NeedsClarification, NeedsDecision, Done, Failed, Cancelled) in SQLite.

Does not exist:

- Any notion of sibling executions, parent grouping, or more than one live execution per task.

## Expressing fan-out intent without touching HandoffTask v1

The wire format stays byte-compatible. HandoffTask v1 structurally accepts an `extensions` map, but core preflight rejects unsupported namespaces and M9's boundary policy rejects non-empty extensions outright — by design, fail closed (invariant 17). Therefore:

- **Fan-out intent is NOT carried in the task payload.** No new extension namespace, no schema bump. A stored task is identical whether or not it will later fan out.
- **Fan-out intent is local Run-approval configuration.** At Run approval time the operator chooses "fan out to: [codex/implement, antigravity/implement]" as part of the execution *intent* passed to planning — the same surface that already carries preferred profile/model/effort. This satisfies ADR-0018's boundary condition: distribution happens after approval, never instead of it (invariant 13), and remote-originated tasks still cannot select execution paths (invariant 12).
- Because intent lives outside the protocol, MCP-submitted tasks are fan-out-capable only through a later local Run approval in the TUI. No planner can request fan-out over the wire in M10. This is deliberate; revisit only with a protocol version change and its own ADR.

## Requirements

1. **Parent/sibling durable-state model.**
   - The approved task keeps its canonical `TaskId` (invariant 22). Nothing about task identity changes.
   - Each fan-out leg becomes a sibling *execution record* with a Relay-generated child identifier, persisted in `relay-store` with a `parent_task_id` column referencing the canonical task. Siblings are executions, not tasks: they do not appear as independent entries in task listing, review, or ingress.
   - The parent task's durable `TaskState` becomes a deterministic rollup of sibling outcomes (rule in requirement 5). Per-sibling outcome detail lives in execution records/reports.
2. **Per-sibling plan derivation reuses `plan_execution`.** Each sibling gets its own `ExecutionRunPolicy` produced by calling `plan_execution` once per requested profile with the same approved task, same project, same permission envelope rules. Cross-provider model leakage rejection already prevents one provider's model from traveling to another sibling. If any sibling's plan fails validation, the entire fan-out fails closed before anything starts; partial fan-outs are never launched.
3. **Slot accounting.** Every sibling consumes global and per-project slots exactly like a standalone execution. No sibling bypasses `ExecutionLimits`; no new slot class ships in M10 (per ADR-0018 consequences, provider-scoped limits remain additive-later). Consequence stated plainly: with default limits (`max_parallel = 2`, `per_project_max_parallel = 1`), a two-way fan-out runs one sibling immediately and queues the other. Operators who want simultaneous comparison must raise limits explicitly. Document this in the Run approval copy.
4. **Cancellation semantics.** Cancelling the parent task cancels all siblings deterministically: the scheduler fans cancellation to every live sibling token and records each cancelled sibling individually. Cancelling one sibling alone is permitted for operators (it is just an execution), but does not change the parent rollup rule below. Queued-but-unstarted siblings cancel cleanly like queued executions do today.
5. **Sibling disagreement handling (deterministic, no verdict invention).**
   - All siblings DONE -> parent DONE.
   - Any sibling escalates (NEEDS_CLARIFICATION / NEEDS_DECISION) -> parent takes that escalation state; the packet names the offending sibling(s).
   - Mixed DONE/FAILED with no escalation -> parent NEEDS_DECISION, because choosing which output survives is a material decision a human owns (invariant 5). The packet lists per-sibling outcomes.
   - All FAILED -> parent FAILED.
   - Repair remains bounded per sibling exactly as today; repair bounds do not multiply across siblings beyond each policy's clamped values.
6. **Tests:** fake-runtime fan-out of N=2 and N=3; fail-closed when one sibling profile cannot be planned; slot serialization under default limits; parent-cancel cancels running and queued siblings; every rollup branch in requirement 5.

## Boundaries

- Presentation may display sibling rows but owns no fan-out policy.
- No aggregation logic in this phase; reports remain per-execution until phase 03.
- Scheduler changes stay additive to frozen M6 semantics.

## Done when

- Two fake-runtime siblings complete against one parent task with correct store records and rollup state.
- Parent cancellation leaves zero live sibling processes.
- Full workspace gates pass; phase 03 may begin only after this phase is proven.
